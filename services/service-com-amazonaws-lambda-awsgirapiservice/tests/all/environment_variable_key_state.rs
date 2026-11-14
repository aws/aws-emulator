// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the KEY a function's ENVIRONMENT VARIABLES are encrypted with.
//!
//! THE RULE: the plaintext of a function's environment variables is readable only while the
//! key service says the customer managed key named by `KMSKeyArn` can decrypt them. So every
//! projection of that function's configuration reports `Environment.Variables` only while
//! that key can be used, and otherwise reports `EnvironmentResponse$Error`
//! (`EnvironmentError{ErrorCode, Message}` — "error messages for environment variables that
//! couldn't be applied") with no `Variables` at all; and every invocation of the function is
//! refused with the modelled 502 for the class of failure the key service reported, rather
//! than run. Both relay the key service's own answer — the exception class it stated the
//! refusal under and the sentence it stated about the key — and both reverse by themselves
//! once the key can decrypt again, because the key is asked at the point of use and nothing
//! is recorded.
//!
//! The rows differ along what the rule generalises over: the state the key is in (usable,
//! disabled, scheduled for deletion, the deletion cancelled, enabled again), which
//! projection of the configuration is read (`GetFunctionConfiguration`, `GetFunction`,
//! `ListFunctions`, `ListVersionsByFunction`), and what the function's key reference is (a
//! key the key service holds, a key it does not hold, no key at all, and a key with no
//! variables to decrypt).
//!
//! The key really is in the state a row says it is: it is created and moved between states
//! with the key service's own operations, over the same engine.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::durable_execution_completion::kms_call;
use crate::function_lifecycle::{
    create_function, json_call, make_activation_due, make_update_due, router_on, send, zip_file,
    Answer, ROLE,
};
use crate::synchronous_invoke_run::package;

/// Where `CreateFunction`, `ListFunctions` and `Invoke` are bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// The handler of every package here: it answers the value of the environment variable the
/// function was created with, so a served invocation can only have decrypted it.
const HANDLER_SOURCE: &str =
    "import os\ndef lambda_handler(event, context):\n    return {'secret': os.environ['SECRET']}\n";

/// The environment variable the handler reads, and the value it was given.
const VARIABLE: &str = "SECRET";
const MARKER: &str = "env-marker-1";

/// A well-formed key ARN of a key the key service holds no record of, satisfying the model's
/// `KMSKeyArn` pattern.
const ABSENT_KEY: &str = "arn:aws:kms:us-east-1:000000000000:key/11111111-2222-3333-4444-555555555555";

/// Lambda's own sentence for the decrypt it could not perform, as a REFUSED INVOCATION words
/// it — one per class of KMS failure, taken from the modelled exceptions' own documentation
/// (`smithy_query(query="operation", operation="Invoke")`: `KMSDisabledException`,
/// `KMSInvalidStateException`, `KMSNotFoundException`, `KMSAccessDeniedException`, each
/// `@httpError(502)`).
const INVOKE_DISABLED_SENTENCE: &str =
    "Lambda was unable to decrypt the environment variables because the KMS key used is \
     disabled. Please check the function's KMS key settings.";
const INVOKE_INVALID_STATE_SENTENCE: &str =
    "Lambda was unable to decrypt the environment variables because the state of the KMS key \
     used is not valid for Decrypt. Please check the function's KMS key settings.";
const INVOKE_NOT_FOUND_SENTENCE: &str =
    "Lambda was unable to decrypt the environment variables because the KMS key was not found. \
     Please check the function's KMS key settings.";

/// The same sentences as a CONFIGURATION READ words them, of the caller's own environment and
/// the caller's own key settings.
const REPORT_DISABLED_SENTENCE: &str =
    "Lambda was unable to decrypt your environment variables because the KMS key used is \
     disabled. Please check your KMS key settings.";
const REPORT_INVALID_STATE_SENTENCE: &str =
    "Lambda was unable to decrypt your environment variables because the state of the KMS key \
     used is not valid for Decrypt. Please check your KMS key settings.";
const REPORT_NOT_FOUND_SENTENCE: &str =
    "Lambda was unable to decrypt your environment variables because the KMS key was not found. \
     Please check your KMS key settings.";

