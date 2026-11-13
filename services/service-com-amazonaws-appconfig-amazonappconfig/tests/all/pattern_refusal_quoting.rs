// SPDX-License-Identifier: CC0-1.0

//! How AppConfig QUOTES the `@pattern` a refused request broke.
//!
//! The rule under test, one level more general than any single request: a constraint
//! violation is the modelled `BadRequestException` (HTTP 400) whose message states the
//! member and the expression the value had to satisfy — and the expression is the one the
//! WHOLE value must match: an end the model anchors keeps the model's text there except the
//! trailing `$`, which goes, and an end the model leaves OPEN is spelled with the
//! unconstrained run `.*` it admits. Everything else in the expression is quoted as
//! modelled: a leading `^`, and the anchors a branch of the expression carries of its own.
//!
//! Only the RENDERING drops that anchor. The CHECK still runs the model's pattern as
//! written, which the table pins with a value that satisfies the QUOTED expression over a
//! prefix of itself and is refused anyway, and with a value that satisfies the pattern and
//! is served.
//!
//! The rows span what the rule generalises over: the shape of the modelled expression
//! (anchored at both ends, anchored inside its own branches, carrying no anchor), where
//! the member is bound (a body member, a nested-structure member, an `@httpHeader`, an
//! `@httpLabel`), and the operation behind it. Every pattern is the model's own, read with
//! `smithy_query` and named per row.

use serde_json::{json, Value};

use crate::appconfig_workflow::{
    create_application, create_environment, create_flags_profile, create_profile, json_call,
    router, send, Answer,
};

