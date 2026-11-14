// SPDX-License-Identifier: CC0-1.0

//! Which refusal class Secrets Manager answers a bad request with, and the sentence it
//! carries.
//!
//! The rules under test, each over a TABLE of inputs rather than one case:
//!
//! 1. The three 400 classes mean different things, and a refusal is classified by MEANING,
//!    not by the operation that raised it: a value that failed the rules declared for its
//!    member is the constraint refusal `ValidationException`; a value the service cannot
//!    use whatever the resource holds is `InvalidParameterException` ("The parameter name
//!    or value is invalid."); and a request that cannot be served given what the resource
//!    holds right now — a quota the write would pass, two selectors that name two different
//!    versions, a value member the write needs and does not carry — is
//!    `InvalidRequestException` ("A parameter value is not valid for the current state of
//!    the resource."), both from this service's Smithy model.
//! 2. Each of those refusals carries the service's own FIXED sentence, held in one place
//!    ([`service_com_amazonaws_secretsmanager_secretsmanager::state`]) so every operation
//!    that raises one answers with the same words, and nothing of the request is
//!    interpolated into it beyond the identifier AWS names (a tag key).
//! 3. A constraint refusal states the member path and the requirement, with the rejected
//!    value echoed only up to a bounded prefix — a `@sensitive` member's value never, and a
//!    longer value as that prefix plus an elision marker — a `@pattern` without the
//!    whole-value anchors the model writes around it, and an enumerated type's admitted
//!    values sorted.
//!
//! The rows differ along what each rule generalises over: the operation and member that
//! carried the value, the constraint class (`@length` on a string and on a blob, `@pattern`,
//! `@range`, an enum value set), whether the member is `@sensitive`, the size of the value
//! (inside the echo bound, one byte past it, and far past it), how many tags the secret
//! already holds, which of the two rotation-schedule forms the rules name, and which
//! version each selector of a read resolves to.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_secretsmanager_secretsmanager::ops::types::FILTER_NAME_STRING_TYPE_VALUES;
use service_com_amazonaws_secretsmanager_secretsmanager::state::{
    system_tag_message, value_version_not_found_message, BOTH_SECRET_VALUES_MESSAGE,
    DUPLICATE_TAG_KEYS_MESSAGE, DURATION_WITHOUT_SCHEDULE, INVALID_SECRET_NAME_MESSAGE,
    NAME_SCHEDULED_FOR_DELETION_MESSAGE, NO_ROTATION_FUNCTION_MESSAGE, NO_SECRET_VALUE_MESSAGE,
    STATED_FILTER_NAME_VALUE_SET, TAGS_PER_SECRET, TOO_MANY_TAGS_MESSAGE,
    VERSION_ID_AND_STAGE_DISAGREE_MESSAGE,
};
use service_com_amazonaws_secretsmanager_secretsmanager::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 header naming `secretsmanager` in `us-west-2`; the router routes on the credential
/// scope and the emulator verifies signatures only when an authenticator is registered, which
/// this harness does not do.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-west-2/secretsmanager/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=deadbeef";

/// The wire error code of a constraint refusal: the code the boundary validator answers a
/// value outside a declared rule with.
const CONSTRAINT: &str = "ValidationException";

/// The modelled code for a parameter value the service cannot use.
const PARAMETER: &str = "InvalidParameterException";

/// The modelled code for a request the resource's state does not admit.
const STATE: &str = "InvalidRequestException";

/// The modelled code for something the request named that the service does not hold.
const NOT_FOUND: &str = "ResourceNotFoundException";

/// How many bytes of a rejected value this service's constraint clause echoes before the
/// rest is elided, and what stands in for the rest. The rendering is this service's
/// (`src/service/validation.rs`); a value inside the bound is stated as sent.
const ECHO_BOUND: usize = 64;
const ELISION: &str = " ...";

