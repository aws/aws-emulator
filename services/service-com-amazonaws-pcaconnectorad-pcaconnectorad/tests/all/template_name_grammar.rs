// SPDX-License-Identifier: CC0-1.0

//! The grammar `TemplateName` publishes, and WHEN this service applies it.
//!
//! Two rules, asserted together because the second is only observable through the
//! first:
//!
//! 1. A value a modelled `@pattern` does not admit is refused — including an
//!    expression the boundary validator's own regex engine cannot compile, which it
//!    would otherwise skip and admit. `TemplateName` is this model's one such
//!    expression (`smithy_query(query="shape",shape="TemplateName")`:
//!    `^(?!^\s+$)((?![\x5c'\x2b,;<=>#\x22])([\x20-\x7E]))+$`), and every member
//!    targeting it is subject to it — a template's own `Name` and each entry of a
//!    definition's `SupersededTemplates`
//!    (`smithy_query(query="closure",operation="CreateTemplate")`, shape
//!    `TemplateNameList`).
//! 2. That refusal is stated as this service's validating layer words a schema
//!    finding: HTTP 400 `ValidationException` whose message is the bracketed list
//!    `[ECMA 262 regex "<the expression the model declares>" does not match input
//!    string "<the value the request sent>"]`, one finding per offending value and
//!    nothing else in it — no member path and no error count.
//! 3. Modelled validation runs BEFORE any resource lookup, so a request that is
//!    invalid in itself earns that refusal whether or not the connector it names
//!    exists, and a request whose values are all admitted is refused only for the
//!    resource (the 404 the sibling suite pins).
//!
//! Every row drives the real wire path through [`super::harness`]. The rows span what
//! the expression generalises over: each character its inner negative look-ahead
//! excludes, the all-whitespace value its leading look-ahead excludes, characters
//! outside its `[\x20-\x7E]` class, both `@length` bounds, the member the value is
//! sent for (`Name` and `SupersededTemplates`), each version of the definition union,
//! one and two offending values in a single request, and a connector that exists
//! beside one that does not. No row carries a hand-copied message: each one is
//! computed from the expression this crate compiles.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_pcaconnectorad_pcaconnectorad as pcad;
use pcad::state::TEMPLATE_NAME_PATTERN;

use super::connector_lifecycle::create_connector;
use super::harness::{call, encode_label, router, router_with_engine, ACCOUNT, REGION};

/// The connector the admitted rows are created under: the smallest body
/// `CreateConnector` accepts (`smithy_query(query="operation",
/// operation="CreateConnector")`).
fn connector_body(directory_id: &str) -> Value {
    json!({
        "DirectoryId": directory_id,
        "CertificateAuthorityArn": format!(
            "arn:aws:acm-pca:{REGION}:{ACCOUNT}:certificate-authority/\
             11111111-2222-3333-4444-555555555555"
        ),
        "VpcInformation": { "SecurityGroupIds": ["sg-0123456789abcdef0"] },
    })
}

/// A well-formed connector ARN of this Region and account that no call ever minted,
/// so a request carrying it can only be refused by a lookup.
fn absent_connector_arn() -> String {
    format!("arn:aws:pca-connector-ad:{REGION}:{ACCOUNT}:connector/99999999-8888-7777-6666-555555555555")
}

/// The `CertificateValidity` every definition below carries; nothing in this suite
/// asserts on it.
fn certificate_validity() -> Value {
    json!({
        "ValidityPeriod": { "PeriodType": "YEARS", "Period": 1 },
        "RenewalPeriod": { "PeriodType": "WEEKS", "Period": 6 },
    })
}