/// The envelope the SDK layer that relayed the decrypt states around the key service's
/// answer: the service it was made to, the status that service answers a refusal of the
/// caller's own request with, that call's own request id and the attempts it took.
const RELAY_HEAD: &str = " (Service: Kms, Status Code: 400, Request ID: ";
const RELAY_TAIL: &str = ") (SDK Attempt Count: 1)";

/// A router carrying Lambda and the key service on ONE engine, so the key a function names is
/// a key this account really holds.
fn router_with_kms(engine: &Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = router_on(engine);
    router.register_service(
        service_com_amazonaws_kms_trentservice::EmulatedService::new(Arc::clone(engine)),
    );
    router
}

/// A symmetric encryption key this account holds, answered by its ARN.
async fn created_key(router: &MultiServiceRouter) -> String {
    let created = kms_call(router, "CreateKey", &json!({})).await;
    assert_eq!(
        created.status,
        200,
        "case \"CreateKey\": expected 200, got {} {}",
        created.status,
        String::from_utf8_lossy(&created.body)
    );
    created
        .json()
        .pointer("/KeyMetadata/Arn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            panic!(
                "case \"CreateKey\": expected a key ARN, got {}",
                String::from_utf8_lossy(&created.body)
            )
        })
        .to_string()
}

/// `CreateFunction` for a function whose handler reads the environment variable, with the key
/// reference and the variables the caller named, then the activation the clock owes brought
/// forward.
async fn created_function(
    engine: &Arc<engine::InMemoryEngine>,
    router: &MultiServiceRouter,
    name: &str,
    kms_key_arn: Option<&str>,
    variables: Option<Value>,
) {
    let mut body = json!({
        "FunctionName": name,
        "Role": ROLE,
        "Runtime": "python3.12",
        "Handler": "lambda_function.lambda_handler",
        "Timeout": 5,
        "Code": { "ZipFile": zip_file(&package(HANDLER_SOURCE)) },
    });
    if let Some(key_arn) = kms_key_arn {
        body["KMSKeyArn"] = json!(key_arn);
    }
    if let Some(variables) = variables {
        body["Environment"] = json!({ "Variables": variables });
    }
    let created = create_function(router, &body).await;
    assert_eq!(
        created.status,
        201,
        "case {name:?}: expected 201 from CreateFunction, got {} {}",
        created.status,
        String::from_utf8_lossy(&created.body)
    );
    make_activation_due(engine);
}

