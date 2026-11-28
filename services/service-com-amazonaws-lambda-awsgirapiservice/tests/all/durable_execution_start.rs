// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the durable execution an `Invoke` starts.
//!
//! The rule under test: an invocation this service ACCEPTS starts one durable execution
//! when it invokes a durable function — one whose resolved configuration carries a
//! `DurableConfig`, "Configuration settings for durable functions" — or when it names a
//! `DurableExecutionName`; the execution runs under the version the call resolved, is
//! named by the request or by a generated unique identifier ("If not specified, Lambda
//! generates a unique identifier automatically"), and the call answers the ARN it is
//! addressed by in `X-Amz-Durable-Execution-Arn` ("The ARN of the durable execution that
//! was started"). From that moment `ListDurableExecutionsByFunction` reports exactly that
//! execution. A second invocation reusing a live name starts no duplicate: with the same
//! payload it answers the execution already started, and with a different payload it is
//! refused `DurableExecutionAlreadyStartedException` (409). An invocation that starts no
//! execution answers no ARN at all.
//!
//! Every assertion is over real HTTP requests through the router — the invocation that
//! starts the execution, and the listing that reads it back — so an ARN that was never
//! persisted fails here. The rows differ along what the rule generalises over: whether
//! the function carries a `DurableConfig`, whether the request names an execution and
//! with what name (each published length bound, each character class the pattern admits),
//! the qualifier the call resolves ($LATEST, a published version, an alias), the payload,
//! and the invocation type.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::ComAmazonawsLambdaAwsgirapiserviceState;

use crate::function_lifecycle::{
    create_function, json_call, make_activation_due, package_bytes, router_on, send, zip_file,
    Answer, ACCOUNT, REGION, ROLE,
};

/// Where `Invoke` is bound, from `smithy_query(query="operation", operation="Invoke")`
/// (`POST /2015-03-31/functions/{FunctionName}/invocations`).
const INVOCATIONS_URI: &str = "/2015-03-31/functions";

/// Where `ListDurableExecutionsByFunction` is bound
/// (`GET /2025-12-01/functions/{FunctionName}/durable-executions`).
const DURABLE_EXECUTIONS_URI: &str = "/2025-12-01/functions";

/// The response header `Invoke$DurableExecutionArn` is bound to.
///
/// This and the modelled facts below it are `pub(crate)` so the suites of this target that
/// assert the same modelled constraint read it from ONE place and cannot drift from the
/// model (`tests/all/invoke_durable_execution_handle.rs` is the other reader).
pub(crate) const DURABLE_EXECUTION_ARN_HEADER: &str = "x-amz-durable-execution-arn";

/// The request header `Invoke$DurableExecutionName` is bound to.
pub(crate) const DURABLE_EXECUTION_NAME_HEADER: &str = "x-amz-durable-execution-name";

/// The infix the model's `DurableExecutionArn` pattern puts between the function version
/// and the execution's name and id.
pub(crate) const ARN_INFIX: &str = "/durable-execution/";

/// The bounds the model puts on `DurableExecutionName` (`@length(min: 1, max: 64)`), from
/// `smithy_query(query="operation", operation="Invoke")`.
pub(crate) const NAME_MIN: usize = 1;
pub(crate) const NAME_MAX: usize = 64;

/// A customer managed key ARN satisfying the model's `KMSKeyArn` pattern, which the
/// durable functions here name in their `DurableConfig`.
pub(crate) const KMS_KEY: &str = "arn:aws:kms:us-east-1:000000000000:key/durable-executions";

/// The `ExecutionStatus` a freshly started execution carries, from
/// `smithy_query(query="shape", shape="ExecutionStatus")` (`RUNNING | SUCCEEDED | FAILED |
/// TIMED_OUT | STOPPED`).
pub(crate) const STATUS_RUNNING: &str = "RUNNING";

/// The unpublished version label, as the model's `Version` pattern spells it.
pub(crate) const VERSION_LATEST: &str = "$LATEST";