/// The published maximum of `TagKeyType` (`@length` 1..128, this service's Smithy model).
const TAG_KEY_MAX: usize = 128;

/// The published maximum of `SecretStringType` and `SecretBinaryType` (`@length` 1..65536,
/// both `@sensitive`, this service's Smithy model).
const SECRET_VALUE_MAX: usize = 65536;

/// The published bounds of `NameType`, the shape `CreateSecretRequest$Name` targets
/// (`@length` 1..512, this service's Smithy model). The accepted rows stay inside
/// `SecretNameType`'s own 256, which is the shape the response reports the name in.
const NAME_MAX: usize = 512;
const REPORTED_NAME_MAX: usize = 256;

/// The published bounds of `ListSecretsRequest$MaxResults` (`@range` 1..100).
const MAX_RESULTS_MIN: i64 = 1;
const MAX_RESULTS_MAX: i64 = 100;

/// A `ClientRequestToken` of the minimum modelled length (`@length` 32..64), unique per
/// `seed`: a write that stores a value stakes a version under the id the caller names.
fn token(seed: &str) -> String {
    let mut id = format!("t-{seed}-");
    while id.len() < 32 {
        id.push('z');
    }
    id.chars().take(32).collect()
}

/// A fresh emulator holding only this service, over its own engine.
fn emulator() -> MultiServiceRouter {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(EmulatedService::new(engine));
    router
}