/// The `TemplateDefinition` union's three members, each with the `SupersededTemplates`
/// list the row supplies (omitted when it is empty, since the list is modelled
/// `length 1..100`).
fn definition(version: &str, superseded: &[&str]) -> Value {
    let mut inner = match version {
        "TemplateV3" => json!({
            "CertificateValidity": certificate_validity(),
            "PrivateKeyAttributes": {
                "MinimalKeyLength": 2048,
                "KeySpec": "KEY_EXCHANGE",
                "KeyUsageProperty": { "PropertyType": "ALL" },
                "Algorithm": "RSA",
            },
            "PrivateKeyFlags": { "ClientVersion": "WINDOWS_SERVER_2016" },
            "EnrollmentFlags": { "NoSecurityExtension": true },
            "SubjectNameFlags": { "SanRequireDirectoryGuid": true },
            "GeneralFlags": { "MachineType": true },
            "HashAlgorithm": "SHA256",
            "Extensions": { "KeyUsage": { "UsageFlags": { "KeyEncipherment": true } } },
        }),
        // `TemplateV2` and `TemplateV4` declare the same required members (V4's
        // `HashAlgorithm` is optional), so one body satisfies both shapes.
        _ => json!({
            "CertificateValidity": certificate_validity(),
            "PrivateKeyAttributes": { "MinimalKeyLength": 2048, "KeySpec": "KEY_EXCHANGE" },
            "PrivateKeyFlags": { "ClientVersion": "WINDOWS_SERVER_2016" },
            "EnrollmentFlags": { "NoSecurityExtension": true },
            "SubjectNameFlags": { "SanRequireDirectoryGuid": true },
            "GeneralFlags": { "MachineType": true },
            "Extensions": { "KeyUsage": { "UsageFlags": { "KeyEncipherment": true } } },
        }),
    };
    if !superseded.is_empty() {
        inner["SupersededTemplates"] = json!(superseded);
    }
    json!({ version: inner })
}

/// The three versions of the definition union, so every row's rule is asserted over
/// each of them.
const VERSIONS: &[&str] = &["TemplateV2", "TemplateV3", "TemplateV4"];

/// One finding the refusal states, as the validating layer words it: the expression
/// the model declares, anchors included, and the value the request submitted.
fn finding(value: &str) -> String {
    format!("ECMA 262 regex \"{TEMPLATE_NAME_PATTERN}\" does not match input string \"{value}\"")
}

/// The whole message a refusal states: the layer's bracketed list over the findings,
/// in the order the input declares the members that carried them.
fn report(values: &[&str]) -> String {
    let findings: Vec<String> = values.iter().map(|value| finding(value)).collect();
    format!("[{}]", findings.join(", "))
}

/// A name of exactly `length` characters the expression admits, so a row can sit on a
/// `@length` bound without also breaking the grammar.
fn name_of_length(length: usize) -> String {
    "T".repeat(length)
}

/// The values `TemplateName`'s expression does NOT admit, each with why it is here.
///
/// The ten characters its inner negative look-ahead excludes are spelled by the model
/// as `[\x5c'\x2b,;<=>#\x22]` — backslash, apostrophe, plus, comma, semicolon, less
/// than, equals, greater than, hash and double quote — and the leading look-ahead
/// `(?!^\s+$)` excludes a value that is whitespace from end to end. Everything else
/// has to be inside `[\x20-\x7E]`, so a control character, a DEL and a non-ASCII
/// character are outside it too.
fn rejected_names() -> Vec<(&'static str, String)> {
    let mut rows: Vec<(&'static str, String)> = vec![
        ("a comma, the excluded character the recorded case carried", "bad,name".to_string()),
        ("a backslash", "bad\\name".to_string()),
        ("an apostrophe", "bad'name".to_string()),
        ("a plus sign", "bad+name".to_string()),
        ("a semicolon", "bad;name".to_string()),
        ("a less-than sign", "bad<name".to_string()),
        ("an equals sign", "bad=name".to_string()),
        ("a greater-than sign", "bad>name".to_string()),
        ("a hash", "bad#name".to_string()),
        ("a double quote", "bad\"name".to_string()),
        ("several excluded characters at once", "Bad+Name;Not<Allowed>".to_string()),
        ("an excluded character as the whole value", ",".to_string()),
        ("one space, which is whitespace end to end", " ".to_string()),
        ("several spaces", "   ".to_string()),
        ("a tab, outside the printable class and whitespace", "\t".to_string()),
        ("a tab inside an otherwise admitted name", "bad\tname".to_string()),
        ("a newline inside an otherwise admitted name", "bad\nname".to_string()),
        ("DEL, one past the printable class", "bad\u{7f}name".to_string()),
        ("a non-ASCII letter", "schöne-vorlage".to_string()),
        ("an emoji", "template\u{1f600}".to_string()),
    ];
    // The `@length` maximum is 64 characters; a 64-character value that breaks the
    // grammar is refused for the grammar, not for its size.
    rows.push((
        "an excluded character at the @length maximum",
        format!("{},", name_of_length(63)),
    ));
    rows
}