/// `Invoke` on the function `reference` names, with `query` appended, carrying `payload`
/// and naming `execution` as the durable execution when it names one.
async fn invoke(
    router: &smithy_server_runtime::MultiServiceRouter,
    reference: &str,
    query: &str,
    invocation_type: &str,
    execution: Option<&str>,
    payload: &[u8],
) -> Answer {
    let mut headers = vec![("x-amz-invocation-type", invocation_type.to_string())];
    if let Some(name) = execution {
        headers.push((DURABLE_EXECUTION_NAME_HEADER, name.to_string()));
    }
    send(
        router,
        "POST",
        &format!("{INVOCATIONS_URI}/{reference}/invocations{query}"),
        "application/octet-stream",
        payload.to_vec(),
        &headers,
    )
    .await
}

/// Where `GetDurableExecution` is bound
/// (`GET /2025-12-01/durable-executions/{DurableExecutionArn}`). The label is not greedy,
/// so the slashes the ARN carries are percent-encoded in the path, exactly as an SDK
/// encodes them.
async fn get_execution(
    router: &smithy_server_runtime::MultiServiceRouter,
    arn: &str,
    query: &str,
) -> Answer {
    json_call(
        router,
        "GET",
        &format!(
            "/2025-12-01/durable-executions/{}{query}",
            arn.replace('/', "%2F")
        ),
        &Value::Null,
    )
    .await
}

/// `ListDurableExecutionsByFunction` on the function `reference` names, with `query`
/// appended.
async fn list(
    router: &smithy_server_runtime::MultiServiceRouter,
    reference: &str,
    query: &str,
) -> Answer {
    json_call(
        router,
        "GET",
        &format!("{DURABLE_EXECUTIONS_URI}/{reference}/durable-executions{query}"),
        &Value::Null,
    )
    .await
}

