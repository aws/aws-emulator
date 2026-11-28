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
//! 4. A constraint one of a LIST member's ELEMENTS broke is stated once at that list member,
//!    quoting the whole list the member held and nesting the element shape's WHOLE declared
//!    constraint set — the expression of a `@pattern` inside such a set as the model writes
//!    it — so two offending elements of one list are one validation error about one member.
//! 5. A rule stated over a COMBINATION of request members, rather than over one member's own
//!    value, states what change the request asks of the secret it addresses, so the secret is
//!    resolved first: a `SecretId` the Region holds no secret for is `ResourceNotFoundException`
//!    whatever those members say, and the combination rule decides only for a secret that
//!    exists.
//!
//! The rows differ along what each rule generalises over: the operation and member that
//! carried the value, the constraint class (`@length` on a string and on a blob, `@pattern`,
//! `@range`, an enum value set), whether the member is `@sensitive`, the size of the value
//! (inside the echo bound, one byte past it, and far past it), how many tags the secret
//! already holds, which of the two rotation-schedule forms the rules name, and which
//! version each selector of a read resolves to.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_secretsmanager_secretsmanager::ops::types::{
    FILTER_NAME_STRING_TYPE_VALUES, SORT_ORDER_TYPE_VALUES,
};
use service_com_amazonaws_secretsmanager_secretsmanager::state::{
    invalid_filter_value_message, system_tag_message, too_many_filter_values_message,
    value_version_not_found_message, BOTH_SECRET_VALUES_MESSAGE, DUPLICATE_TAG_KEYS_MESSAGE,
    DURATION_WITHOUT_SCHEDULE, FILTER_VALUES_PER_FILTER, INVALID_NEXT_TOKEN_MESSAGE,
    INVALID_ROTATION_TOKEN_MESSAGE, INVALID_SECRET_NAME_MESSAGE,
    NAME_SCHEDULED_FOR_DELETION_MESSAGE, NO_ROTATION_FUNCTION_MESSAGE, NO_SECRET_VALUE_MESSAGE,
    ROTATION_TOKEN_CHARS, SECRET_ID_SIZE_LIMIT, SECRET_ID_TOO_LONG_MESSAGE, SECRET_NAME_CHARS,
    STATED_FILTER_NAME_VALUE_SET, STATED_SORT_ORDER_VALUE_SET, TAGS_PER_SECRET,
    TOO_MANY_TAGS_MESSAGE, VERSION_ID_AND_STAGE_DISAGREE_MESSAGE,
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

/// The modelled code for a pagination token the service cannot resume from.
const INVALID_TOKEN: &str = "InvalidNextTokenException";

/// The published maximum of `NextTokenType` (`@length` 1..4096, this service's Smithy
/// model): the longest pagination token any paginated member carries.
const NEXT_TOKEN_MAX: usize = 4096;

/// The published maximum of `SecretIdType` (`@length` 1..2048, this service's Smithy
/// model), which is also the longest entry `SecretIdListType` admits.
const SECRET_ID_LIST_ENTRY_MAX: usize = 2048;

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
    // Smithy model.) When the `VersionId` resolves and the staging label beside it is not
    // one that version carries, the request contradicts itself and is refused for the state
    // of the secret; when the `VersionId` itself resolves to nothing, the read found no
    // value and says which member it resolved by. The rows run over which selector names
    // which version, over both reserved staging labels and one attached to nothing, and
    // over a version id the secret does not hold.
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

    // They disagree: the version id resolves, and the label is not one that version
    // carries — whether another version carries it or no version does.
    for (case, version, stage) in [
        ("the previous version under AWSCURRENT", &first, "AWSCURRENT"),
        ("the current version under AWSPREVIOUS", &second, "AWSPREVIOUS"),
        ("a staging label no version carries", &first, "AWSPENDING"),
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

    // The version id resolves to nothing: the read is a miss, named by the version id it
    // resolved by.
    for (case, version, stage) in [("a version id the secret does not hold", &absent, "AWSCURRENT")]
    {
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

/// What every element of a `Filter`'s `Values` has to satisfy, as the clause stated at that
/// member nests it: the whole set the element shape `FilterValueStringType` declares
/// (`@length` 0..512 and that `@pattern`;
/// `smithy_query(query="shape",shape="FilterValueStringType")`), with the expression as the
/// MODEL writes it — anchors included, which is the rendering a nested set carries.
const FILTER_VALUE_SET: &str = "Member must satisfy constraint: [Member must have length less \
     than or equal to 512, Member must have length greater than or equal to 0, Member must \
     satisfy regular expression pattern: ^\\!?[a-zA-Z0-9 :_@\\/\\+\\=\\.\\-\\!]*$]";

/// The same for `SecretIdListType`'s element shape `SecretIdType` (`@length` 1..2048, no
/// pattern; `smithy_query(query="shape",shape="SecretIdListType")`).
const SECRET_ID_SET: &str = "Member must satisfy constraint: [Member must have length less than \
     or equal to 2048, Member must have length greater than or equal to 1]";

/// The same for `SecretVersionStagesType`'s element shape `SecretVersionStageType`
/// (`@length` 1..256; `smithy_query(query="shape",shape="SecretVersionStagesType")`).
const VERSION_STAGE_SET: &str = "Member must satisfy constraint: [Member must have length less \
     than or equal to 256, Member must have length greater than or equal to 1]";

/// The published maximum of `FilterValueStringType` (`@length` 0..512).
const FILTER_VALUE_MAX: usize = 512;

/// The published maximum of `SecretVersionStageType` (`@length` 1..256).
const VERSION_STAGE_MAX: usize = 256;

/// The clause a constraint broken by one of `member`'s elements is stated with: the member
/// itself, the whole list the request carried there — held to what a clause of this service
/// echoes ([`echoed_list`]) — and the element shape's whole set.
fn element_clause(member: &str, held: &[&str], set: &str) -> String {
    format!(
        "Value '{}' at '{member}' failed to satisfy constraint: {set}",
        echoed_list(held)
    )
}

/// `held` as a clause of this service quotes the list it was carried in: the rendered list,
/// cut to [`ECHO_BOUND`] characters followed by [`ELISION`] when it is longer.
fn echoed_list(held: &[&str]) -> String {
    echoed_value(&format!("[{}]", held.join(", ")))
}

/// `rendered` as a clause of this service quotes a value: as sent while it is inside
/// [`ECHO_BOUND`] characters, and that bounded prefix followed by [`ELISION`] when it is
/// longer.
fn echoed_value(rendered: &str) -> String {
    if rendered.chars().count() <= ECHO_BOUND {
        return rendered.to_string();
    }
    let prefix: String = rendered.chars().take(ECHO_BOUND).collect();
    format!("{prefix}{ELISION}")
}

#[tokio::test]
async fn a_constraint_one_of_a_lists_elements_broke_is_stated_at_that_list() {
    // A constraint declared on a list's ELEMENT shape is checked element by element, and the
    // refusal states it once at the LIST member: the value echoed is the whole list the
    // member held, and the requirement is the element shape's WHOLE declared set rather
    // than the one constraint that happened to fail — so a caller is told what every element
    // of that member has to satisfy, and two offending elements of one list are ONE
    // validation error about one member.
    //
    // The rows differ along which member carried the list (three operations, three element
    // shapes), which end of `@length` the element broke, how many elements the list held,
    // which position offended, how deep the list sits (a list of its own, and one inside a
    // structure inside a list), and whether the rendered list is inside the echo bound or
    // past it.
    let router = emulator();
    create(&router, "clauses/elements", "setup").await;
    let over_long_value = run("x", FILTER_VALUE_MAX + 1);
    let over_long_stage = run("v", VERSION_STAGE_MAX + 1);
    let rows: Vec<(String, &str, Value, String)> = vec![
        (
            "a value one character past the element maximum".to_string(),
            "ListSecrets",
            json!({"Filters": [{"Key": "name", "Values": [over_long_value.clone()]}]}),
            element_clause(
                "filters.1.member.values",
                &[over_long_value.as_str()],
                FILTER_VALUE_SET,
            ),
        ),
        (
            "a value past the maximum beside one inside it".to_string(),
            "ListSecrets",
            json!({"Filters": [{"Key": "name", "Values": ["fine", over_long_value.clone()]}]}),
            element_clause(
                "filters.1.member.values",
                &["fine", over_long_value.as_str()],
                FILTER_VALUE_SET,
            ),
        ),
        (
            "a value of the second filter in the list".to_string(),
            "ListSecrets",
            json!({
                "Filters": [
                    {"Key": "name", "Values": ["ok"]},
                    {"Key": "description", "Values": [over_long_value.clone()]},
                ],
            }),
            element_clause(
                "filters.2.member.values",
                &[over_long_value.as_str()],
                FILTER_VALUE_SET,
            ),
        ),
        (
            "a filter value of the batched read".to_string(),
            "BatchGetSecretValue",
            json!({"Filters": [{"Key": "name", "Values": [over_long_value.clone()]}]}),
            element_clause(
                "filters.1.member.values",
                &[over_long_value.as_str()],
                FILTER_VALUE_SET,
            ),
        ),
        (
            "an id below the element minimum, in a list of its own".to_string(),
            "BatchGetSecretValue",
            json!({"SecretIdList": ["clauses/elements", ""]}),
            element_clause("secretIdList", &["clauses/elements", ""], SECRET_ID_SET),
        ),
        (
            "a staging label past the element maximum".to_string(),
            "PutSecretValue",
            json!({
                "SecretId": "clauses/elements",
                "SecretString": "v",
                "ClientRequestToken": token("stages"),
                "VersionStages": [over_long_stage.clone()],
            }),
            element_clause(
                "versionStages",
                &[over_long_stage.as_str()],
                VERSION_STAGE_SET,
            ),
        ),
    ];
    for (case, operation, body, clause) in rows {
        let answered = err(&router, operation, body, &case).await;
        refused(&case, answered, CONSTRAINT, &one_violation(&clause));
    }

    // A member that is not a list element keeps its own clause, so a request breaking both
    // is two validation errors — the grouped one and the member's own.
    let case = "a list element beside a member of its own";
    let (_, message) = err(
        &router,
        "ListSecrets",
        json!({
            "MaxResults": MAX_RESULTS_MAX + 1,
            "Filters": [{"Key": "name", "Values": [over_long_value.clone()]}],
        }),
        case,
    )
    .await;
    let grouped = element_clause(
        "filters.1.member.values",
        &[over_long_value.as_str()],
        FILTER_VALUE_SET,
    );
    let scalar = format!(
        "Value '{}' at 'maxResults' failed to satisfy constraint: Member must have value less \
         than or equal to {MAX_RESULTS_MAX}",
        MAX_RESULTS_MAX + 1
    );
    assert!(
        message.starts_with("2 validation errors detected: "),
        "case {case:?}: expected two counted violations, got {message:?}"
    );
    for expected in [&grouped, &scalar] {
        assert!(
            message.contains(expected),
            "case {case:?}: expected the message to carry {expected:?}, got {message:?}"
        );
    }
}

#[tokio::test]
async fn a_rotation_duration_with_nothing_to_start_it_names_both_members_that_would() {
    // Rules naming a duration and no schedule describe a window with no start, and the
    // refusal states the rule: a schedule expression OR a rotation frequency satisfies it,
    // so the sentence names both rather than only one of them. Fixed text, with nothing of
    // the request interpolated — the rows run over the duration the rules named and over the
    // secret the call addressed, since neither enters the sentence.
    const SENTENCE: &str = "You must provide a schedule expression or rotation frequency when \
                            using the duration option.";
    let router = emulator();
    assert_eq!(
        DURATION_WITHOUT_SCHEDULE, SENTENCE,
        "the sentence every operation raising this refusal answers with must be AWS's"
    );
    for (case, name) in [
        ("a secret with no rotation configured", "duration/plain"),
        ("a secret named by a longer name", "duration/another-secret"),
    ] {
        create(&router, name, case).await;
        for duration in ["1h", "2h", "12h", "99h"] {
            let case = format!("{case}, a window of {duration}");
            let answered = err(
                &router,
                "RotateSecret",
                json!({"SecretId": name, "RotationRules": {"Duration": duration}}),
                &case,
            )
            .await;
            assert!(
                !answered.1.contains(duration) && !answered.1.contains(name),
                "case {case:?}: the sentence states the rule and nothing of the request, got \
                 {:?}",
                answered.1
            );
            refused(&case, answered, PARAMETER, SENTENCE);
        }
    }
}

/// The published maximum of `MaxResultsBatchType`, the shape
/// `BatchGetSecretValueRequest$MaxResults` targets (`@range` 1..20, this service's Smithy
/// model): the page size one batched read answers with.
const BATCH_MAX_RESULTS: i64 = 20;

/// The published maximum of `SecretIdListType` (`@length` 1..20), the other way that same
/// batch is named.
const BATCH_IDS_MAX: usize = 20;

/// The published maximum of `NonEmptyResourcePolicyType` (`@length` 1..20480) — the shape
/// both `PutResourcePolicyRequest$ResourcePolicy` and
/// `ValidateResourcePolicyRequest$ResourcePolicy` target, so it is the documented size
/// limit on a resource policy document whichever of the two carries it.
const RESOURCE_POLICY_MAX: usize = 20480;

/// A valid resource policy document exactly `length` characters long, padded inside its
/// `Sid` so the document stays a policy document at every length.
fn policy_of_length(length: usize) -> String {
    // A resource policy names an identity that EXISTS, which here means one of the
    // emulator's own account (`crate::secret_policy_validation` states the same rule).
    let principal = format!("arn:aws:iam::{}:root", ::engine::emulator_account_id());
    let document = |sid: &str| {
        format!(
            "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Sid\":\"{sid}\",\
             \"Effect\":\"Allow\",\"Principal\":{{\"AWS\":\"{principal}\"}},\
             \"Action\":\"secretsmanager:GetSecretValue\",\"Resource\":\"*\"}}]}}"
        )
    };
    let padded = document(&run("X", length.saturating_sub(document("").len())));
    assert_eq!(
        padded.len(),
        length,
        "case \"a policy document of {length} characters\": built {} instead",
        padded.len()
    );
    padded
}

/// The `ResourcePolicy` a secret reads back, as the document text.
async fn attached_policy(router: &MultiServiceRouter, name: &str, case: &str) -> String {
    let read = ok(router, "GetResourcePolicy", json!({"SecretId": name}), case).await;
    read.get("ResourcePolicy")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: no ResourcePolicy in {read}"))
        .to_string()
}

/// The `Sid` of the first statement of a policy document, which is what a row pads to
/// reach a length: the attached document is re-rendered on read-back, so the statement is
/// compared rather than the text.
fn first_sid(document: &str, case: &str) -> String {
    let parsed: Value = serde_json::from_str(document)
        .unwrap_or_else(|_| panic!("case {case:?}: the policy document is JSON"));
    parsed["Statement"][0]["Sid"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no Sid in {parsed}"))
        .to_string()
}

#[tokio::test]
async fn a_limit_this_service_enforces_itself_states_that_operations_own_refusal() {
    // Secrets Manager enforces some of its documented parameter limits in the service
    // itself: such a request is refused with one of THAT operation's modelled errors and
    // the sentence stating the limit — a client branching on the modelled code to page
    // smaller or to split a document sees it — and the sentence carries nothing of what the
    // caller sent, where the shared constraint clause both travels under a code the
    // operation does not model and quotes the value back.
    //
    // The rows run over the two limits (a page size and a document size), over both
    // members that carry a policy document, over how far past the limit the value is, and
    // over which of the batched read's two selectors is over its bound.
    let router = emulator();
    create(&router, "limits", "the limits table").await;
    let page_size_sentence = format!("Max results must not exceed {BATCH_MAX_RESULTS}.");
    let id_list_sentence = format!(
        "The secret ID list has too many IDs. You can include up to {BATCH_IDS_MAX} secret IDs."
    );
    const POLICY_SIZE_SENTENCE: &str = "ResourcePolicy exceeds size limit.";
    let name_filter = json!([{"Key": "name", "Values": ["limits"]}]);
    let over_long_ids: Vec<String> = (0..=BATCH_IDS_MAX)
        .map(|index| format!("limits-{index}"))
        .collect();
    let over_long_policy = policy_of_length(RESOURCE_POLICY_MAX + 1);
    let far_over_policy = policy_of_length(RESOURCE_POLICY_MAX * 2);

    // Each row also names a fragment of what it sent that the refusal must not carry.
    let rows: Vec<(String, &str, Value, &str, String, String)> = vec![
        (
            "a page size one past the batched read's ceiling".to_string(),
            "BatchGetSecretValue",
            json!({"Filters": name_filter.clone(), "MaxResults": BATCH_MAX_RESULTS + 1}),
            STATE,
            page_size_sentence.clone(),
            (BATCH_MAX_RESULTS + 1).to_string(),
        ),
        (
            "a page size far past that ceiling".to_string(),
            "BatchGetSecretValue",
            json!({"Filters": name_filter.clone(), "MaxResults": BATCH_MAX_RESULTS * 5}),
            STATE,
            page_size_sentence.clone(),
            (BATCH_MAX_RESULTS * 5).to_string(),
        ),
        (
            "an id list one past one batch".to_string(),
            "BatchGetSecretValue",
            json!({"SecretIdList": over_long_ids}),
            PARAMETER,
            id_list_sentence,
            "limits-".to_string(),
        ),
        (
            "a resource policy one character past the size limit".to_string(),
            "PutResourcePolicy",
            json!({"SecretId": "limits", "ResourcePolicy": over_long_policy.clone()}),
            PARAMETER,
            POLICY_SIZE_SENTENCE.to_string(),
            "XXXXXXXX".to_string(),
        ),
        (
            "a resource policy far past the size limit".to_string(),
            "PutResourcePolicy",
            json!({"SecretId": "limits", "ResourcePolicy": far_over_policy}),
            PARAMETER,
            POLICY_SIZE_SENTENCE.to_string(),
            "XXXXXXXX".to_string(),
        ),
        (
            "a policy validated one character past the size limit".to_string(),
            "ValidateResourcePolicy",
            json!({"SecretId": "limits", "ResourcePolicy": over_long_policy.clone()}),
            PARAMETER,
            POLICY_SIZE_SENTENCE.to_string(),
            "XXXXXXXX".to_string(),
        ),
    ];
    for (case, operation, body, code, message, echoed) in rows {
        let answered = err(&router, operation, body, &case).await;
        assert!(
            !answered.1.contains("validation error") && !answered.1.contains(echoed.as_str()),
            "case {case:?}: expected the service's own sentence, carrying no part of the \
             request, got {:?}",
            answered.1
        );
        refused(&case, answered, code, &message);
    }

    // The limits THEMSELVES are served: it is being past them that is refused. The page
    // size rows run over both published ends of the batched read's range.
    for page_size in [1, BATCH_MAX_RESULTS] {
        let case = format!("a page size of {page_size}, inside the ceiling");
        let read = ok(
            &router,
            "BatchGetSecretValue",
            json!({"Filters": name_filter.clone(), "MaxResults": page_size}),
            &case,
        )
        .await;
        let served = read
            .get("SecretValues")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case:?}: no SecretValues in {read}"))
            .len();
        assert_eq!(
            served, 1,
            "case {case:?}: expected the one selected secret to be served, got {read}"
        );
    }
    let case = "an id list at the bound of one batch";
    let mut ids: Vec<String> = vec!["limits".to_string()];
    ids.extend((1..BATCH_IDS_MAX).map(|index| format!("limits-{index}")));
    ok(
        &router,
        "BatchGetSecretValue",
        json!({"SecretIdList": ids}),
        case,
    )
    .await;

    // A document AT the size limit is attached and reads back, and the refused oversized
    // one leaves that attachment exactly as it was.
    let case = "a resource policy at the size limit";
    let at_the_limit = policy_of_length(RESOURCE_POLICY_MAX);
    ok(
        &router,
        "PutResourcePolicy",
        json!({"SecretId": "limits", "ResourcePolicy": at_the_limit.clone()}),
        case,
    )
    .await;
    let validated = ok(
        &router,
        "ValidateResourcePolicy",
        json!({"SecretId": "limits", "ResourcePolicy": at_the_limit.clone()}),
        case,
    )
    .await;
    assert_eq!(
        validated.get("PolicyValidationPassed").and_then(Value::as_bool),
        Some(true),
        "case {case:?}: expected the document at the limit to validate, got {validated}"
    );
    let expected_sid = first_sid(&at_the_limit, case);
    let stored = attached_policy(&router, "limits", case).await;
    assert_eq!(
        first_sid(&stored, case),
        expected_sid,
        "case {case:?}: expected the document at the limit to read back, got {stored:.64?}"
    );
    let refused_case = "an oversized attach after one that was accepted";
    let answered = err(
        &router,
        "PutResourcePolicy",
        json!({"SecretId": "limits", "ResourcePolicy": over_long_policy}),
        refused_case,
    )
    .await;
    refused(refused_case, answered, PARAMETER, POLICY_SIZE_SENTENCE);
    let unchanged = attached_policy(&router, "limits", refused_case).await;
    assert_eq!(
        first_sid(&unchanged, refused_case),
        expected_sid,
        "case {refused_case:?}: expected the attached policy to be the accepted one, got \
         {unchanged:.64?}"
    );

    // A maximum this service does NOT state itself keeps the shared constraint clause: the
    // listing's own page size has a bound of its own, and a value past it is still the
    // `ValidationException` every other declared rule is answered with.
    let case = "the listing's page size, one past its own range";
    let answered = err(
        &router,
        "ListSecrets",
        json!({"MaxResults": MAX_RESULTS_MAX + 1}),
        case,
    )
    .await;
    refused(
        case,
        answered,
        CONSTRAINT,
        &one_violation(&format!(
            "Value '{}' at 'maxResults' failed to satisfy constraint: Member must have value \
             less than or equal to {MAX_RESULTS_MAX}",
            MAX_RESULTS_MAX + 1
        )),
    );
}

/// The fixed sentence a `SecretId` the account holds no secret for is refused with, held in
/// one place by the service so every operation that resolves an id answers the same words.
const NOT_FOUND_MESSAGE: &str =
    service_com_amazonaws_secretsmanager_secretsmanager::state::SECRET_NOT_FOUND_MESSAGE;

/// The sentence `UpdateSecretVersionStage` refuses a call naming neither version member
/// with: `MoveToVersionId` is "the ID of the version to add the staging label to" and
/// `RemoveFromVersionId` "the ID of the version that the staging label is to be removed
/// from" (this service's Smithy model), and both are optional, so a call naming neither
/// asks for no change.
const NO_VERSION_NAMED_MESSAGE: &str =
    "You must specify either MoveToVersionId or RemoveFromVersionId.";

/// One operation's rule over the COMBINATION of its members: the members whose combination
/// the rule refuses, and the class and sentence that refusal carries for a secret the
/// account HOLDS.
struct CombinationRule {
    operation: &'static str,
    case: &'static str,
    /// The request members besides `SecretId`.
    members: Value,
    code: &'static str,
    message: &'static str,
}

/// Every rule of this service that is stated over a combination of request members rather
/// than over one member's own value, one row per operation that carries one — the two
/// version members of a staging-label move, and the two value members of each write that
/// stores one (both named, and, where the operation's purpose is to store a value, neither).
fn combination_rules() -> Vec<CombinationRule> {
    vec![
        CombinationRule {
            operation: "UpdateSecretVersionStage",
            case: "neither version member, moving the service's own label",
            members: json!({"VersionStage": "AWSCURRENT"}),
            code: PARAMETER,
            message: NO_VERSION_NAMED_MESSAGE,
        },
        CombinationRule {
            operation: "UpdateSecretVersionStage",
            case: "neither version member, moving a caller's own label",
            members: json!({"VersionStage": "blue"}),
            code: PARAMETER,
            message: NO_VERSION_NAMED_MESSAGE,
        },
        CombinationRule {
            operation: "PutSecretValue",
            case: "both value members",
            members: json!({
                "SecretString": "stored",
                "SecretBinary": "AAECAwQ=",
                "ClientRequestToken": token("put-both-values"),
            }),
            code: PARAMETER,
            message: BOTH_SECRET_VALUES_MESSAGE,
        },
        CombinationRule {
            operation: "PutSecretValue",
            case: "neither value member",
            members: json!({"ClientRequestToken": token("put-no-value")}),
            code: STATE,
            message: NO_SECRET_VALUE_MESSAGE,
        },
        CombinationRule {
            operation: "UpdateSecret",
            case: "both value members",
            members: json!({
                "SecretString": "stored",
                "SecretBinary": "AAECAwQ=",
                "ClientRequestToken": token("update-both-values"),
            }),
            code: PARAMETER,
            message: BOTH_SECRET_VALUES_MESSAGE,
        },
    ]
}

/// The request `rule` sends against the secret `secret_id` addresses.
fn addressed_request(rule: &CombinationRule, secret_id: &str) -> Value {
    let mut body = rule.members.clone();
    let members = body.as_object_mut().expect("a request body is an object");
    members.insert("SecretId".to_string(), json!(secret_id));
    body
}

/// THE RULE: an operation addressed to ONE secret resolves that secret before it applies a
/// rule over the COMBINATION of its other members — such a rule states what change the
/// request asks of the secret, and a secret the Region does not hold admits no change — so a
/// `SecretId` naming no secret is `ResourceNotFoundException` with the service's one
/// not-found sentence, and the combination rule decides only for a secret that exists.
///
/// The rows span what the rule generalises over: the operation and the combination its rule
/// is stated over (the two version members of a staging-label move, the two value members of
/// each write that stores one), the staging label named, and the form the `SecretId`
/// addresses the absent secret by (a bare name, a full ARN, a partial ARN).
#[tokio::test]
async fn a_missing_secret_outranks_a_rule_over_combined_members() {
    for rule in combination_rules() {
        for (form, absent) in absent_secret_ids("absent-secret") {
            let case = format!("{} with {} on {form}", rule.operation, rule.case);
            let router = emulator();
            let answered = err(
                &router,
                rule.operation,
                addressed_request(&rule, &absent),
                &case,
            )
            .await;
            refused(&case, answered, NOT_FOUND, NOT_FOUND_MESSAGE);
        }
    }
}

/// THE RULE's other half: for a secret the account DOES hold, the combination rule is what
/// refuses the same request — the secret resolving first takes nothing away from it.
#[tokio::test]
async fn a_rule_over_combined_members_still_refuses_a_secret_that_exists() {
    for rule in combination_rules() {
        let case = format!("{} with {} on a held secret", rule.operation, rule.case);
        let router = emulator();
        create(&router, "held-secret", &case).await;
        let answered = err(
            &router,
            rule.operation,
            addressed_request(&rule, "held-secret"),
            &case,
        )
        .await;
        refused(&case, answered, rule.code, rule.message);

        // The refused call changed nothing: the secret is still there to be read.
        let described = ok(
            &router,
            "DescribeSecret",
            json!({"SecretId": "held-secret"}),
            &case,
        )
        .await;
        assert_eq!(
            described.get("Name").and_then(Value::as_str),
            Some("held-secret"),
            "case {case:?}: expected the refused call to leave the secret as it was, got \
             {described}"
        );
    }
}

/// The forms a `SecretId` addresses a secret the account holds nothing under by: "the ARN or
/// name of the secret" (`SecretIdType`'s members, this service's Smithy model) plus the
/// partial ARN that names a secret too — the full ARN without the six trailing characters a
/// secret's ARN ends in. `(the form's name, the id)`.
fn absent_secret_ids(name: &str) -> Vec<(String, String)> {
    let arn = format!(
        "arn:aws:secretsmanager:us-west-2:{}:secret:{name}-AbCdEf",
        ::engine::emulator_account_id()
    );
    let partial = arn
        .rsplit_once('-')
        .map(|(prefix, _)| prefix.to_string())
        .unwrap_or_else(|| arn.clone());
    vec![
        ("a bare name".to_string(), name.to_string()),
        ("a full ARN".to_string(), arn),
        ("a partial ARN".to_string(), partial),
    ]
}

/// The published maximum of `FiltersListType` (`@length` 0..10, this service's Smithy
/// model): how many filters one selection carries.
const FILTERS_PER_REQUEST: usize = 10;

/// One `Filter` as a request carries it, and as a clause renders it.
struct SentFilter {
    key: &'static str,
    values: Vec<String>,
}

impl SentFilter {
    /// The filter as a request body member.
    fn sent(&self) -> Value {
        json!({"Key": self.key, "Values": self.values})
    }

    /// The filter as a clause renders a modelled structure: the shape's name over the
    /// members that were set, each `<member>=<value>`, in the order the model declares them
    /// (`smithy_query(query="shape",shape="Filter")`).
    fn rendered(&self) -> String {
        format!(
            "Filter(key={}, values=[{}])",
            self.key,
            self.values.join(", ")
        )
    }
}

/// `count` filters a request may carry, each selecting on `name` with a value of its own.
fn sent_filters(count: usize) -> Vec<SentFilter> {
    (0..count)
        .map(|index| SentFilter {
            key: "name",
            values: vec![format!("clauses-filter-{index}")],
        })
        .collect()
}

/// Those filters as a request body member.
fn sent_filter_list(filters: &[SentFilter]) -> Value {
    Value::Array(filters.iter().map(SentFilter::sent).collect())
}

/// Those filters as a clause quotes the member that held them: the rendered list, held to
/// what a clause of this service echoes.
fn echoed_filter_list(filters: &[SentFilter]) -> String {
    let rendered: Vec<String> = filters.iter().map(SentFilter::rendered).collect();
    echoed_value(&format!("[{}]", rendered.join(", ")))
}

/// THE RULE: a COLLECTION member's own `@length` refusal quotes the collection the request
/// carried at that member — rendered as a value renders inside a clause, a list of modelled
/// structures included — held to what a clause of this service echoes, rather than stating
/// no value at all.
///
/// The rows run over both operations that take `Filters`, over the collection's element kind
/// (a list of structures and a list of strings), and over the two sides of the echo bound;
/// the accepted rows show that the published maximum ITSELF is served.
#[tokio::test]
async fn a_collection_past_its_length_quotes_the_collection_the_request_carried() {
    let router = emulator();
    create(&router, "clauses-filter-0", "setup").await;
    for operation in ["BatchGetSecretValue", "ListSecrets"] {
        // The published maximum is served: it is being past it that is refused.
        let at_the_bound = sent_filters(FILTERS_PER_REQUEST);
        let case = format!("{operation} with {FILTERS_PER_REQUEST} filters");
        ok(
            &router,
            operation,
            json!({"Filters": sent_filter_list(&at_the_bound)}),
            &case,
        )
        .await;

        let over_the_bound = sent_filters(FILTERS_PER_REQUEST + 1);
        let case = format!("{operation} with {} filters", FILTERS_PER_REQUEST + 1);
        let answered = err(
            &router,
            operation,
            json!({"Filters": sent_filter_list(&over_the_bound)}),
            &case,
        )
        .await;
        refused(
            &case,
            answered,
            CONSTRAINT,
            &one_violation(&format!(
                "Value '{}' at 'filters' failed to satisfy constraint: Member must have length \
                 less than or equal to {FILTERS_PER_REQUEST}",
                echoed_filter_list(&over_the_bound)
            )),
        );
    }

    // The same rule over a list of STRINGS, and over a rendering INSIDE the echo bound: an
    // empty list is quoted as the empty list it was.
    let case = "a staging-label list below its minimum";
    let answered = err(
        &router,
        "PutSecretValue",
        json!({
            "SecretId": "clauses-filter-0",
            "SecretString": "v",
            "ClientRequestToken": token("no-stages"),
            "VersionStages": [],
        }),
        case,
    )
    .await;
    refused(
        case,
        answered,
        CONSTRAINT,
        &one_violation(
            "Value '[]' at 'versionStages' failed to satisfy constraint: Member must have \
             length greater than or equal to 1",
        ),
    );
}

/// THE RULE: a filter carrying more values than one filter admits is refused with Secrets
/// Manager's OWN parameter error naming that count, and a filter value outside the character
/// set a search value may be built from with its own parameter error naming the value —
/// neither with the shared constraint clause, whose code an operation does not model.
///
/// The rows run over both operations that take `Filters`, over the position in the filter
/// list, over which character outside the set the value carries, and over the values the
/// grammar admits (which are served).
#[tokio::test]
async fn a_filters_values_are_refused_as_the_service_states_them() {
    let router = emulator();
    create(&router, "clauses-filter-0", "setup").await;
    let too_many: Vec<String> = (0..=FILTER_VALUES_PER_FILTER)
        .map(|index| format!("value-{index}"))
        .collect();
    for operation in ["BatchGetSecretValue", "ListSecrets"] {
        // A filter at the published count of values is served.
        let case = format!("{operation} with {FILTER_VALUES_PER_FILTER} filter values");
        let at_the_bound: Vec<String> = too_many
            .iter()
            .take(FILTER_VALUES_PER_FILTER)
            .cloned()
            .collect();
        ok(
            &router,
            operation,
            json!({"Filters": [{"Key": "name", "Values": at_the_bound}]}),
            &case,
        )
        .await;

        let case = format!("{operation} with {} filter values", too_many.len());
        let answered = err(
            &router,
            operation,
            json!({"Filters": [{"Key": "name", "Values": too_many.clone()}]}),
            &case,
        )
        .await;
        refused(
            &case,
            answered,
            PARAMETER,
            &too_many_filter_values_message(),
        );

        // At a later position in the filter list, the same refusal.
        let case = format!("{operation} with too many values on the second filter");
        let answered = err(
            &router,
            operation,
            json!({
                "Filters": [
                    {"Key": "name", "Values": ["ok"]},
                    {"Key": "description", "Values": too_many.clone()},
                ],
            }),
            &case,
        )
        .await;
        refused(
            &case,
            answered,
            PARAMETER,
            &too_many_filter_values_message(),
        );

        // A value outside the grammar is refused naming that value, whichever character
        // outside the set it carries, whichever position it sits in, and however long it
        // is: this sentence copies the value from the request.
        let long_outside = format!("{}#", run("x", ECHO_BOUND * 2));
        for outside in [
            "clauses#",
            "clauses*",
            "value?",
            "a,b",
            "ü",
            long_outside.as_str(),
        ] {
            let case = format!("{operation} with the filter value {outside:?}");
            let answered = err(
                &router,
                operation,
                json!({"Filters": [{"Key": "name", "Values": [outside]}]}),
                &case,
            )
            .await;
            refused(
                &case,
                answered,
                PARAMETER,
                &invalid_filter_value_message(outside),
            );
        }
        let case = format!("{operation} with an out-of-grammar value at a later position");
        let answered = err(
            &router,
            operation,
            json!({"Filters": [{"Key": "name", "Values": ["fine", "late#"]}]}),
            &case,
        )
        .await;
        refused(
            &case,
            answered,
            PARAMETER,
            &invalid_filter_value_message("late#"),
        );

        // Every character the grammar admits is served, the leading negation mark
        // included ("You can prefix your search value with an exclamation mark",
        // `Filter$Values`, this service's Smithy model).
        for admitted in ["!clauses-filter-0", "a b:c_d@e/f+g=h.i-j!", ""] {
            let case = format!("{operation} with the filter value {admitted:?}");
            ok(
                &router,
                operation,
                json!({"Filters": [{"Key": "name", "Values": [admitted]}]}),
                &case,
            )
            .await;
        }
    }
}

/// THE RULE: a pagination token this service cannot resume from is refused as an invalid
/// pagination token — the class every paginated operation of this service models — and never
/// as a schema violation, whether the token is past the ceiling a token has or simply names
/// no cursor.
///
/// The rows run over every operation that pages, over both sides of the published ceiling,
/// and over a token that is well sized but names nothing.
#[tokio::test]
async fn a_pagination_token_the_service_cannot_resume_from_is_an_invalid_token() {
    let router = emulator();
    create(&router, "tokens", "setup").await;
    let name_filter = json!([{"Key": "name", "Values": ["tokens"]}]);
    let past_the_ceiling = run("Q", NEXT_TOKEN_MAX + 1);
    let at_the_ceiling = run("Q", NEXT_TOKEN_MAX);
    let rows: Vec<(String, &str, Value)> = vec![
        (
            format!("the batched read with a {}-character token", NEXT_TOKEN_MAX + 1),
            "BatchGetSecretValue",
            json!({"Filters": name_filter.clone(), "NextToken": past_the_ceiling.clone()}),
        ),
        (
            format!("the batched read with a {NEXT_TOKEN_MAX}-character token"),
            "BatchGetSecretValue",
            json!({"Filters": name_filter.clone(), "NextToken": at_the_ceiling.clone()}),
        ),
        (
            format!("the listing with a {}-character token", NEXT_TOKEN_MAX + 1),
            "ListSecrets",
            json!({"NextToken": past_the_ceiling.clone()}),
        ),
        (
            format!("the listing with a {NEXT_TOKEN_MAX}-character token"),
            "ListSecrets",
            json!({"NextToken": at_the_ceiling.clone()}),
        ),
        (
            format!("the version listing with a {}-character token", NEXT_TOKEN_MAX + 1),
            "ListSecretVersionIds",
            json!({"SecretId": "tokens", "NextToken": past_the_ceiling}),
        ),
        (
            format!("the version listing with a {NEXT_TOKEN_MAX}-character token"),
            "ListSecretVersionIds",
            json!({"SecretId": "tokens", "NextToken": at_the_ceiling.clone()}),
        ),
        (
            format!("the policy listing with a {}-character token", NEXT_TOKEN_MAX + 1),
            "ListResourcePolicies",
            json!({"SecretId": "tokens", "NextToken": run("Q", NEXT_TOKEN_MAX + 1)}),
        ),
        (
            format!("the policy listing with a {NEXT_TOKEN_MAX}-character token"),
            "ListResourcePolicies",
            json!({"SecretId": "tokens", "NextToken": at_the_ceiling}),
        ),
    ];
    for (case, operation, body) in rows {
        let answered = err(&router, operation, body, &case).await;
        assert!(
            !answered.1.contains("validation error"),
            "case {case:?}: expected the invalid-token sentence, got {:?}",
            answered.1
        );
        refused(&case, answered, INVALID_TOKEN, INVALID_NEXT_TOKEN_MESSAGE);
    }
}

/// THE RULE: a `SecretId` is read before anything is looked up — an identifier at or past
/// the size limit the service states for one is an invalid parameter, and a shorter one that
/// is neither a secret's ARN nor a name a secret can be created under carries the one
/// sentence stating what a name may be built from — so neither is reported as a secret the
/// account does not hold.
///
/// The rows run over both halves of the rule, over the length relative to each published
/// bound, over which character outside the name set the identifier carries, and over the
/// identifiers that CAN name a secret (which resolve, or are reported missing).
#[tokio::test]
async fn a_secret_id_the_service_cannot_use_is_refused_before_any_lookup() {
    let router = emulator();
    create(&router, "policies", "setup").await;
    let past_the_size_limit: Vec<(String, String)> = vec![
        (
            format!("an id of {SECRET_ID_SIZE_LIMIT} characters"),
            run("a", SECRET_ID_SIZE_LIMIT),
        ),
        (
            format!("an id of {} characters", SECRET_ID_SIZE_LIMIT + 1),
            run("a", SECRET_ID_SIZE_LIMIT + 1),
        ),
        (
            format!("an id of {} characters", SECRET_ID_SIZE_LIMIT * 2),
            run("a", SECRET_ID_SIZE_LIMIT * 2),
        ),
    ];
    for (case, secret_id) in &past_the_size_limit {
        let answered = err(
            &router,
            "DeleteResourcePolicy",
            json!({"SecretId": secret_id}),
            case,
        )
        .await;
        assert!(
            !answered.1.contains("aaaa"),
            "case {case:?}: expected the sentence to carry no part of the id, got {:?}",
            answered.1
        );
        refused(case, answered, PARAMETER, SECRET_ID_TOO_LONG_MESSAGE);
    }

    let not_a_name: Vec<(String, String)> = vec![
        (
            format!("an id of {} characters", SECRET_NAME_CHARS + 1),
            run("a", SECRET_NAME_CHARS + 1),
        ),
        ("an id carrying a space".to_string(), "my policy".to_string()),
        ("an id carrying a hash".to_string(), "policies#1".to_string()),
        (
            "an ARN of another service's resource".to_string(),
            format!(
                "arn:aws:iam::{}:role/not-a-secret",
                ::engine::emulator_account_id()
            ),
        ),
    ];
    for (case, secret_id) in &not_a_name {
        let answered = err(
            &router,
            "DeleteResourcePolicy",
            json!({"SecretId": secret_id}),
            case,
        )
        .await;
        refused(case, answered, CONSTRAINT, INVALID_SECRET_NAME_MESSAGE);
    }

    // An identifier that CAN name a secret is looked up: a secret the account holds is
    // served, and one it does not hold is reported missing — in every form a `SecretId`
    // takes.
    for (form, absent) in absent_secret_ids("absent-policy-secret") {
        let case = format!("an absent secret addressed by {form}");
        let answered = err(
            &router,
            "DeleteResourcePolicy",
            json!({"SecretId": absent}),
            &case,
        )
        .await;
        refused(&case, answered, NOT_FOUND, NOT_FOUND_MESSAGE);
    }
    let case = "a secret the account holds";
    let answered = ok(
        &router,
        "DeleteResourcePolicy",
        json!({"SecretId": "policies"}),
        case,
    )
    .await;
    assert_eq!(
        answered.get("Name").and_then(Value::as_str),
        Some("policies"),
        "case {case:?}: expected the addressed secret back, got {answered}"
    );
}

/// THE RULE's per-entry half: a `SecretIdList` entry the service cannot use fails THAT entry
/// alone — the batch answers 200, serves every other selection, and reports the entry as the
/// invalid name it is rather than as a secret that is missing.
///
/// The rows run over the forms an entry cannot name a secret in (a length past the name
/// maximum, up to the longest entry the member admits; a character outside the name set; an
/// ARN of another service's resource) and over an absent-but-usable name, which is reported
/// missing instead.
#[tokio::test]
async fn a_batched_read_reports_an_unusable_id_as_an_invalid_name() {
    let router = emulator();
    create(&router, "batch-real", "setup").await;
    let rows: Vec<(String, String, &str, &str)> = vec![
        (
            format!("an entry of {SECRET_ID_LIST_ENTRY_MAX} characters"),
            run("a", SECRET_ID_LIST_ENTRY_MAX),
            CONSTRAINT,
            INVALID_SECRET_NAME_MESSAGE,
        ),
        (
            format!("an entry of {} characters", SECRET_NAME_CHARS + 1),
            run("a", SECRET_NAME_CHARS + 1),
            CONSTRAINT,
            INVALID_SECRET_NAME_MESSAGE,
        ),
        (
            "an entry carrying a space".to_string(),
            "batch real".to_string(),
            CONSTRAINT,
            INVALID_SECRET_NAME_MESSAGE,
        ),
        (
            "an entry that is another service's ARN".to_string(),
            format!(
                "arn:aws:iam::{}:role/not-a-secret",
                ::engine::emulator_account_id()
            ),
            CONSTRAINT,
            INVALID_SECRET_NAME_MESSAGE,
        ),
        (
            "an absent but usable name".to_string(),
            "batch-absent".to_string(),
            NOT_FOUND,
            NOT_FOUND_MESSAGE,
        ),
    ];
    for (case, entry, code, message) in rows {
        let read = ok(
            &router,
            "BatchGetSecretValue",
            json!({"SecretIdList": [entry.clone(), "batch-real"]}),
            &case,
        )
        .await;
        let served: Vec<&str> = read
            .get("SecretValues")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case:?}: no SecretValues in {read}"))
            .iter()
            .filter_map(|value| value.get("Name").and_then(Value::as_str))
            .collect();
        assert_eq!(
            served,
            vec!["batch-real"],
            "case {case:?}: expected the usable selection to be served, got {read}"
        );
        let errors = read
            .get("Errors")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case:?}: no Errors in {read}"));
        let reported: Vec<(&str, &str, &str)> = errors
            .iter()
            .map(|entry| {
                let member = |name: &str| entry.get(name).and_then(Value::as_str).unwrap_or("");
                (member("SecretId"), member("ErrorCode"), member("Message"))
            })
            .collect();
        assert_eq!(
            reported,
            vec![(entry.as_str(), code, message)],
            "case {case:?}: expected one entry reporting the id as sent, got {read}"
        );
    }
}

/// THE RULE: the value a clause quotes at a LIST member is held to what a clause of this
/// service echoes, exactly as the value of a scalar member's own clause is — so a caller
/// never receives an unbounded echo of what it sent.
///
/// The rows run over the list's element shape, over whether the rendered list is inside the
/// echo bound or past it, and over the operation that carried it.
#[tokio::test]
async fn the_list_a_clause_quotes_is_held_to_the_echo_bound() {
    let router = emulator();
    create(&router, "elisions", "setup").await;
    let at_the_tag_key_bound = run("t", TAG_KEY_MAX);
    let past_the_tag_key_bound = run("u", TAG_KEY_MAX + 1);
    let keys = [
        at_the_tag_key_bound.as_str(),
        past_the_tag_key_bound.as_str(),
    ];
    let case = "a tag key one character past its maximum, beside one at it";
    let answered = err(
        &router,
        "UntagResource",
        json!({"SecretId": "elisions", "TagKeys": keys}),
        case,
    )
    .await;
    refused(
        case,
        answered.clone(),
        CONSTRAINT,
        &one_violation(&format!(
            "Value '{}' at 'tagKeys' failed to satisfy constraint: {TAG_KEY_SET}",
            echoed_list(&keys)
        )),
    );
    assert!(
        answered.1.ends_with(TAG_KEY_SET) && answered.1.contains(ELISION),
        "case {case:?}: expected the rendered list elided, got {:?}",
        answered.1
    );
    assert!(
        !answered.1.contains(&run("u", ECHO_BOUND + 1)),
        "case {case:?}: the clause must echo at most {ECHO_BOUND} characters of the list, got \
         {:?}",
        answered.1
    );

    // A rendered list INSIDE the bound is quoted whole, with no marker.
    let case = "a tag key below its minimum, in a short list";
    let answered = err(
        &router,
        "UntagResource",
        json!({"SecretId": "elisions", "TagKeys": ["keep", ""]}),
        case,
    )
    .await;
    refused(
        case,
        answered,
        CONSTRAINT,
        &one_violation(&format!(
            "Value '[keep, ]' at 'tagKeys' failed to satisfy constraint: {TAG_KEY_SET}"
        )),
    );
}

/// What every element of an `UntagResource` `TagKeys` list has to satisfy, as the clause
/// stated at that member nests it: the whole set `TagKeyType` declares (`@length` 1..128;
/// `smithy_query(query="shape",shape="TagKeyType")`).
const TAG_KEY_SET: &str = "Member must satisfy constraint: [Member must have length less than \
     or equal to 128, Member must have length greater than or equal to 1]";

/// THE RULE: a `@pattern` clause quotes the expression this service publishes for the
/// member's shape — the whole-value form where that is what it publishes — so a caller
/// repairing its value reads exactly the rule that was enforced.
#[tokio::test]
async fn a_pattern_clause_quotes_the_expression_the_service_publishes() {
    let router = emulator();
    let stored = "v";
    create(&router, "patterns", "setup").await;
    // A token of the minimum modelled length (`RotationTokenType`, `@length` 36..256), so
    // the only rule it breaks is the character set.
    let outside = format!("{}#", run("a", 35));
    let case = "a rotation token carrying a character outside its set";
    let answered = err(
        &router,
        "PutSecretValue",
        json!({
            "SecretId": "patterns",
            "SecretString": "replaced",
            "ClientRequestToken": token("rotation-token"),
            "RotationToken": outside.clone(),
        }),
        case,
    )
    .await;
    assert!(
        !answered.1.contains(&outside),
        "case {case:?}: a sensitive member's value is never echoed, got {:?}",
        answered.1
    );
    refused(
        case,
        answered,
        CONSTRAINT,
        &one_violation(
            "Value at 'rotationToken' failed to satisfy constraint: Member must satisfy \
             regular expression pattern: ^[a-zA-Z0-9\\-]+$",
        ),
    );

    // The refused write stored nothing: the secret still holds the value it was created
    // with.
    let read = ok(
        &router,
        "GetSecretValue",
        json!({"SecretId": "patterns"}),
        case,
    )
    .await;
    assert_eq!(
        read.get("SecretString").and_then(Value::as_str),
        Some(stored),
        "case {case:?}: expected the refused write to leave the value as it was, got {read}"
    );

    // A token the model's length and character set DO admit, but that is not the form a
    // rotation's identifier has, is the service's own parameter refusal — and stores
    // nothing either. The rows run over the forms such a token takes.
    let shaped_wrong: Vec<(String, String)> = vec![
        (
            format!("a token of {ROTATION_TOKEN_CHARS} characters that is not an identifier"),
            run("a", ROTATION_TOKEN_CHARS),
        ),
        (
            "a token with the hyphens in the wrong places".to_string(),
            format!("{}-{}", run("a", 20), run("b", 15)),
        ),
        (
            "a token longer than an identifier".to_string(),
            run("a", ROTATION_TOKEN_CHARS * 2),
        ),
    ];
    for (case, rotation_token) in &shaped_wrong {
        let answered = err(
            &router,
            "PutSecretValue",
            json!({
                "SecretId": "patterns",
                "SecretString": "replaced",
                "ClientRequestToken": token(case),
                "RotationToken": rotation_token,
            }),
            case,
        )
        .await;
        refused(case, answered, PARAMETER, INVALID_ROTATION_TOKEN_MESSAGE);
        let read = ok(
            &router,
            "GetSecretValue",
            json!({"SecretId": "patterns"}),
            case,
        )
        .await;
        assert_eq!(
            read.get("SecretString").and_then(Value::as_str),
            Some(stored),
            "case {case:?}: expected the refused write to stake no version, got {read}"
        );
    }

    // A token OF that form is accepted, and the write it carries stores its value. The
    // form is eight, four, four, four and twelve hexadecimal characters joined by hyphens.
    let case = "a token of the form a rotation's identifier has";
    let minted = "0123abcd-4567-89ef-0123-456789abcdef";
    assert_eq!(
        minted.len(),
        ROTATION_TOKEN_CHARS,
        "case {case:?}: expected a token of {ROTATION_TOKEN_CHARS} characters, got {}",
        minted.len()
    );
    ok(
        &router,
        "PutSecretValue",
        json!({
            "SecretId": "patterns",
            "SecretString": "rotated",
            "ClientRequestToken": token(case),
            "RotationToken": minted,
        }),
        case,
    )
    .await;
    let read = ok(
        &router,
        "GetSecretValue",
        json!({"SecretId": "patterns"}),
        case,
    )
    .await;
    assert_eq!(
        read.get("SecretString").and_then(Value::as_str),
        Some("rotated"),
        "case {case:?}: expected the accepted write to store its value, got {read}"
    );

    // A shape this service publishes WITHOUT the whole-value pair keeps the unanchored
    // rendering of the same expression.
    let case = "a rotation window outside its pattern";
    let answered = err(
        &router,
        "RotateSecret",
        json!({"SecretId": "patterns", "RotationRules": {"Duration": "1x"}}),
        case,
    )
    .await;
    refused(
        case,
        answered,
        CONSTRAINT,
        &one_violation(
            "Value '1x' at 'rotationRules.duration' failed to satisfy constraint: Member must \
             satisfy regular expression pattern: [0-9]+h",
        ),
    );
}

/// THE RULE: an enumerated type's admitted values are stated in the order THIS SERVICE lists
/// that set, which is a property of the type rather than of the value that was rejected.
///
/// The rows run over the values outside the domain (including the upper-case spellings of
/// the admitted ones), and the accepted rows show every admitted value is served.
#[tokio::test]
async fn an_enum_clause_states_the_sort_order_set_in_the_services_order() {
    let router = emulator();
    create(&router, "orders", "setup").await;
    let set = STATED_SORT_ORDER_VALUE_SET.join(", ");
    let mut sorted_model: Vec<&str> = SORT_ORDER_TYPE_VALUES.to_vec();
    sorted_model.sort_unstable();
    let mut sorted_stated: Vec<&str> = STATED_SORT_ORDER_VALUE_SET.to_vec();
    sorted_stated.sort_unstable();
    assert_eq!(
        sorted_stated, sorted_model,
        "case \"the stated sort orders\": the stated set must hold exactly the model's values"
    );
    for rejected in ["ASC", "DESC", "Asc", "sideways"] {
        let case = format!("the sort order {rejected:?}");
        let answered = err(
            &router,
            "ListSecrets",
            json!({"SortOrder": rejected}),
            &case,
        )
        .await;
        refused(
            &case,
            answered,
            CONSTRAINT,
            &one_violation(&format!(
                "Value '{rejected}' at 'sortOrder' failed to satisfy constraint: Member must \
                 satisfy enum value set: [{set}]"
            )),
        );
    }
    for admitted in SORT_ORDER_TYPE_VALUES {
        let case = format!("the sort order {admitted:?}");
        ok(
            &router,
            "ListSecrets",
            json!({"SortOrder": admitted}),
            &case,
        )
        .await;
    }
}