/// The values it DOES admit, so the suite also pins that the check is the model's
/// expression and not a broader refusal: the printable characters it leaves alone
/// (including the `@length` bounds of 1 and 64 characters) must still be accepted.
fn admitted_names() -> Vec<(&'static str, String)> {
    vec![
        ("a plain name", "WebServerTemplate".to_string()),
        ("one character, the @length minimum", name_of_length(1)),
        ("64 characters, the @length maximum", name_of_length(64)),
        ("spaces inside the name", "Web Server Template".to_string()),
        ("a leading space, which is not whitespace end to end", " Leading".to_string()),
        (
            "the printable punctuation the expression leaves alone",
            "!$%&()*-./:?@[]^_`{|}~".to_string(),
        ),
        ("digits only", "20260101".to_string()),
    ]
}

/// `CreateTemplate` with `name`, the definition of `version` superseding
/// `superseded`, against `connector_arn`.
async fn create_template(
    router: &MultiServiceRouter,
    connector_arn: &str,
    name: &str,
    version: &str,
    superseded: &[&str],
) -> (u16, String, Value) {
    let body = json!({
        "ConnectorArn": connector_arn,
        "Name": name,
        "Definition": definition(version, superseded),
    });
    call(router, "POST", "/templates", Some(&body)).await
}

/// Assert one refusal: the status, the code and the whole message.
fn assert_refused(case: &str, answered: (u16, String, Value), expected: &str) {
    let (status, code, parsed) = answered;
    assert_eq!(
        status, 400,
        "case {case:?}: a value the modelled expression does not admit is refused with 400, \
         got {status} / {code:?} / {parsed}"
    );
    assert_eq!(
        code, "ValidationException",
        "case {case:?}: expected ValidationException, got {code:?} / {parsed}"
    );
    assert_eq!(
        parsed["message"].as_str(),
        Some(expected),
        "case {case:?}: expected message {expected:?}, got {parsed}"
    );
}

#[tokio::test]
async fn a_name_outside_the_modelled_expression_is_refused_before_the_connector_is_resolved() {
    let router = router();
    let absent = absent_connector_arn();
    // The parent does not exist, so a refusal here can only come from the request's
    // own values: this is the ordering half of the rule.
    for (case, name) in rejected_names() {
        let answered = create_template(&router, &absent, &name, "TemplateV2", &[]).await;
        assert_refused(case, answered, &report(&[&name]));
    }
}

#[tokio::test]
async fn a_name_outside_the_modelled_expression_is_refused_on_a_connector_that_exists() {
    let router = router();
    let connector = create_connector(&router, &connector_body("d-0123456789")).await;

    for (case, name) in rejected_names() {
        for version in VERSIONS {
            let answered = create_template(&router, &connector, &name, version, &[]).await;
            assert_refused(case, answered, &report(&[&name]));
        }
    }
}

#[tokio::test]
async fn a_superseded_template_name_outside_the_expression_is_refused_too() {
    let router = router();
    let connector = create_connector(&router, &connector_body("d-0123456789")).await;

    for (case, name) in rejected_names() {
        for version in VERSIONS {
            // The template's own name is admitted, so the one finding the message
            // states is about the superseded entry.
            let answered =
                create_template(&router, &connector, "WebServerTemplate", version, &[&name]).await;
            assert_refused(case, answered, &report(&[&name]));
        }
    }
}