/// Send one `awsJson1_1` request and return its status and JSON body.
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("secretsmanager.{operation}"))
        .header("authorization", AUTHORIZATION)
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("the request body is serializable"),
        ))
        .expect("a valid request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// Send one request that must succeed, and return its body.
async fn ok(router: &MultiServiceRouter, operation: &str, body: Value, case: &str) -> Value {
    let (status, parsed) = call(router, operation, &body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from {operation}, got {status} with body {parsed}"
    );
    parsed
}

/// Send one request that must be refused, and return its wire error code and message. The
/// status is asserted here: every client error of this service travels with 400.
async fn err(
    router: &MultiServiceRouter,
    operation: &str,
    body: Value,
    case: &str,
) -> (String, String) {
    let (status, parsed) = call(router, operation, &body).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400 from {operation}, got {status} with body {parsed}"
    );
    let raw = parsed
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: no __type in error body {parsed}"));
    let code = raw.split('#').next_back().unwrap_or(raw).to_string();
    let message = parsed
        .get("message")
        .or_else(|| parsed.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: no message in error body {parsed}"))
        .to_string();
    (code, message)
}

/// Assert the code and the message of one refusal.
fn refused(case: &str, answered: (String, String), code: &str, message: &str) {
    let (actual_code, actual_message) = answered;
    assert_eq!(
        actual_code, code,
        "case {case:?}: expected {code}, got {actual_code} with message {actual_message:?}"
    );
    assert_eq!(
        actual_message, message,
        "case {case:?}: expected {message:?}, got {actual_message:?}"
    );
}

/// Create a secret holding one value, and answer the version id it was staked under.
async fn create(router: &MultiServiceRouter, name: &str, case: &str) -> String {
    let id = token(name);
    ok(
        router,
        "CreateSecret",
        json!({"Name": name, "SecretString": "v", "ClientRequestToken": id.clone()}),
        case,
    )
    .await;
    id
}

/// AWS's one-violation envelope over a clause.
fn one_violation(clause: &str) -> String {
    format!("1 validation error detected: {clause}")
}

/// `n` repetitions of `unit`, as a value a row sends.
fn run(unit: &str, n: usize) -> String {
    unit.repeat(n)
}

#[tokio::test]
async fn a_name_the_service_cannot_use_is_one_constraint_refusal_with_one_sentence() {
    // "The secret name can contain ASCII letters, numbers, and the following characters:
    // /_+=.@-" (`CreateSecretRequest$Name`, this service's Smithy model), and `NameType`
    // bounds its length. A name outside EITHER half of that rule is one refusal with one
    // sentence — the rows run over both halves (a character the set excludes, from each
    // class the documentation leaves out; a length past the published maximum; the empty
    // string at the published minimum), and over the names the rule admits.
    let router = emulator();
    let accepted: &[(&str, String)] = &[
        ("a single character", "s".to_string()),
        (
            "every documented extra character",
            "a/b_c+d=e.f@g-h".to_string(),
        ),
        (
            "a name at the length the response reports",
            run("n", REPORTED_NAME_MAX),
        ),
    ];
    for (case, name) in accepted {
        let created = ok(
            &router,
            "CreateSecret",
            json!({"Name": name, "SecretString": "v", "ClientRequestToken": token(case)}),
            case,
        )
        .await;
        assert_eq!(
            created.get("Name").and_then(Value::as_str),
            Some(name.as_str()),
            "case {case:?}: expected the stored Name back, got {created}"
        );
    }

    let refusals: &[(&str, String)] = &[
        ("a space", "my secret".to_string()),
        ("a colon", "my:secret".to_string()),
        ("a percent sign", "my%secret".to_string()),
        ("a non-ascii letter", "Ünicorn".to_string()),
        ("an emoji", "secret🔐".to_string()),
        (
            "an ARN-shaped name",
            "arn:aws:secretsmanager:::secret:x".to_string(),
        ),
        ("the empty name", String::new()),
        ("one character past the maximum", run("n", NAME_MAX + 1)),
        ("far past the maximum", run("n", NAME_MAX * 2)),
    ];
    for (case, name) in refusals {
        let answered = err(
            &router,
            "CreateSecret",
            json!({"Name": name, "SecretString": "v", "ClientRequestToken": token("refused")}),
            case,
        )
        .await;
        refused(case, answered, CONSTRAINT, INVALID_SECRET_NAME_MESSAGE);
        // A refused name stored nothing to find. The empty name is not asked for: a
        // `SecretId` is itself bounded below by one character, so the read would be
        // refused for its own member rather than for finding nothing.
        if name.is_empty() {
            continue;
        }
        let (code, _) = err(
            &router,
            "DescribeSecret",
            json!({"SecretId": name}),
            case,
        )
        .await;
        assert_eq!(
            code, NOT_FOUND,
            "case {case:?}: a refused name must store nothing, got {code}"
        );
    }
}

#[tokio::test]
async fn a_write_states_exactly_one_secret_value() {
    // "Either SecretString or SecretBinary must have a value, but not both."
    // (`CreateSecretRequest$SecretString`, and the same text on the other two writes.) Both
    // together is a pair of parameter values that cannot hold; neither, on the write whose
    // whole purpose is to store one, is a request the secret's state does not admit. The
    // rows run over the operation and over the size and kind of each value.
    let router = emulator();
    create(&router, "both-values", "setup").await;
    let pairs: &[(&str, Value)] = &[
        ("short values", json!({"SecretString": "s", "SecretBinary": "AAECAwQ="})),
        (
            "a long string",
            json!({"SecretString": "z".repeat(4096), "SecretBinary": "AAECAwQ="}),
        ),
        (
            "a long blob",
            json!({"SecretString": "a", "SecretBinary": "AA".repeat(512)}),
        ),
    ];
    for (label, values) in pairs {
        for operation in ["CreateSecret", "PutSecretValue", "UpdateSecret"] {
            let case = format!("{operation} with {label}");
            let mut body = values.clone();
            let members = body.as_object_mut().expect("a request body is an object");
            if operation == "CreateSecret" {
                // A name over the documented charset, so the refusal under test is the one
                // the two value members cause rather than the name's own.
                members.insert(
                    "Name".to_string(),
                    json!(format!("both-{}", label.replace(' ', "-"))),
                );
            } else {
                members.insert("SecretId".to_string(), json!("both-values"));
            }
            members.insert("ClientRequestToken".to_string(), json!(token(&case)));
            let answered = err(&router, operation, body, &case).await;
            refused(&case, answered, PARAMETER, BOTH_SECRET_VALUES_MESSAGE);
        }
    }

    // A put that carries no value at all names nothing to store.
    let answered = err(
        &router,
        "PutSecretValue",
        json!({"SecretId": "both-values", "ClientRequestToken": token("valueless")}),
        "PutSecretValue with neither value member",
    )
    .await;
    refused(
        "PutSecretValue with neither value member",
        answered,
        STATE,
        NO_SECRET_VALUE_MESSAGE,
    );
}

#[tokio::test]
async fn the_tagging_quota_is_refused_for_the_secrets_state() {
    // The AWS tagging quota is 50 tags per resource ("Service quotas for Tagging", Amazon
    // Web Services General Reference, referenced from `TagResource`'s own documentation in
    // this service's Smithy model). A write that would leave a secret past it is refused
    // for what the secret holds, with one sentence. The rows run over the operation and
    // over where the overrun comes from: one request carrying more than the quota, and one
    // more tag on a secret already at it.
    let router = emulator();
    create(&router, "quota", "setup").await;
    let tags = |count: usize| -> Value {
        (0..count)
            .map(|index| json!({"Key": format!("k{index}"), "Value": "v"}))
            .collect::<Vec<Value>>()
            .into()
    };
    // Exactly the quota is accepted, in one request.
    ok(
        &router,
        "TagResource",
        json!({"SecretId": "quota", "Tags": tags(TAGS_PER_SECRET)}),
        "exactly the quota",
    )
    .await;
    // A key already attached is replaced rather than added, so re-tagging at the quota is
    // still accepted.
    ok(
        &router,
        "TagResource",
        json!({"SecretId": "quota", "Tags": [{"Key": "k0", "Value": "replaced"}]}),
        "re-tagging at the quota",
    )
    .await;

    let rows: &[(&str, &str, Value)] = &[
        (
            "one more tag on a secret at the quota",
            "TagResource",
            json!({"SecretId": "quota", "Tags": [{"Key": "past", "Value": "v"}]}),
        ),
        (
            "one request past the quota",
            "TagResource",
            json!({"SecretId": "quota", "Tags": tags(TAGS_PER_SECRET + 1)}),
        ),
        (
            "more tag keys removed than a secret may carry",
            "UntagResource",
            json!({
                "SecretId": "quota",
                "TagKeys": (0..=TAGS_PER_SECRET)
                    .map(|index| json!(format!("k{index}")))
                    .collect::<Vec<Value>>(),
            }),
        ),
        (
            "a create past the quota",
            "CreateSecret",
            json!({"Name": "over-tagged", "Tags": tags(TAGS_PER_SECRET + 1)}),
        ),
    ];
    for (case, operation, body) in rows {
        let answered = err(&router, operation, body.clone(), case).await;
        refused(case, answered, STATE, TOO_MANY_TAGS_MESSAGE);
    }
}

#[tokio::test]
async fn a_tag_in_the_reserved_namespace_is_refused_naming_the_key() {
    // The `aws:` prefix is AWS's own namespace: a caller may neither write nor remove a tag
    // under it. The sentence is fixed apart from the key the request named, which is the
    // one thing that tells a caller which of its keys was the reserved one. The rows run
    // over the shape of the key and over the operation that carried it.
    let router = emulator();
    create(&router, "reserved", "setup").await;
    for key in [
        "aws:reserved",
        "aws:owner",
        "aws:cloudformation:stack-name",
        "aws:",
    ] {
        let rows: &[(&str, Value)] = &[
            (
                "TagResource",
                json!({"SecretId": "reserved", "Tags": [{"Key": key, "Value": "v"}]}),
            ),
            (
                "UntagResource",
                json!({"SecretId": "reserved", "TagKeys": [key]}),
            ),
            (
                "CreateSecret",
                json!({"Name": format!("reserved-{}", key.len()), "Tags": [{"Key": key, "Value": "v"}]}),
            ),
        ];
        for (operation, body) in rows {
            let case = format!("{operation} with the key {key:?}");
            let answered = err(&router, operation, body.clone(), &case).await;
            refused(&case, answered, PARAMETER, &system_tag_message(key));
        }
    }
}

#[tokio::test]
async fn a_read_whose_two_selectors_name_two_versions_is_refused_for_the_state() {
    // "If you include both this parameter and VersionId, the two parameters must refer to
    // the same secret version." (`GetSecretValueRequest$VersionStage`, this service's
    // Smithy model.) When each selector resolves and the two resolve to DIFFERENT versions
    // the request contradicts itself and is refused for the state of the secret; when one
    // of them resolves to nothing, the read found no value and says which member it
    // resolved by. The rows run over which selector names which version, over both
    // reserved staging labels and one attached to nothing, and over a version id the
    // secret does not hold.
    let router = emulator();
    let name = "selectors";
    let first = create(&router, name, "setup").await;
    let second = token("second");
    ok(
        &router,
        "PutSecretValue",
        json!({"SecretId": name, "SecretString": "v2", "ClientRequestToken": second.clone()}),
        "setup",
    )
    .await;
    let absent = token("absent");

    // The two selectors agree: the read is served.
    for (case, version, stage) in [
        ("the current version by both selectors", &second, "AWSCURRENT"),
        ("the previous version by both selectors", &first, "AWSPREVIOUS"),
    ] {
        let answered = ok(
            &router,
            "GetSecretValue",
            json!({"SecretId": name, "VersionId": version, "VersionStage": stage}),
            case,
        )
        .await;
        assert_eq!(
            answered.get("VersionId").and_then(Value::as_str),
            Some(version.as_str()),
            "case {case:?}: expected the named version back, got {answered}"
        );
    }

    // They disagree: both resolve, to two different versions.
    for (case, version, stage) in [
        ("the previous version under AWSCURRENT", &first, "AWSCURRENT"),
        ("the current version under AWSPREVIOUS", &second, "AWSPREVIOUS"),
    ] {
        let answered = err(
            &router,
            "GetSecretValue",
            json!({"SecretId": name, "VersionId": version, "VersionStage": stage}),
            case,
        )
        .await;
        refused(case, answered, STATE, VERSION_ID_AND_STAGE_DISAGREE_MESSAGE);
    }

    // One of them resolves to nothing: the read is a miss, named by the version id it
    // resolved by.
    for (case, version, stage) in [
        ("a staging label no version carries", &first, "AWSPENDING"),
        ("a version id the secret does not hold", &absent, "AWSCURRENT"),
    ] {
        let answered = err(
            &router,
            "GetSecretValue",
            json!({"SecretId": name, "VersionId": version, "VersionStage": stage}),
            case,
        )
        .await;
        refused(
            case,
            answered,
            NOT_FOUND,
            &value_version_not_found_message(version),
        );
    }
}

#[tokio::test]
async fn rotation_rules_that_cannot_hold_are_refused_as_a_parameter() {
    // "The window starts according to the ScheduleExpression."
    // (`RotationRulesType$Duration`, this service's Smithy model.) A duration is the length
    // of the window a scheduled rotation happens in, so rules naming a duration and no
    // schedule describe a window with no start: the parameter is refused before the
    // secret's own state is consulted at all — the rows that DO name a schedule get past
    // the rules and are refused for the secret's state instead, which is what shows the
    // refusal belongs to the rules rather than to the duration being present.
    let router = emulator();
    create(&router, "rules", "setup").await;
    let without_schedule: &[(&str, Value)] = &[
        ("a two-hour window", json!({"Duration": "2h"})),
        ("a three-hour window", json!({"Duration": "3h"})),
        (
            "a window at the longest modelled duration",
            json!({"Duration": "12h"}),
        ),
    ];
    for (case, rules) in without_schedule {
        let answered = err(
            &router,
            "RotateSecret",
            json!({"SecretId": "rules", "RotationRules": rules}),
            case,
        )
        .await;
        refused(case, answered, PARAMETER, DURATION_WITHOUT_SCHEDULE);
    }

    let with_schedule: &[(&str, Value)] = &[
        (
            "a duration beside a rate schedule",
            json!({"Duration": "2h", "ScheduleExpression": "rate(10 days)"}),
        ),
        (
            "a duration beside a cron schedule",
            json!({"Duration": "3h", "ScheduleExpression": "cron(0 8 1 * ? *)"}),
        ),
        (
            "a duration beside a rotation frequency",
            json!({"Duration": "2h", "AutomaticallyAfterDays": 30}),
        ),
        (
            "a rotation frequency alone",
            json!({"AutomaticallyAfterDays": 30}),
        ),
    ];
    for (case, rules) in with_schedule {
        let answered = err(
            &router,
            "RotateSecret",
            json!({"SecretId": "rules", "RotationRules": rules}),
            case,
        )
        .await;
        refused(case, answered, STATE, NO_ROTATION_FUNCTION_MESSAGE);
    }
}

#[tokio::test]
async fn a_name_inside_a_recovery_window_is_refused_without_naming_the_secret() {
    // A secret scheduled for deletion still holds its name, so a create addressed to that
    // name is refused for that secret's state — with a sentence that states a secret with
    // this name is already scheduled for deletion and reflects no identifier of the
    // caller's back into the error body. The rows run over the name, since the sentence is
    // the same for every one of them.
    let router = emulator();
    for (case, name) in [
        ("a short name", "gone"),
        ("a name with every documented extra character", "a/b_c+d=e.f@g-h"),
        ("a long name", "deleted-secret-with-a-considerably-longer-name"),
    ] {
        create(&router, name, case).await;
        ok(
            &router,
            "DeleteSecret",
            json!({"SecretId": name}),
            case,
        )
        .await;
        let answered = err(
            &router,
            "CreateSecret",
            json!({"Name": name, "SecretString": "v", "ClientRequestToken": token("reuse")}),
            case,
        )
        .await;
        assert!(
            !answered.1.contains(name),
            "case {case:?}: the refusal must not name the secret, got {:?}",
            answered.1
        );
        refused(case, answered, STATE, NAME_SCHEDULED_FOR_DELETION_MESSAGE);
    }
}

#[tokio::test]
async fn a_tag_list_that_names_one_key_twice_is_refused() {
    // "Each tag is a key and value pair", and "Secrets Manager tag key names are case
    // sensitive. A tag with the key "ABC" is a different tag from one with key "abc"."
    // (`CreateSecretRequest$Tags`, this service's Smithy model.) A key identifies one tag,
    // so a list repeating one asks for two values for one tag; keys differing only in case
    // are two tags and are accepted. The rows run over where the repeat sits in the list,
    // how many times it repeats, whether the repeated entries carry the same value, and the
    // operation that carried them.
    let router = emulator();
    create(&router, "dupkey", "setup").await;
    let rows: &[(&str, Value)] = &[
        (
            "the same key twice with different values",
            json!([{"Key": "env", "Value": "a"}, {"Key": "env", "Value": "b"}]),
        ),
        (
            "the same key twice with the same value",
            json!([{"Key": "env", "Value": "a"}, {"Key": "env", "Value": "a"}]),
        ),
        (
            "a repeat separated by another key",
            json!([
                {"Key": "team", "Value": "a"},
                {"Key": "env", "Value": "b"},
                {"Key": "team", "Value": "c"},
            ]),
        ),
        (
            "a key three times",
            json!([
                {"Key": "cost", "Value": "a"},
                {"Key": "cost", "Value": "b"},
                {"Key": "cost", "Value": "c"},
            ]),
        ),
    ];
    for (label, tags) in rows {
        for operation in ["TagResource", "CreateSecret"] {
            let case = format!("{operation} with {label}");
            let body = if operation == "TagResource" {
                json!({"SecretId": "dupkey", "Tags": tags})
            } else {
                json!({"Name": format!("dup-{}", label.replace(' ', "-")), "Tags": tags})
            };
            let answered = err(&router, operation, body, &case).await;
            refused(&case, answered, PARAMETER, DUPLICATE_TAG_KEYS_MESSAGE);
        }
    }

    // Keys that differ only in case are two different tags, so both are attached.
    ok(
        &router,
        "TagResource",
        json!({
            "SecretId": "dupkey",
            "Tags": [{"Key": "ABC", "Value": "upper"}, {"Key": "abc", "Value": "lower"}],
        }),
        "keys differing only in case",
    )
    .await;
    let described = ok(
        &router,
        "DescribeSecret",
        json!({"SecretId": "dupkey"}),
        "after the refusals",
    )
    .await;
    let tags = described
        .get("Tags")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case \"after the refusals\": no Tags in {described}"));
    assert_eq!(
        tags.len(),
        2,
        "case \"after the refusals\": only the two case-distinct tags are attached, got \
         {described}"
    );
}