/// The executions an answer reported, in the order it reported them.
fn executions(answer: &Answer) -> Vec<Value> {
    answer
        .json()
        .get("DurableExecutions")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// A string member of one reported execution.
fn member(execution: &Value, name: &str) -> Option<String> {
    execution
        .get(name)
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// The sentence a refusal carries, under either spelling the model gives the member.
fn refusal_message(answer: &Answer) -> String {
    let body = answer.json();
    for name in ["message", "Message"] {
        if let Some(stated) = body.get(name).and_then(Value::as_str) {
            return stated.to_string();
        }
    }
    String::new()
}

/// How many asynchronous invocations the function `name` holds queued — the other thing an
/// accepted `Event` invocation performs, read through the service's own state because
/// Lambda exposes no operation that reports the queue.
fn queued_invocations(engine: &Arc<engine::InMemoryEngine>, name: &str) -> usize {
    let state = engine.state::<ComAmazonawsLambdaAwsgirapiserviceState>();
    let store = state.lock();
    store
        .functions
        .get(name)
        .map_or(0, |record| record.async_invocations.len())
}

/// The unqualified ARN a function of `name` carries.
pub(crate) fn function_arn(name: &str) -> String {
    format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:{name}")
}

/// The ARN of `version` of the function `name`, as the model's `NameSpacedFunctionArn`
/// pattern spells it.
pub(crate) fn versioned_arn(name: &str, version: &str) -> String {
    if version == VERSION_LATEST {
        function_arn(name)
    } else {
        format!("{}:{version}", function_arn(name))
    }
}

/// Whether `name` satisfies the model's `DurableExecutionName` pattern
/// (`^[a-zA-Z0-9-_]+$`) and its length bounds.
pub(crate) fn is_execution_name(name: &str) -> bool {
    (NAME_MIN..=NAME_MAX).contains(&name.chars().count())
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}

/// Create the function `name`, publishing a version and an alias so every qualifier form
/// resolves, and bring it to the state an invocation needs. `durable` gives it the
/// `DurableConfig` that makes it a durable function.
async fn create_invokable_function(
    router: &smithy_server_runtime::MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    name: &str,
    durable: bool,
) {
    let mut body = json!({
        "FunctionName": name,
        "Role": ROLE,
        "Runtime": "python3.12",
        "Handler": "index.handler",
        "Publish": true,
        "Code": { "ZipFile": zip_file(&package_bytes(64)) },
    });
    if durable {
        body["DurableConfig"] = json!({ "KMSKeyArn": KMS_KEY, "ExecutionTimeout": 3_600 });
    }
    let created = create_function(router, &body).await;
    assert_eq!(
        created.status,
        201,
        "case {name}: CreateFunction must answer 201, got {} {}",
        created.status,
        String::from_utf8_lossy(&created.body)
    );
    let alias = json_call(
        router,
        "POST",
        &format!("{INVOCATIONS_URI}/{name}/aliases"),
        &json!({ "Name": "live", "FunctionVersion": "1" }),
    )
    .await;
    assert_eq!(
        alias.status,
        201,
        "case {name}: CreateAlias must answer 201, got {} {}",
        alias.status,
        String::from_utf8_lossy(&alias.body)
    );
    make_activation_due(engine);
}

/// One row of the start table: how the function is configured and addressed, the name the
/// request gives the execution, and the version the execution must run under.
struct StartCase {
    case: &'static str,
    durable: bool,
    query: &'static str,
    requested_name: Option<String>,
    payload: &'static [u8],
    version: &'static str,
}

/// The rows: a durable function invoked at each qualifier form, with a name at each
/// published length bound and one carrying every character class the pattern admits, with
/// no name at all, and a function with no `DurableConfig` whose invocation names an
/// execution.
fn start_cases() -> Vec<StartCase> {
    vec![
        StartCase {
            case: "a one-character name, the published minimum, at the unpublished version",
            durable: true,
            query: "",
            requested_name: Some("a".repeat(NAME_MIN)),
            payload: b"",
            version: VERSION_LATEST,
        },
        StartCase {
            case: "a name at the published maximum length, with a JSON payload",
            durable: true,
            query: "",
            requested_name: Some("n".repeat(NAME_MAX)),
            payload: br#"{"key":"value"}"#,
            version: VERSION_LATEST,
        },
        StartCase {
            case: "a name carrying a digit, a hyphen and an underscore, on a published version",
            durable: true,
            query: "?Qualifier=1",
            requested_name: Some("Exec-9_name".to_string()),
            payload: br#"{"wait_seconds":5}"#,
            version: "1",
        },
        StartCase {
            case: "an alias, which resolves to the version it invokes",
            durable: true,
            query: "?Qualifier=live",
            requested_name: Some("through-the-alias".to_string()),
            payload: b"[]",
            version: "1",
        },
        StartCase {
            case: "no name at all, so one is generated",
            durable: true,
            query: "",
            requested_name: None,
            payload: br#"{"generated":true}"#,
            version: VERSION_LATEST,
        },
        StartCase {
            case: "a function with no DurableConfig whose invocation names an execution",
            durable: false,
            query: "",
            requested_name: Some("named-on-a-plain-function".to_string()),
            payload: b"{}",
            version: VERSION_LATEST,
        },
    ]
}

#[tokio::test]
async fn an_accepted_invocation_starts_the_durable_execution_it_addresses() {
    for case in start_cases() {
        let engine = Arc::new(engine::InMemoryEngine::new());
        let router = router_on(&engine);
        let function = "durable-start";
        create_invokable_function(&router, &engine, function, case.durable).await;

        let answer = invoke(
            &router,
            function,
            case.query,
            "Event",
            case.requested_name.as_deref(),
            case.payload,
        )
        .await;
        assert_eq!(
            answer.status,
            202,
            "case {:?}: an accepted asynchronous invocation must answer 202, got {} {}",
            case.case,
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        let arn = answer.header(DURABLE_EXECUTION_ARN_HEADER).unwrap_or_default();
        assert!(
            !arn.is_empty(),
            "case {:?}: the invocation must answer the started execution's ARN in \
             {DURABLE_EXECUTION_ARN_HEADER}, got {:?}",
            case.case,
            answer.headers
        );

        // The listing is the independent read: the execution is there, exactly one of it,
        // under the ARN the invocation answered.
        let listed = list(&router, function, "").await;
        assert_eq!(
            listed.status,
            200,
            "case {:?}: the listing must answer 200, got {} {}",
            case.case,
            listed.status,
            String::from_utf8_lossy(&listed.body)
        );
        let reported = executions(&listed);
        assert_eq!(
            reported.len(),
            1,
            "case {:?}: expected the one execution the invocation started, got {reported:?}",
            case.case
        );
        let execution = &reported[0];
        assert_eq!(
            member(execution, "DurableExecutionArn"),
            Some(arn.clone()),
            "case {:?}: the listed execution must be the one the invocation answered, got \
             {execution:?}",
            case.case
        );
        let name = member(execution, "DurableExecutionName").unwrap_or_default();
        match &case.requested_name {
            Some(requested) => assert_eq!(
                &name, requested,
                "case {:?}: the execution must carry the name the request gave it, got \
                 {execution:?}",
                case.case
            ),
            // "If not specified, Lambda generates a unique identifier automatically."
            None => assert!(
                is_execution_name(&name),
                "case {:?}: a generated name must satisfy the modelled pattern and bounds, \
                 got {name:?}",
                case.case
            ),
        }
        assert_eq!(
            member(execution, "FunctionArn"),
            Some(versioned_arn(function, case.version)),
            "case {:?}: the execution must run under the version the call resolved, got \
             {execution:?}",
            case.case
        );
        assert_eq!(
            member(execution, "Status"),
            Some(STATUS_RUNNING.to_string()),
            "case {:?}: a freshly started execution is {STATUS_RUNNING}, got {execution:?}",
            case.case
        );
        assert_eq!(
            member(execution, "KMSKeyArn"),
            case.durable.then(|| KMS_KEY.to_string()),
            "case {:?}: the execution must carry the key its DurableConfig names, got \
             {execution:?}",
            case.case
        );
        // The ARN is the function version's, qualified even for the unpublished version,
        // then the execution's name and a minted id — the one form the modelled
        // `DurableExecutionArn` pattern admits.
        let expected_prefix = format!(
            "{}:{}{ARN_INFIX}{name}/",
            function_arn(function),
            case.version
        );
        let id = arn.strip_prefix(&expected_prefix).unwrap_or_default();
        assert!(
            !id.is_empty() && is_execution_name(id),
            "case {:?}: expected an ARN {expected_prefix}<id> with a minted id, got {arn:?}",
            case.case
        );
        // The execution is addressable by the name it carries, which is what a client
        // polls for.
        let by_name = list(&router, function, &format!("?DurableExecutionName={name}")).await;
        assert_eq!(
            executions(&by_name)
                .iter()
                .filter_map(|entry| member(entry, "DurableExecutionArn"))
                .collect::<Vec<String>>(),
            vec![arn.clone()],
            "case {:?}: the execution must be listable under its own name, got {}",
            case.case,
            String::from_utf8_lossy(&by_name.body)
        );

        // And it is addressable by its ARN: `GetDurableExecution` reports the same record
        // the listing does, plus the input payload the start carried and the function's
        // own durable configuration.
        let read = get_execution(&router, &arn, "").await;
        assert_eq!(
            read.status,
            200,
            "case {:?}: GetDurableExecution must answer 200, got {} {}",
            case.case,
            read.status,
            String::from_utf8_lossy(&read.body)
        );
        assert_eq!(
            (
                read.string("DurableExecutionArn"),
                read.string("DurableExecutionName"),
                read.string("FunctionArn"),
                read.string("Status"),
                read.string("Version"),
            ),
            (
                Some(arn.clone()),
                Some(name.clone()),
                Some(versioned_arn(function, case.version)),
                Some(STATUS_RUNNING.to_string()),
                Some(case.version.to_string()),
            ),
            "case {:?}: the read must report the execution the invocation started, got {}",
            case.case,
            String::from_utf8_lossy(&read.body)
        );
        assert_eq!(
            read.string("InputPayload"),
            (!case.payload.is_empty())
                .then(|| String::from_utf8_lossy(case.payload).to_string()),
            "case {:?}: the read must report the payload the start carried, got {}",
            case.case,
            String::from_utf8_lossy(&read.body)
        );
        assert_eq!(
            read.json()["ExecutionDataIncluded"],
            Value::Bool(true),
            "case {:?}: the read must say the execution data is included, got {}",
            case.case,
            String::from_utf8_lossy(&read.body)
        );
        assert_eq!(
            read.json()["DurableConfig"]["KMSKeyArn"].as_str(),
            case.durable.then_some(KMS_KEY),
            "case {:?}: the read must report the function's durable configuration, got {}",
            case.case,
            String::from_utf8_lossy(&read.body)
        );

        // "Set to false for a more compact response that includes only execution
        // metadata": the metadata is still reported, the data is not.
        let compact = get_execution(&router, &arn, "?IncludeExecutionData=false").await;
        assert_eq!(
            (
                compact.status,
                compact.string("DurableExecutionArn"),
                compact.json()["ExecutionDataIncluded"].clone(),
                compact.json()["InputPayload"].clone(),
            ),
            (200, Some(arn.clone()), Value::Bool(false), Value::Null),
            "case {:?}: a compact read must report the metadata and no payload, got {} {}",
            case.case,
            compact.status,
            String::from_utf8_lossy(&compact.body)
        );

        // An ARN that names the right execution under another id names an execution this
        // emulator never started.
        let absent = get_execution(
            &router,
            &format!("{expected_prefix}0123456789abcdef0123456789abcdef"),
            "",
        )
        .await;
        assert_eq!(
            absent.status,
            404,
            "case {:?}: an ARN naming no started execution must be 404, got {} {}",
            case.case,
            absent.status,
            String::from_utf8_lossy(&absent.body)
        );
    }
}

/// One row of the repeat table: the payload the second invocation carries, and whether
/// that payload is the one the execution was started with.
struct RepeatCase {
    case: &'static str,
    first: &'static [u8],
    second: &'static [u8],
    same_payload: bool,
}

/// The rows: a repeat with the identical payload (empty and non-empty), and a repeat whose
/// payload differs by a value, by its shape and by being absent.
fn repeat_cases() -> Vec<RepeatCase> {
    vec![
        RepeatCase {
            case: "the same empty payload",
            first: b"",
            second: b"",
            same_payload: true,
        },
        RepeatCase {
            case: "the same JSON payload",
            first: br#"{"wait_seconds":5}"#,
            second: br#"{"wait_seconds":5}"#,
            same_payload: true,
        },
        RepeatCase {
            case: "a payload differing by one value",
            first: br#"{"wait_seconds":5}"#,
            second: br#"{"wait_seconds":6}"#,
            same_payload: false,
        },
        RepeatCase {
            case: "a payload of another shape",
            first: br#"{"wait_seconds":5}"#,
            second: b"[1,2,3]",
            same_payload: false,
        },
        RepeatCase {
            case: "no payload at all where there was one",
            first: br#"{"wait_seconds":5}"#,
            second: b"",
            same_payload: false,
        },
    ]
}

#[tokio::test]
async fn a_repeat_of_a_live_execution_name_starts_no_duplicate() {
    for case in repeat_cases() {
        let engine = Arc::new(engine::InMemoryEngine::new());
        let router = router_on(&engine);
        let function = "durable-repeat";
        create_invokable_function(&router, &engine, function, true).await;
        let name = "repeated-execution";

        let first = invoke(&router, function, "", "Event", Some(name), case.first).await;
        assert_eq!(
            first.status,
            202,
            "case {:?}: the first invocation must be accepted, got {} {}",
            case.case,
            first.status,
            String::from_utf8_lossy(&first.body)
        );
        let arn = first.header(DURABLE_EXECUTION_ARN_HEADER).unwrap_or_default();
        assert!(
            !arn.is_empty(),
            "case {:?}: the first invocation must answer an execution ARN",
            case.case
        );

        let repeat = invoke(&router, function, "", "Event", Some(name), case.second).await;
        if case.same_payload {
            // "If you invoke a durable function using a name that already exists with the
            // same payload, Lambda returns the existing execution instead of creating a
            // duplicate."
            assert_eq!(
                (repeat.status, repeat.header(DURABLE_EXECUTION_ARN_HEADER)),
                (202, Some(arn.clone())),
                "case {:?}: a repeat with the same payload must answer the execution already \
                 started, got {} {}",
                case.case,
                repeat.status,
                String::from_utf8_lossy(&repeat.body)
            );
        } else {
            // "If the payload differs, Lambda returns a
            // DurableExecutionAlreadyStartedException error."
            assert_eq!(
                repeat.status,
                409,
                "case {:?}: a repeat with a different payload must be refused 409, got {} {}",
                case.case,
                repeat.status,
                String::from_utf8_lossy(&repeat.body)
            );
            assert!(
                repeat
                    .error_code()
                    .contains("DurableExecutionAlreadyStartedException"),
                "case {:?}: expected the modelled DurableExecutionAlreadyStartedException, got \
                 {:?}",
                case.case,
                repeat.error_code()
            );
            assert!(
                repeat.header(DURABLE_EXECUTION_ARN_HEADER).is_none(),
                "case {:?}: a refused invocation must answer no execution ARN, got {:?}",
                case.case,
                repeat.header(DURABLE_EXECUTION_ARN_HEADER)
            );
            // The refusal NAMES the execution already started: the fixed sentence, then that
            // execution's own ARN, which is how a client recovers the in-flight execution
            // from the refusal — the ARN every read of a durable execution is addressed by.
            assert_eq!(
                refusal_message(&repeat),
                format!("Execution already started: {arn}"),
                "case {:?}: expected the refusal to name the execution already started, got {}",
                case.case,
                String::from_utf8_lossy(&repeat.body)
            );
        }

        // Either way the function holds the ONE execution of that name the first
        // invocation started, and the repeat performed nothing a second time: a
        // de-duplicated invocation is not a second invocation, and a refused one is no
        // invocation at all.
        let listed = list(&router, function, &format!("?DurableExecutionName={name}")).await;
        assert_eq!(
            executions(&listed)
                .iter()
                .filter_map(|entry| member(entry, "DurableExecutionArn"))
                .collect::<Vec<String>>(),
            vec![arn.clone()],
            "case {:?}: the name must still address the one execution started, got {}",
            case.case,
            String::from_utf8_lossy(&listed.body)
        );
        assert_eq!(
            queued_invocations(&engine, function),
            1,
            "case {:?}: the repeat must queue no second asynchronous invocation",
            case.case
        );
    }
}

/// One row of the no-execution table: the function's configuration, the invocation type,
/// whether a name is named, and the status the invocation answers.
struct NoStartCase {
    case: &'static str,
    durable: bool,
    invocation_type: &'static str,
    requested_name: Option<&'static str>,
    status: u16,
}

/// The rows: an invocation of a function that is not durable and names no execution, and
/// a dry run of a durable function. A SYNCHRONOUS invocation of a durable function does
/// start one — "This is returned when invoking a durable function" is not qualified by the
/// invocation type — and is covered by `invoke_durable_execution_handle`.
fn no_start_cases() -> Vec<NoStartCase> {
    vec![
        NoStartCase {
            case: "a function with no DurableConfig, naming no execution",
            durable: false,
            invocation_type: "Event",
            requested_name: None,
            status: 202,
        },
        NoStartCase {
            case: "a dry run, which validates the invocation and runs nothing",
            durable: true,
            invocation_type: "DryRun",
            requested_name: Some("dry-run-execution"),
            status: 204,
        },
    ]
}

#[tokio::test]
async fn an_invocation_that_starts_no_execution_answers_no_arn() {
    for case in no_start_cases() {
        let engine = Arc::new(engine::InMemoryEngine::new());
        let router = router_on(&engine);
        let function = "durable-none";
        create_invokable_function(&router, &engine, function, case.durable).await;

        let answer = invoke(
            &router,
            function,
            "",
            case.invocation_type,
            case.requested_name,
            b"{}",
        )
        .await;
        assert_eq!(
            answer.status,
            case.status,
            "case {:?}: expected HTTP {}, got {} {}",
            case.case,
            case.status,
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert!(
            answer.header(DURABLE_EXECUTION_ARN_HEADER).is_none(),
            "case {:?}: expected no execution ARN, got {:?}",
            case.case,
            answer.header(DURABLE_EXECUTION_ARN_HEADER)
        );
        let listed = list(&router, function, "").await;
        assert_eq!(
            (listed.status, executions(&listed)),
            (200, Vec::new()),
            "case {:?}: an invocation that started no execution must leave nothing to \
             report, got {} {}",
            case.case,
            listed.status,
            String::from_utf8_lossy(&listed.body)
        );
    }
}