/// The four READS that project a function's configuration, each answered as the
/// `FunctionConfiguration` document it reports for `name`.
async fn projections(router: &MultiServiceRouter, name: &str) -> Vec<(&'static str, Value)> {
    let configuration = json_call(
        router,
        "GET",
        &format!("{FUNCTIONS_URI}/{name}/configuration"),
        &Value::Null,
    )
    .await;
    let whole = json_call(router, "GET", &format!("{FUNCTIONS_URI}/{name}"), &Value::Null).await;
    let listed = json_call(router, "GET", FUNCTIONS_URI, &Value::Null).await;
    let versions = json_call(
        router,
        "GET",
        &format!("{FUNCTIONS_URI}/{name}/versions"),
        &Value::Null,
    )
    .await;
    for (read, answer) in [
        ("GetFunctionConfiguration", &configuration),
        ("GetFunction", &whole),
        ("ListFunctions", &listed),
        ("ListVersionsByFunction", &versions),
    ] {
        assert_eq!(
            answer.status,
            200,
            "case {read:?}: expected 200 from the read, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
    }
    let one_of = |answer: &Answer, member: &str| -> Value {
        answer
            .json()
            .get(member)
            .and_then(Value::as_array)
            .and_then(|entries| {
                entries
                    .iter()
                    .find(|entry| {
                        entry.get("FunctionName").and_then(Value::as_str) == Some(name)
                    })
                    .cloned()
            })
            .unwrap_or_else(|| {
                panic!(
                    "expected {member:?} to carry {name:?}, got {}",
                    String::from_utf8_lossy(&answer.body)
                )
            })
    };
    vec![
        ("GetFunctionConfiguration", configuration.json()),
        (
            "GetFunction",
            whole
                .json()
                .get("Configuration")
                .cloned()
                .unwrap_or(Value::Null),
        ),
        ("ListFunctions", one_of(&listed, "Functions")),
        ("ListVersionsByFunction", one_of(&versions, "Versions")),
    ]
}

/// `Invoke` on `name`, synchronously, with an empty event.
async fn invoke(router: &MultiServiceRouter, name: &str) -> Answer {
    send(
        router,
        "POST",
        &format!("{FUNCTIONS_URI}/{name}/invocations"),
        "application/json",
        b"{}".to_vec(),
        &[],
    )
    .await
}

/// The `Message` an answer carries, whichever casing the channel that rendered it used.
fn message_of(answer: &Answer) -> String {
    answer
        .string("Message")
        .or_else(|| answer.string("message"))
        .unwrap_or_default()
}

/// What the configuration must report for one key state, and what an invocation must answer.
struct Step {
    /// What makes this step its own case.
    case: &'static str,
    /// The key-service call that puts the key into this step's state.
    key_call: Option<(&'static str, bool)>,
    /// The `EnvironmentError.ErrorCode` the configuration must report, or `None` when the
    /// variables are readable.
    error_code: Option<&'static str>,
    /// Lambda's sentence on that error, and on the refusal of an invocation.
    report_sentence: &'static str,
    invoke_sentence: &'static str,
    /// The sentence the key service states about the key, after the key's own ARN.
    key_reason: &'static str,
    /// The modelled error an invocation must be refused with, or `None` when it must run.
    invoke_error: Option<&'static str>,
}

/// A function's environment variables are readable, and its invocations are served, exactly
/// while the key they are encrypted with can decrypt them — and the key moving between states
/// moves both, in both directions.
#[tokio::test]
async fn the_environment_and_its_invocations_follow_the_key_through_its_states() {
    let steps = [
        Step {
            case: "a key the key service reports as usable",
            key_call: None,
            error_code: None,
            report_sentence: "",
            invoke_sentence: "",
            key_reason: "",
            invoke_error: None,
        },
        Step {
            case: "a disabled key",
            key_call: Some(("DisableKey", false)),
            error_code: Some("DisabledException"),
            report_sentence: REPORT_DISABLED_SENTENCE,
            invoke_sentence: INVOKE_DISABLED_SENTENCE,
            key_reason: "is disabled.",
            invoke_error: Some("KMSDisabledException"),
        },
        Step {
            case: "the key enabled again",
            key_call: Some(("EnableKey", false)),
            error_code: None,
            report_sentence: "",
            invoke_sentence: "",
            key_reason: "",
            invoke_error: None,
        },
        Step {
            case: "a key scheduled for deletion",
            key_call: Some(("ScheduleKeyDeletion", true)),
            error_code: Some("KMSInvalidStateException"),
            report_sentence: REPORT_INVALID_STATE_SENTENCE,
            invoke_sentence: INVOKE_INVALID_STATE_SENTENCE,
            key_reason: "is pending deletion.",
            invoke_error: Some("KMSInvalidStateException"),
        },
        Step {
            // Cancelling a scheduled deletion leaves the key DISABLED, so the class the
            // configuration reports changes with the key's state.
            case: "the scheduled deletion cancelled",
            key_call: Some(("CancelKeyDeletion", false)),
            error_code: Some("DisabledException"),
            report_sentence: REPORT_DISABLED_SENTENCE,
            invoke_sentence: INVOKE_DISABLED_SENTENCE,
            key_reason: "is disabled.",
            invoke_error: Some("KMSDisabledException"),
        },
        Step {
            case: "the key enabled once more",
            key_call: Some(("EnableKey", false)),
            error_code: None,
            report_sentence: "",
            invoke_sentence: "",
            key_reason: "",
            invoke_error: None,
        },
    ];

    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_with_kms(&engine);
    let key_arn = created_key(&router).await;
    let name = "env-key-states";
    created_function(
        &engine,
        &router,
        name,
        Some(&key_arn),
        Some(json!({ VARIABLE: MARKER })),
    )
    .await;

    for step in &steps {
        if let Some((operation, with_window)) = step.key_call {
            let mut body = json!({ "KeyId": key_arn });
            if with_window {
                // The floor of `ScheduleKeyDeletionRequest$PendingWindowInDays`
                // (`@range(min: 7, max: 30)`).
                body["PendingWindowInDays"] = json!(7);
            }
            let answered = kms_call(&router, operation, &body).await;
            assert_eq!(
                answered.status,
                200,
                "case {:?}: expected 200 from {operation}, got {} {}",
                step.case,
                answered.status,
                String::from_utf8_lossy(&answered.body)
            );
        }

        for (read, configuration) in projections(&router, name).await {
            let case = format!("{} read by {read}", step.case);
            assert_eq!(
                configuration.get("KMSKeyArn").and_then(Value::as_str),
                Some(key_arn.as_str()),
                "case {case:?}: expected the configuration to echo the key ARN, got \
                 {configuration}"
            );
            let variables = configuration.pointer("/Environment/Variables");
            let reported_code = configuration
                .pointer("/Environment/Error/ErrorCode")
                .and_then(Value::as_str);
            match step.error_code {
                None => {
                    assert_eq!(
                        variables
                            .and_then(|variables| variables.get(VARIABLE))
                            .and_then(Value::as_str),
                        Some(MARKER),
                        "case {case:?}: expected the variable to be readable, got \
                         {configuration}"
                    );
                    assert!(
                        configuration.pointer("/Environment/Error").is_none(),
                        "case {case:?}: expected no Environment.Error, got {configuration}"
                    );
                }
                Some(expected) => {
                    assert_eq!(
                        reported_code,
                        Some(expected),
                        "case {case:?}: expected the relayed exception class, got \
                         {configuration}"
                    );
                    assert!(
                        variables.is_none(),
                        "case {case:?}: expected no Environment.Variables while the key \
                         cannot decrypt, got {configuration}"
                    );
                    let expected_message = format!(
                        "{} KMS Exception: {expected} KMS Message: {key_arn} {}",
                        step.report_sentence, step.key_reason
                    );
                    assert_eq!(
                        configuration
                            .pointer("/Environment/Error/Message")
                            .and_then(Value::as_str),
                        Some(expected_message.as_str()),
                        "case {case:?}: expected the error to state the decrypt and relay \
                         the key service's answer, got {configuration}"
                    );
                }
            }
        }

        // Two invocations, because a refusal is not a one-off: the key is asked again for
        // each, and each is answered identically but for the relayed call's own request id.
        let mut relayed_ids = Vec::new();
        for attempt in 0..2 {
            let answered = invoke(&router, name).await;
            let case = format!("{} invoked, attempt {attempt}", step.case);
            match step.invoke_error {
                None => {
                    assert_eq!(
                        answered.status,
                        200,
                        "case {case:?}: expected the invocation to run, got {} {}",
                        answered.status,
                        String::from_utf8_lossy(&answered.body)
                    );
                    assert_eq!(
                        answered.json().get("secret").and_then(Value::as_str),
                        Some(MARKER),
                        "case {case:?}: expected the handler's own payload, got {}",
                        String::from_utf8_lossy(&answered.body)
                    );
                }
                Some(expected) => {
                    assert_eq!(
                        answered.status,
                        502,
                        "case {case:?}: expected the modelled 502, got {} {}",
                        answered.status,
                        String::from_utf8_lossy(&answered.body)
                    );
                    assert_eq!(
                        answered.error_code(),
                        expected,
                        "case {case:?}: expected {expected}, got {}",
                        answered.error_code()
                    );
                    let message = message_of(&answered);
                    let head = format!(
                        "{} KMS Exception: {}KMS Message: {key_arn} {}{RELAY_HEAD}",
                        step.invoke_sentence,
                        step.error_code.unwrap_or_default(),
                        step.key_reason
                    );
                    assert!(
                        message.starts_with(&head) && message.ends_with(RELAY_TAIL),
                        "case {case:?}: expected the refusal to state {head:?} and end \
                         {RELAY_TAIL:?}, got {message:?}"
                    );
                    let relayed = message
                        .strip_prefix(&head)
                        .and_then(|rest| rest.strip_suffix(RELAY_TAIL))
                        .unwrap_or_default()
                        .to_string();
                    assert!(
                        !relayed.is_empty(),
                        "case {case:?}: expected the relayed call's own request id, got \
                         {message:?}"
                    );
                    relayed_ids.push(relayed);
                }
            }
        }
        if relayed_ids.len() == 2 {
            assert_ne!(
                relayed_ids[0], relayed_ids[1],
                "case {:?}: expected each relayed call its own request id, got {:?} twice",
                step.case, relayed_ids[0]
            );
        }
    }
}

/// A COLD execution environment — one built after an `UpdateFunctionConfiguration` has
/// settled — is the invocation that has to decrypt the variables again, so it is refused
/// while the key cannot decrypt them and served once the key can.
#[tokio::test]
async fn an_invocation_after_an_update_settles_follows_the_key() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_with_kms(&engine);
    let key_arn = created_key(&router).await;
    let name = "env-key-cold";
    created_function(
        &engine,
        &router,
        name,
        Some(&key_arn),
        Some(json!({ VARIABLE: MARKER })),
    )
    .await;

    let disabled = kms_call(&router, "DisableKey", &json!({ "KeyId": key_arn })).await;
    assert_eq!(
        disabled.status,
        200,
        "case \"DisableKey\": expected 200, got {} {}",
        disabled.status,
        String::from_utf8_lossy(&disabled.body)
    );

    // A change to the function's settings, settled the way the clock settles it.
    let updated = json_call(
        &router,
        "PUT",
        &format!("{FUNCTIONS_URI}/{name}/configuration"),
        &json!({ "Timeout": 11 }),
    )
    .await;
    assert_eq!(
        updated.status,
        200,
        "case \"UpdateFunctionConfiguration\": expected 200, got {} {}",
        updated.status,
        String::from_utf8_lossy(&updated.body)
    );
    make_update_due(&engine);
    let settled = json_call(
        &router,
        "GET",
        &format!("{FUNCTIONS_URI}/{name}/configuration"),
        &Value::Null,
    )
    .await;
    assert_eq!(
        settled.json().get("LastUpdateStatus").and_then(Value::as_str),
        Some("Successful"),
        "case \"the settled update\": expected the update to have settled, got {}",
        String::from_utf8_lossy(&settled.body)
    );

    let refused = invoke(&router, name).await;
    assert_eq!(
        refused.error_code(),
        "KMSDisabledException",
        "case \"a cold invocation under a disabled key\": expected the modelled refusal, got \
         {} {}",
        refused.status,
        String::from_utf8_lossy(&refused.body)
    );

    let enabled = kms_call(&router, "EnableKey", &json!({ "KeyId": key_arn })).await;
    assert_eq!(
        enabled.status,
        200,
        "case \"EnableKey\": expected 200, got {} {}",
        enabled.status,
        String::from_utf8_lossy(&enabled.body)
    );
    let served = invoke(&router, name).await;
    assert_eq!(
        served.json().get("secret").and_then(Value::as_str),
        Some(MARKER),
        "case \"the identical invocation once the key is usable\": expected the handler's own \
         payload, got {} {}",
        served.status,
        String::from_utf8_lossy(&served.body)
    );
}

/// One row: what the function's key reference and environment are, and what the configuration
/// and an invocation must therefore report.
struct Reference {
    case: &'static str,
    /// Whether the function names the key the key service holds, the key it does not hold, or
    /// no key at all.
    key: Option<&'static str>,
    /// Whether the function carries environment variables at all.
    variables: bool,
    /// The `ErrorCode` the configuration must report, and the modelled refusal of an
    /// invocation — `None` for both where the key covers nothing that has to be decrypted.
    error_code: Option<&'static str>,
    invoke_error: Option<&'static str>,
}

/// The rule is about the key the variables are ENCRYPTED WITH: a function with no key, and one
/// whose key covers no variables, have nothing to decrypt and are unaffected — while a key the
/// key service holds no record of cannot decrypt them, which is the not-found class of the
/// same refusal.
#[tokio::test]
async fn only_a_key_that_covers_variables_decides_what_is_reported() {
    let rows = [
        Reference {
            case: "no key at all",
            key: None,
            variables: true,
            error_code: None,
            invoke_error: None,
        },
        Reference {
            case: "a key covering no variables",
            key: Some("held"),
            variables: false,
            error_code: None,
            invoke_error: None,
        },
        Reference {
            case: "a key the key service holds no record of",
            key: Some(ABSENT_KEY),
            variables: true,
            error_code: Some("NotFoundException"),
            invoke_error: Some("KMSNotFoundException"),
        },
    ];

    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_with_kms(&engine);
    let held = created_key(&router).await;
    // Every row's key is DISABLED where it exists, so a row that reports nothing reports it
    // because the key covers nothing to decrypt rather than because the key is usable.
    let disabled = kms_call(&router, "DisableKey", &json!({ "KeyId": held })).await;
    assert_eq!(
        disabled.status,
        200,
        "case \"DisableKey\": expected 200, got {} {}",
        disabled.status,
        String::from_utf8_lossy(&disabled.body)
    );

    for (index, row) in rows.iter().enumerate() {
        let name = format!("env-key-reference-{index}");
        let key = match row.key {
            Some("held") => Some(held.clone()),
            Some(named) => Some(named.to_string()),
            None => None,
        };
        created_function(
            &engine,
            &router,
            &name,
            key.as_deref(),
            row.variables.then(|| json!({ VARIABLE: MARKER })),
        )
        .await;

        for (read, configuration) in projections(&router, &name).await {
            let case = format!("{} read by {read}", row.case);
            assert_eq!(
                configuration
                    .pointer("/Environment/Error/ErrorCode")
                    .and_then(Value::as_str),
                row.error_code,
                "case {case:?}: expected the reported error class to be {:?}, got \
                 {configuration}",
                row.error_code
            );
            if row.error_code.is_some() {
                let message = configuration
                    .pointer("/Environment/Error/Message")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let head = format!(
                    "{REPORT_NOT_FOUND_SENTENCE} KMS Exception: NotFoundException KMS Message: "
                );
                assert!(
                    message.starts_with(&head) && message.contains(ABSENT_KEY),
                    "case {case:?}: expected the error to state the decrypt and relay the key \
                     service's answer about {ABSENT_KEY}, got {message:?}"
                );
                assert!(
                    configuration.pointer("/Environment/Variables").is_none(),
                    "case {case:?}: expected no Environment.Variables, got {configuration}"
                );
            } else if row.variables {
                assert_eq!(
                    configuration
                        .pointer("/Environment/Variables")
                        .and_then(|variables| variables.get(VARIABLE))
                        .and_then(Value::as_str),
                    Some(MARKER),
                    "case {case:?}: expected the variable to be readable, got {configuration}"
                );
            }
        }

        let answered = invoke(&router, &name).await;
        match row.invoke_error {
            None if row.variables => {
                assert_eq!(
                    answered.json().get("secret").and_then(Value::as_str),
                    Some(MARKER),
                    "case {:?}: expected the handler's own payload, got {} {}",
                    row.case,
                    answered.status,
                    String::from_utf8_lossy(&answered.body)
                );
            }
            None => {
                // The handler reads a variable this row's function does not carry, so the run
                // ENDS in the handler's own error rather than in a refusal of the invocation:
                // the invocation was admitted and run, which is what this row asserts.
                assert_eq!(
                    answered.status,
                    200,
                    "case {:?}: expected the invocation to run, got {} {}",
                    row.case,
                    answered.status,
                    String::from_utf8_lossy(&answered.body)
                );
                assert_eq!(
                    answered.header("x-amz-function-error").as_deref(),
                    Some("Unhandled"),
                    "case {:?}: expected the run's own error, got {}",
                    row.case,
                    String::from_utf8_lossy(&answered.body)
                );
            }
            Some(expected) => {
                assert_eq!(
                    answered.status,
                    502,
                    "case {:?}: expected the modelled 502, got {} {}",
                    row.case,
                    answered.status,
                    String::from_utf8_lossy(&answered.body)
                );
                assert_eq!(
                    answered.error_code(),
                    expected,
                    "case {:?}: expected {expected}, got {}",
                    row.case,
                    answered.error_code()
                );
                let message = message_of(&answered);
                assert!(
                    message.starts_with(INVOKE_NOT_FOUND_SENTENCE)
                        && message.contains(ABSENT_KEY)
                        && message.ends_with(RELAY_TAIL),
                    "case {:?}: expected the refusal to state the decrypt and relay the key \
                     service's answer, got {message:?}",
                    row.case
                );
            }
        }
    }
}