#[tokio::test]
async fn every_offending_value_of_one_request_is_listed_in_the_one_report() {
    let router = router();
    let connector = create_connector(&router, &connector_body("d-0123456789")).await;

    // Two and three offending values, over the members that carry them: the findings
    // are listed in the order the input declares those members — the template's own
    // `Name` first, then the superseded entries in list order.
    let rows: Vec<(&str, &str, Vec<&str>, Vec<&str>)> = vec![
        (
            "the name and one superseded entry",
            "bad,name",
            vec!["also;bad"],
            vec!["bad,name", "also;bad"],
        ),
        (
            "the name and two superseded entries",
            "bad<name",
            vec!["also>bad", "and#worse"],
            vec!["bad<name", "also>bad", "and#worse"],
        ),
        (
            "an admitted name beside two offending superseded entries",
            "WebServerTemplate",
            vec!["bad'one", "bad\"two"],
            vec!["bad'one", "bad\"two"],
        ),
        (
            "an offending name beside an admitted superseded entry",
            "bad+name",
            vec!["GoodTemplate"],
            vec!["bad+name"],
        ),
    ];

    for (case, name, superseded, expected) in rows {
        let answered = create_template(&router, &connector, name, "TemplateV2", &superseded).await;
        assert_refused(case, answered, &report(&expected));
    }
}

#[tokio::test]
async fn a_name_the_expression_admits_is_created_and_read_back() {
    let router = router();
    let connector = create_connector(&router, &connector_body("d-0123456789")).await;

    for (case, name) in admitted_names() {
        let (status, code, parsed) =
            create_template(&router, &connector, &name, "TemplateV2", &["GoodTemplate"]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a name the expression admits must be created, \
             got {status} / {code:?} / {parsed}"
        );
        let arn = parsed["TemplateArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: the create answered no TemplateArn: {parsed}"))
            .to_string();

        let (status, code, read) = call(
            &router,
            "GET",
            &format!("/templates/{}", encode_label(&arn)),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetTemplate must read the template back, \
             got {status} / {code:?} / {read}"
        );
        assert_eq!(
            read["Template"]["Name"].as_str(),
            Some(name.as_str()),
            "case {case:?}: the persisted name must be the one submitted, got {read}"
        );
    }
}

/// The ordering rule's other half: a request whose values the expression admits is
/// refused for the RESOURCE it names, in the resource refusal's own class and
/// wording.
#[tokio::test]
async fn an_admitted_name_on_a_connector_that_does_not_exist_is_still_resource_not_found() {
    let router = router();
    let absent = absent_connector_arn();

    for (case, name) in admitted_names() {
        let (status, code, parsed) =
            create_template(&router, &absent, &name, "TemplateV2", &[]).await;
        assert_eq!(
            status, 404,
            "case {case:?}: an admitted name on an absent connector is the resource refusal, \
             got {status} / {code:?} / {parsed}"
        );
        assert_eq!(
            code, "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {code:?} / {parsed}"
        );
        let expected = format!("Connector was not found: {absent}.");
        assert_eq!(
            parsed["Message"].as_str(),
            Some(expected.as_str()),
            "case {case:?}: expected message {expected:?}, got {parsed}"
        );
    }
}

/// A refused request PERSISTS NOTHING: the store holds no template for the connector
/// the refusals named, so the name AWS forbids was never minted.
#[tokio::test]
async fn a_refused_create_records_no_template() {
    let (router, engine) = router_with_engine();
    let connector = create_connector(&router, &connector_body("d-0123456789")).await;

    for (case, name) in rejected_names() {
        let answered = create_template(&router, &connector, &name, "TemplateV2", &[]).await;
        assert_refused(case, answered, &report(&[&name]));
    }

    let shared = pcad::state::service_state(&*engine).expect("the service's own state");
    let stored: Vec<String> = shared
        .lock()
        .templates
        .values()
        .map(|template| template.name.clone())
        .collect();
    assert!(
        stored.is_empty(),
        "no refused create may persist a template, got {stored:?}"
    );
}