#[tokio::test]
async fn a_constraint_refusal_states_the_member_and_bounds_what_it_echoes() {
    // The envelope, the member path and the requirement's wording are the framework's; what
    // this service chooses is how much of the rejected value a clause may reproduce, and how
    // a pattern and an enumerated set are stated. The rows run over the constraint class
    // (`@length` on a string and on a blob, `@pattern`, `@range`, an enum value set),
    // whether the member is `@sensitive`, and the size of the value relative to the echo
    // bound.
    let router = emulator();
    create(&router, "clauses", "setup").await;
    // The admitted filter keys are `FilterNameStringType`'s: the generated
    // `FILTER_NAME_STRING_TYPE_VALUES` holds them sorted and `STATED_FILTER_NAME_VALUE_SET`
    // in the order a clause states them in — the same set either way.
    let sorted = |values: &[&str]| {
        let mut values: Vec<String> = values.iter().map(|value| (*value).to_string()).collect();
        values.sort();
        values
    };
    assert_eq!(
        sorted(&STATED_FILTER_NAME_VALUE_SET),
        sorted(FILTER_NAME_STRING_TYPE_VALUES),
        "case \"the stated filter keys\": the stated set must hold exactly the model's values"
    );
    let filter_key_set = STATED_FILTER_NAME_VALUE_SET.join(", ");

    let rows: Vec<(String, &str, Value, String)> = vec![
        (
            "a tag key far past its maximum".to_string(),
            "TagResource",
            json!({
                "SecretId": "clauses",
                "Tags": [{"Key": run("k", TAG_KEY_MAX + 64), "Value": "v"}],
            }),
            one_violation(&format!(
                "Value '{}{ELISION}' at 'tags.1.member.key' failed to satisfy constraint: \
                 Member must have length less than or equal to {TAG_KEY_MAX}",
                run("k", ECHO_BOUND)
            )),
        ),
        (
            "a tag key one character past its maximum".to_string(),
            "TagResource",
            json!({
                "SecretId": "clauses",
                "Tags": [{"Key": run("j", TAG_KEY_MAX + 1), "Value": "v"}],
            }),
            one_violation(&format!(
                "Value '{}{ELISION}' at 'tags.1.member.key' failed to satisfy constraint: \
                 Member must have length less than or equal to {TAG_KEY_MAX}",
                run("j", ECHO_BOUND)
            )),
        ),
        (
            "a sensitive string past its maximum".to_string(),
            "PutSecretValue",
            json!({
                "SecretId": "clauses",
                "SecretString": run("s", SECRET_VALUE_MAX + 1),
                "ClientRequestToken": token("too-long"),
            }),
            one_violation(&format!(
                "Value at 'secretString' failed to satisfy constraint: Member must have length \
                 less than or equal to {SECRET_VALUE_MAX}"
            )),
        ),
        (
            "a sensitive string below its minimum".to_string(),
            "PutSecretValue",
            json!({
                "SecretId": "clauses",
                "SecretString": "",
                "ClientRequestToken": token("empty"),
            }),
            one_violation(
                "Value at 'secretString' failed to satisfy constraint: Member must have length \
                 greater than or equal to 1",
            ),
        ),
        (
            "a sensitive blob below its minimum".to_string(),
            "PutSecretValue",
            json!({
                "SecretId": "clauses",
                "SecretBinary": "",
                "ClientRequestToken": token("empty-blob"),
            }),
            one_violation(
                "Value at 'secretBinary' failed to satisfy constraint: Member must have length \
                 greater than or equal to 1",
            ),
        ),
        (
            "a value outside a pattern, inside the echo bound".to_string(),
            "RotateSecret",
            json!({"SecretId": "clauses", "RotationRules": {"Duration": "1x"}}),
            one_violation(
                "Value '1x' at 'rotationRules.duration' failed to satisfy constraint: Member \
                 must satisfy regular expression pattern: [0-9]+h",
            ),
        ),
        (
            "a filter key outside its enumerated set".to_string(),
            "ListSecrets",
            json!({"Filters": [{"Key": "secret-name", "Values": ["x"]}]}),
            one_violation(&format!(
                "Value 'secret-name' at 'filters.1.member.key' failed to satisfy constraint: \
                 Member must satisfy enum value set: [{filter_key_set}]"
            )),
        ),
        (
            "a filter key outside the set, at a later position".to_string(),
            "ListSecrets",
            json!({
                "Filters": [
                    {"Key": "name", "Values": ["x"]},
                    {"Key": "Name", "Values": ["x"]},
                ],
            }),
            one_violation(&format!(
                "Value 'Name' at 'filters.2.member.key' failed to satisfy constraint: Member \
                 must satisfy enum value set: [{filter_key_set}]"
            )),
        ),
        (
            "a page size below its range".to_string(),
            "ListSecrets",
            json!({"MaxResults": MAX_RESULTS_MIN - 1}),
            one_violation(&format!(
                "Value '{}' at 'maxResults' failed to satisfy constraint: Member must have value \
                 greater than or equal to {MAX_RESULTS_MIN}",
                MAX_RESULTS_MIN - 1
            )),
        ),
        (
            "a page size above its range".to_string(),
            "ListSecrets",
            json!({"MaxResults": MAX_RESULTS_MAX + 1}),
            one_violation(&format!(
                "Value '{}' at 'maxResults' failed to satisfy constraint: Member must have value \
                 less than or equal to {MAX_RESULTS_MAX}",
                MAX_RESULTS_MAX + 1
            )),
        ),
    ];
    for (case, operation, body, expected) in rows {
        let answered = err(&router, operation, body, &case).await;
        refused(&case, answered, CONSTRAINT, &expected);
    }

    // The elided clause carries no more of the value than the bound allows, whatever the
    // caller sent: the whole key never reaches the response body.
    let key = run("w", TAG_KEY_MAX * 8);
    let (_, message) = err(
        &router,
        "TagResource",
        json!({"SecretId": "clauses", "Tags": [{"Key": key.clone(), "Value": "v"}]}),
        "a tag key many times its maximum",
    )
    .await;
    assert!(
        !message.contains(&run("w", ECHO_BOUND + 1)),
        "case \"a tag key many times its maximum\": the clause must echo at most \
         {ECHO_BOUND} bytes of the key, got {message:?}"
    );
}