/// One request that breaks a `@pattern`, and the expression its refusal must quote.
struct Quoted {
    /// What a failing assertion calls this row: the operation and the member.
    case: String,
    method: &'static str,
    uri: String,
    body: Value,
    headers: Vec<(&'static str, String)>,
    /// The lowerCamel member path the clause names.
    member: &'static str,
    /// The value the request sent, which the clause echoes back.
    value: String,
    /// The `@pattern` the model declares for that member, from `smithy_query`. Reported
    /// when a row fails, so a failure names the expression the clause was derived from;
    /// the expectation itself is `quoted`, written out per row.
    modelled: &'static str,
    /// The expression the clause must quote: the whole-value form of `modelled`.
    quoted: &'static str,
}

/// The clause AppConfig states for one broken `@pattern`.
fn expected_message(row: &Quoted) -> String {
    format!(
        "1 validation error detected: Value '{}' at '{}' failed to satisfy constraint: Member \
         must satisfy regular expression pattern: {}",
        row.value, row.member, row.quoted
    )
}

/// A `CreateExperimentDefinition` body whose only questionable member is `FlagKey`.
fn experiment_body(environment: &str, profile: &str, name: &str, flag_key: &str) -> Value {
    json!({
        "Name": name,
        "ConfigurationProfileIdentifier": profile,
        "EnvironmentIdentifier": environment,
        "FlagKey": flag_key,
        "Treatments": [{ "Weight": 50.0, "FlagValue": { "Enabled": true } }],
        "Control": { "Weight": 50.0, "FlagValue": { "Enabled": false } },
        "AudienceRule": "true",
    })
}

/// Every `@pattern` refusal quotes the expression the whole value must satisfy, whatever
/// the expression's shape, wherever the member is bound and whichever operation was called.
#[tokio::test]
async fn a_pattern_refusal_quotes_the_whole_value_expression() {
    let router = router();
    let application = create_application(&router, "pattern-quoting").await;
    let environment = create_environment(&router, &application, "prod").await;
    let profile = create_profile(&router, &application, "config").await;

    // `smithy_query(query="shape", shape="FlagKey")`.
    const FLAG_KEY: &str = r"^[a-z][a-zA-Z0-9_-]{1,64}$";
    const FLAG_KEY_QUOTED: &str = r"^[a-z][a-zA-Z0-9_-]{1,64}";
    // `smithy_query(query="shape", shape="DeploymentStrategyId")` — anchored at both ends
    // AND inside each of its two branches.
    const STRATEGY_ID: &str = r"^(^[a-z0-9]{4,7}$|^AppConfig\.[A-Za-z0-9]{9,40}$)$";
    const STRATEGY_ID_QUOTED: &str = r"^(^[a-z0-9]{4,7}$|^AppConfig\.[A-Za-z0-9]{9,40}$)";
    // `smithy_query(query="shape", shape="VersionLabel")` — no anchor at either end, so
    // the expression is a "contains" demand and both ends are spelled with the run they
    // admit.
    const VERSION_LABEL: &str = r"[^0-9]";
    const VERSION_LABEL_QUOTED: &str = r".*[^0-9].*";
    // `smithy_query(query="operation", operation="CreateConfigurationProfile")`.
    const ROLE_ARN: &str =
        r"^((arn):(aws|aws-cn|aws-iso|aws-iso-[a-z]{1}|aws-us-gov|aws-eusc):(iam)::\d{12}:role[/].*)$";
    const ROLE_ARN_QUOTED: &str =
        r"^((arn):(aws|aws-cn|aws-iso|aws-iso-[a-z]{1}|aws-us-gov|aws-eusc):(iam)::\d{12}:role[/].*)";
    const PROFILE_TYPE: &str = r"^[a-zA-Z0-9\.\-]+$";
    const PROFILE_TYPE_QUOTED: &str = r"^[a-zA-Z0-9\.\-]+";
    // `smithy_query(query="shape", shape="Monitor")`: a monitor's `AlarmRoleArn` targets
    // `RoleArn`, so this clause is stated at a LIST ELEMENT's own member.
    let monitor_alarm = "arn:aws:cloudwatch:us-east-1:000000000000:alarm:a";

    let experiments = format!("/applications/{application}/experimentdefinitions");
    let profiles = format!("/applications/{application}/configurationprofiles");

    let rows: Vec<Quoted> = vec![
        Quoted {
            case: "CreateExperimentDefinition on a flag key starting with a digit".to_string(),
            method: "POST",
            uri: experiments.clone(),
            body: experiment_body(&environment, &profile, "digit-first", "1invalidFlagKey"),
            headers: Vec::new(),
            member: "flagKey",
            value: "1invalidFlagKey".to_string(),
            modelled: FLAG_KEY,
            quoted: FLAG_KEY_QUOTED,
        },
        Quoted {
            // The value a FIND of the quoted expression matches over its first six
            // characters: it is refused all the same, because the check is the model's
            // own anchored pattern and only the quoting drops the anchor.
            case: "CreateExperimentDefinition on a flag key whose prefix satisfies the \
                   quoted expression"
                .to_string(),
            method: "POST",
            uri: experiments.clone(),
            body: experiment_body(&environment, &profile, "trailing-bang", "myflag!"),
            headers: Vec::new(),
            member: "flagKey",
            value: "myflag!".to_string(),
            modelled: FLAG_KEY,
            quoted: FLAG_KEY_QUOTED,
        },
        Quoted {
            case: "StartDeployment on a strategy id outside both branches".to_string(),
            method: "POST",
            uri: format!("/applications/{application}/environments/{environment}/deployments"),
            body: json!({
                "ConfigurationProfileId": profile,
                "ConfigurationVersion": "1",
                "DeploymentStrategyId": "BAD-STRAT!",
            }),
            headers: Vec::new(),
            member: "deploymentStrategyId",
            value: "BAD-STRAT!".to_string(),
            modelled: STRATEGY_ID,
            quoted: STRATEGY_ID_QUOTED,
        },
        Quoted {
            case: "GetDeploymentStrategy on an @httpLabel outside both branches".to_string(),
            method: "GET",
            uri: "/deploymentstrategies/BAD!".to_string(),
            body: Value::Null,
            headers: Vec::new(),
            member: "deploymentStrategyId",
            value: "BAD!".to_string(),
            modelled: STRATEGY_ID,
            quoted: STRATEGY_ID_QUOTED,
        },
        Quoted {
            case: "CreateHostedConfigurationVersion on an all-digit @httpHeader".to_string(),
            method: "POST",
            uri: format!("{profiles}/{profile}/hostedconfigurationversions"),
            body: json!({ "flag": true }),
            headers: vec![("VersionLabel", "12345".to_string())],
            member: "versionLabel",
            value: "12345".to_string(),
            // Open at both ends: each is spelled with the run it admits.
            modelled: VERSION_LABEL,
            quoted: VERSION_LABEL_QUOTED,
        },
        Quoted {
            case: "CreateConfigurationProfile on a retrieval role that is not a role"
                .to_string(),
            method: "POST",
            uri: profiles.clone(),
            body: json!({
                "Name": "not-a-role",
                "LocationUri": "hosted",
                "RetrievalRoleArn": "arn:aws:iam::000000000000:user/not-a-role",
            }),
            headers: Vec::new(),
            member: "retrievalRoleArn",
            value: "arn:aws:iam::000000000000:user/not-a-role".to_string(),
            modelled: ROLE_ARN,
            quoted: ROLE_ARN_QUOTED,
        },
        Quoted {
            case: "CreateConfigurationProfile on a type carrying a space".to_string(),
            method: "POST",
            uri: profiles.clone(),
            body: json!({
                "Name": "bad-type",
                "LocationUri": "hosted",
                "Type": "AWS.AppConfig Feature Flags",
            }),
            headers: Vec::new(),
            member: "type",
            value: "AWS.AppConfig Feature Flags".to_string(),
            modelled: PROFILE_TYPE,
            quoted: PROFILE_TYPE_QUOTED,
        },
        Quoted {
            case: "CreateEnvironment on a monitor's alarm role ARN".to_string(),
            method: "POST",
            uri: format!("/applications/{application}/environments"),
            body: json!({
                "Name": "bad-monitor",
                "Monitors": [{
                    "AlarmArn": monitor_alarm,
                    "AlarmRoleArn": "arn:aws:iam::000000000000:user/not-a-role",
                }],
            }),
            headers: Vec::new(),
            member: "monitors.1.member.alarmRoleArn",
            value: "arn:aws:iam::000000000000:user/not-a-role".to_string(),
            modelled: ROLE_ARN,
            quoted: ROLE_ARN_QUOTED,
        },
    ];

    for row in &rows {
        let case = &row.case;
        let payload = if row.body.is_null() {
            Vec::new()
        } else {
            serde_json::to_vec(&row.body).expect("the request body must serialize")
        };
        let answer = send(
            &router,
            row.method,
            &row.uri,
            "application/json",
            payload,
            &row.headers,
        )
        .await;

        assert_eq!(
            answer.status,
            400,
            "case {case:?}: expected 400, got {} / {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        let code = answer.error_code();
        assert!(
            code.contains("BadRequestException") && !code.contains("ValidationException"),
            "case {case:?}: expected the modelled BadRequestException, got {code:?}"
        );
        // restJson1 resolves the error SHAPE from this header, so it must name the
        // modelled code too — `ValidationException` is in no AppConfig model.
        let errortype = answer.header("x-amzn-errortype").unwrap_or_default();
        assert!(
            errortype
                .split('#')
                .next_back()
                .unwrap_or_default()
                .starts_with("BadRequestException"),
            "case {case:?}: expected the BadRequestException shape in x-amzn-errortype, got \
             {errortype:?}"
        );
        let message = message_of(&answer)
            .unwrap_or_else(|| panic!("case {case:?}: no message in {}", answer.json()));
        let expected = expected_message(row);
        assert_eq!(
            message, expected,
            "case {case:?}: the model declares {:?} for this member, so expected message \
             {expected:?}, got {message:?}",
            row.modelled
        );
    }
}

/// The value AppConfig satisfies the SAME patterns with is served, and a request that
/// breaks two of them states both clauses with both expressions quoted the same way — so
/// the quoting is a property of the rendering, not of one refusal.
#[tokio::test]
async fn the_check_is_still_the_models_own_pattern() {
    let router = router();
    let application = create_application(&router, "pattern-accepts").await;
    let environment = create_environment(&router, &application, "prod").await;
    let profile = create_flags_profile(&router, &application, "config").await;

    // Flag keys the modelled `^[a-z][a-zA-Z0-9_-]{1,64}$` admits, differing along length
    // and along which of the admitted characters they use: the shortest the quantifier
    // allows (a leading letter plus one more character), the longest (plus 64), and a
    // mixed-case key carrying both punctuation characters.
    let accepted = [
        "ab".to_string(),
        format!("a{}", "b".repeat(64)),
        "my_Flag-Key9".to_string(),
    ];
    for (index, flag_key) in accepted.iter().enumerate() {
        let answer = json_call(
            &router,
            "POST",
            &format!("/applications/{application}/experimentdefinitions"),
            &json!({
                "Name": format!("accepted-{index}"),
                "ConfigurationProfileIdentifier": profile,
                "EnvironmentIdentifier": environment,
                "FlagKey": flag_key,
                "Treatments": [{ "Weight": 50.0, "FlagValue": { "Enabled": true } }],
                "Control": { "Weight": 50.0, "FlagValue": { "Enabled": false } },
                "AudienceRule": "true",
            }),
        )
        .await;
        assert_eq!(
            answer.status,
            201,
            "case {flag_key:?}: a flag key the model admits must be served, got {} / {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            answer.string("FlagKey").as_deref(),
            Some(flag_key.as_str()),
            "case {flag_key:?}: expected the stored flag key reported back, got {:?}",
            answer.string("FlagKey")
        );
        // The definition really persisted, so the accepted value is not just an echo.
        let id = answer
            .string("Id")
            .unwrap_or_else(|| panic!("case {flag_key:?}: no Id in {}", answer.json()));
        let read_back = json_call(
            &router,
            "GET",
            &format!("/applications/{application}/experimentdefinitions/{id}"),
            &Value::Null,
        )
        .await;
        assert_eq!(
            read_back.status,
            200,
            "case {flag_key:?}: GetExperimentDefinition must answer 200, got {} / {}",
            read_back.status,
            String::from_utf8_lossy(&read_back.body)
        );
        assert_eq!(
            read_back.string("FlagKey").as_deref(),
            Some(flag_key.as_str()),
            "case {flag_key:?}: expected the stored flag key on the read back, got {:?}",
            read_back.string("FlagKey")
        );
    }

    // Two broken patterns in one request: both clauses are stated, and each quotes its own
    // expression without its end anchor.
    let answer = json_call(
        &router,
        "POST",
        &format!("/applications/{application}/configurationprofiles"),
        &json!({
            "Name": "two-clauses",
            "LocationUri": "hosted",
            "RetrievalRoleArn": "arn:aws:iam::000000000000:user/not-a-role",
            "Type": "AWS.AppConfig Feature Flags",
        }),
    )
    .await;
    assert_eq!(
        answer.status,
        400,
        "two broken patterns must be refused, got {} / {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    let message = message_of(&answer)
        .unwrap_or_else(|| panic!("no message in {}", answer.json()));
    assert!(
        message.starts_with("2 validation errors detected: "),
        "expected both clauses in one envelope, got {message:?}"
    );
    for quoted in [
        r"^((arn):(aws|aws-cn|aws-iso|aws-iso-[a-z]{1}|aws-us-gov|aws-eusc):(iam)::\d{12}:role[/].*)",
        r"^[a-zA-Z0-9\.\-]+",
    ] {
        let clause = format!("Member must satisfy regular expression pattern: {quoted}");
        assert!(
            message.contains(&clause),
            "expected the clause {clause:?}, got {message:?}"
        );
    }
    assert!(
        !message.contains(r".*)$") && !message.contains(r"]+$"),
        "neither clause may quote an end anchor, got {message:?}"
    );
}

/// The refusal's message, in the member this service's model declares for it.
fn message_of(answer: &Answer) -> Option<String> {
    answer.string("Message")
}

/// Which body member an error's detail text is in, across the renderers that produce one:
/// the framework's pre-handler refusal, a modelled error a handler returns, and the
/// scaffold's own 501 signal — plus a success, which carries no such member.
///
/// RULE: every error this service's MODEL declares reports its text in `Message` (the
/// member name each error shape declares, with no `@jsonName`, on a restJson1 service), so
/// a client deserializing that shape reads the text wherever it was produced. The
/// scaffold's 501 is not one of those shapes — it is the emulator's "not written yet"
/// signal — so it keeps the member the framework writes it in.
#[tokio::test]
async fn an_errors_detail_text_is_in_the_member_the_model_declares() {
    let router = router();
    let application = create_application(&router, "detail-member").await;

    // (case, method, uri, body, status, the member the text must be in, the member it
    // must NOT be in)
    let rows: Vec<(&str, &str, String, Value, u16, &str, &str)> = vec![
        (
            "a pre-handler constraint refusal",
            "POST",
            "/applications".to_string(),
            json!({}),
            400,
            "Message",
            "message",
        ),
        (
            "a modelled error a handler returns",
            "GET",
            "/applications/zzzzzzz".to_string(),
            Value::Null,
            404,
            "Message",
            "message",
        ),
    ];

    for (case, method, uri, body, status, present, absent) in &rows {
        let answer = json_call(&router, method, uri, body).await;
        assert_eq!(
            answer.status, *status,
            "case {case:?}: expected {status}, got {} / {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert!(
            answer.string(present).is_some_and(|text| !text.is_empty()),
            "case {case:?}: expected the detail text in {present:?}, got {}",
            answer.json()
        );
        assert!(
            answer.string(absent).is_none(),
            "case {case:?}: {absent:?} must not carry it too, got {}",
            answer.json()
        );
    }

    // The scaffold's own 501 signal keeps the member the framework wrote it in. WHICH
    // operation is still a stub is not this suite's business — each is owned by its own
    // issue and implemented on its own schedule — so the assertion is made against the
    // FIRST of these modelled routes that still answers 501, rather than pinning one
    // operation. The rule itself is pinned, independently of any stub, by the
    // `with_detail_member` table in `src/service.rs`.
    let mut asserted_scaffold = false;
    let scaffold_candidates: [(&str, String); 3] = [
        ("GET", "/extensionassociations".to_string()),
        (
            "GET",
            format!("/experimentdefinitions?application_identifier={application}"),
        ),
        (
            "GET",
            format!("/applications/{application}/experimentdefinitions"),
        ),
    ];
    for (method, uri) in &scaffold_candidates {
        let answer = json_call(&router, method, uri, &Value::Null).await;
        if answer.status != 501 {
            continue;
        }
        let case = format!("the scaffold's own 501 signal at {method} {uri}");
        assert_eq!(
            answer.error_code(),
            "NotImplemented",
            "case {case:?}: a 501 is the scaffold signal, got {}",
            answer.json()
        );
        assert!(
            answer.string("message").is_some_and(|text| !text.is_empty()),
            "case {case:?}: expected the scaffold's text in \"message\", got {}",
            answer.json()
        );
        assert!(
            answer.string("Message").is_none(),
            "case {case:?}: the scaffold signal is not a modelled error, so it must not be \
             restated in \"Message\", got {}",
            answer.json()
        );
        asserted_scaffold = true;
        break;
    }
    assert!(
        asserted_scaffold,
        "every candidate route is implemented, so this service has no scaffold signal left \
         to assert here: name another still-scaffolded operation (or drop this block, since \
         the rule itself is pinned by the `with_detail_member` table in `src/service.rs`)"
    );

    // A success is untouched: no detail member is added to it.
    let read_back = json_call(
        &router,
        "GET",
        &format!("/applications/{application}"),
        &Value::Null,
    )
    .await;
    assert_eq!(
        read_back.status,
        200,
        "GetApplication must answer 200, got {} / {}",
        read_back.status,
        String::from_utf8_lossy(&read_back.body)
    );
    assert_eq!(
        read_back.string("Id").as_deref(),
        Some(application.as_str()),
        "expected the application's own record, got {}",
        read_back.json()
    );
    assert!(
        read_back.string("Message").is_none() && read_back.string("message").is_none(),
        "a success carries no detail member, got {}",
        read_back.json()
    );
}
