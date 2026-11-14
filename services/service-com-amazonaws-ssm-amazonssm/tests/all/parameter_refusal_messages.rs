// SPDX-License-Identifier: CC0-1.0

//! What Parameter Store's refusals SAY, end to end over the front door.
//!
//! The rule under test: **every Parameter Store refusal answers with the error class the
//! condition belongs to and the sentence the service words that class with — fixed text
//! around variable parts (the parameter name, the version, the label, the rejected filter
//! key or option, the quota) taken from the request and from this store's own state, never
//! from the caller's value or from a constant standing in for state.**
//!
//! Each suite is a TABLE whose rows differ along what the rule generalises over — the name
//! form and depth, the parameter type, the selector kind, the operation that reports the
//! refusal, the filter key and option, the tier — so no row can pass on a sentence keyed to
//! one recorded request. Each suite also drives the SUCCESS beside the refusal (a value the
//! pattern accepts, a filter key the operation takes, a tier that carries policies), so a
//! handler that refused everything would fail it.

use serde_json::{json, Value};

use crate::parameter_store::{call, error_code, router, router_with_kms};

/// The `message` of a failure body, however the service spells the member.
///
/// Parameter Store words most refusals in a modelled `message` and the filter-key refusal
/// in a `Message`; a client reads whichever is there, so the tests do too.
fn error_message(body: &Value) -> String {
    body["message"]
        .as_str()
        .or_else(|| body["Message"].as_str())
        .unwrap_or_default()
        .to_string()
}

/// `(status, code, message)` of one call, which is the whole of what a refusal says.
fn refusal(status: u16, body: &Value) -> (u16, String, String) {
    (status, error_code(body), error_message(body))
}

/// One parameter written through the front door, for a suite that needs a parameter to
/// refuse something about.
async fn write(router: &smithy_server_runtime::MultiServiceRouter, body: &Value) -> i64 {
    let (status, written) = call(router, "PutParameter", body).await;
    assert_eq!(
        status, 200,
        "expected {body} to be written, got {status} {written}"
    );
    written["Version"].as_i64().unwrap_or_default()
}

// ---------------------------------------------------------------------------
// A put against a name that already exists
// ---------------------------------------------------------------------------

/// The sentence a put against an existing name without `Overwrite` is refused with. It is
/// wholly fixed: it names the REQUEST MEMBER to set, and no parameter.
const ALREADY_EXISTS: &str = "The parameter already exists. To overwrite this value, set \
                              the overwrite option in the request to true.";

/// Whatever the name's form or the parameter's type, the second put is refused with the
/// one sentence — and an overwrite of the same name is served.
#[tokio::test]
async fn an_existing_name_without_overwrite_names_the_overwrite_option() {
    let router = router_with_kms();
    /// One row: how the name is spelled, and the type it is created with.
    type Row<'a> = (&'a str, &'a str, &'a str);
    let rows: Vec<Row<'_>> = vec![
        ("a one-level name", "already-plain", "String"),
        ("a one-level name with its slash", "/already-slashed", "String"),
        ("a two-level hierarchy", "/already/two", "StringList"),
        ("a deep hierarchy", "/already/a/b/c/d", "String"),
        ("a sealed value", "/already/secure", "SecureString"),
    ];
    for (case, name, type_) in rows {
        write(
            &router,
            &json!({ "Name": name, "Type": type_, "Value": "first" }),
        )
        .await;
        let (status, body) = call(
            &router,
            "PutParameter",
            &json!({ "Name": name, "Type": type_, "Value": "second" }),
        )
        .await;
        assert_eq!(
            refusal(status, &body),
            (
                400,
                "ParameterAlreadyExists".to_string(),
                ALREADY_EXISTS.to_string()
            ),
            "case {case:?} ({name}): expected the fixed already-exists sentence, got \
             {status} {body}"
        );
        assert!(
            !error_message(&body).contains(name.trim_start_matches('/')),
            "case {case:?}: the sentence names no parameter, got {}",
            error_message(&body)
        );
        // The same put WITH the flag the sentence names is served.
        let version = write(
            &router,
            &json!({ "Name": name, "Value": "second", "Overwrite": true }),
        )
        .await;
        assert_eq!(
            version, 2,
            "case {case:?}: the overwrite the sentence asks for appends a version, got \
             {version}"
        );
    }
}

// ---------------------------------------------------------------------------
// A value against the request's AllowedPattern
// ---------------------------------------------------------------------------

/// The sentence a value that fails a well-formed `AllowedPattern` is refused with, around
/// the request's own pattern.
fn pattern_mismatch(pattern: &str) -> String {
    format!("Parameter value, cannot be validated against allowedPattern: {pattern}")
}

/// A value outside the pattern is the VALUE's fault — `ParameterPatternMismatchException` —
/// for every type, and a `StringList`'s items are validated one by one. A pattern that is
/// no regular expression is the PATTERN's fault, which is the other class.
#[tokio::test]
async fn a_value_outside_the_allowed_pattern_is_a_pattern_mismatch() {
    let router = router_with_kms();
    /// One row: the type, the pattern, the value, and whether the put is served.
    type Row<'a> = (&'a str, &'a str, &'a str, &'a str, bool);
    let rows: Vec<Row<'_>> = vec![
        ("a String that matches", "String", r"^[0-9]+$", "1234", true),
        ("a String that does not", "String", r"^[0-9]+$", "12a4", false),
        ("a String refused in full", "String", r"[0-9]{3}", "1234", false),
        // "Items in a StringList must be separated by a comma (,)"
        // (`smithy_query(query="doc", shape="PutParameterRequest", member="Type")`), so the
        // pattern holds of each ITEM rather than of the comma-joined whole.
        (
            "a StringList whose every item matches",
            "StringList",
            r"^[0-9]+$",
            "1,22,333",
            true,
        ),
        (
            "a StringList with one item outside",
            "StringList",
            r"^[0-9]+$",
            "1,x,333",
            false,
        ),
        (
            "a StringList whose items are letters",
            "StringList",
            r"^[a-z]+$",
            "dev,prod",
            true,
        ),
        // A `SecureString` is matched on the plaintext the request carried, before it is
        // sealed.
        (
            "a SecureString plaintext that matches",
            "SecureString",
            r"^[0-9]+$",
            "9090",
            true,
        ),
        (
            "a SecureString plaintext that does not",
            "SecureString",
            r"^[0-9]+$",
            "secret",
            false,
        ),
    ];
    for (index, (case, type_, pattern, value, served)) in rows.iter().enumerate() {
        let name = format!("/pattern/case{index}");
        let (status, body) = call(
            &router,
            "PutParameter",
            &json!({
                "Name": name,
                "Type": type_,
                "Value": value,
                "AllowedPattern": pattern,
            }),
        )
        .await;
        if *served {
            assert_eq!(
                status, 200,
                "case {case:?}: the value satisfies {pattern}, got {status} {body}"
            );
            let (status, read) = call(&router, "GetParameter", &json!({ "Name": name })).await;
            assert_eq!(
                (status, read.pointer("/Parameter/Name").and_then(Value::as_str)),
                (200, Some(name.as_str())),
                "case {case:?}: the accepted value round-trips, got {status} {read}"
            );
        } else {
            assert_eq!(
                refusal(status, &body),
                (
                    400,
                    "ParameterPatternMismatchException".to_string(),
                    pattern_mismatch(pattern)
                ),
                "case {case:?}: expected the value to be refused as a pattern mismatch, got \
                 {status} {body}"
            );
        }
    }

    // A pattern that is not a regular expression at all is the OTHER class: the caller's
    // own pattern is what is wrong, not its value.
    let (status, body) = call(
        &router,
        "PutParameter",
        &json!({
            "Name": "/pattern/malformed",
            "Type": "String",
            "Value": "anything",
            "AllowedPattern": "^([0-9+$",
        }),
    )
    .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "InvalidAllowedPatternException"),
        "case a malformed pattern: expected the pattern itself to be refused, got {status} \
         {body}"
    );
}

/// An overwrite refused for its value leaves the parameter exactly as it was: no version
/// is burned by a put that never stored anything.
#[tokio::test]
async fn an_overwrite_refused_for_its_value_burns_no_version() {
    let router = router();
    let name = "/pattern/overwrite";
    write(
        &router,
        &json!({
            "Name": name,
            "Type": "String",
            "Value": "100",
            "AllowedPattern": r"^[0-9]+$",
        }),
    )
    .await;
    let (status, body) = call(
        &router,
        "PutParameter",
        &json!({
            "Name": name,
            "Value": "not-a-number",
            "AllowedPattern": r"^[0-9]+$",
            "Overwrite": true,
        }),
    )
    .await;
    assert_eq!(
        refusal(status, &body),
        (
            400,
            "ParameterPatternMismatchException".to_string(),
            pattern_mismatch(r"^[0-9]+$")
        ),
        "expected the overwrite to be refused as a pattern mismatch, got {status} {body}"
    );
    let (status, read) = call(&router, "GetParameter", &json!({ "Name": name })).await;
    assert_eq!(
        (
            status,
            read.pointer("/Parameter/Version").and_then(Value::as_i64),
            read.pointer("/Parameter/Value").and_then(Value::as_str),
        ),
        (200, Some(1), Some("100")),
        "the refused overwrite stored nothing and burned no version, got {status} {read}"
    );
}

// ---------------------------------------------------------------------------
// A selector that resolves to no version
// ---------------------------------------------------------------------------

/// The sentence a selector naming a version the parameter does not carry is refused with.
fn version_not_found(name: &str, version: i64) -> String {
    format!("Systems Manager could not find version {version} of {name}. Verify the version and try again.")
}

/// The sentence a selector naming a label no version carries is refused with.
fn label_not_found(name: &str, label: &str) -> String {
    format!(
        "Systems Manager could not find a version of {name} that has the specified label, \
         {label}. Verify the parameter label and try again."
    )
}

/// A `name:selector` read that resolves to nothing names the version it could not find, or
/// the label — whichever the caller asked for — and quotes the parameter back by its own
/// name: a top-level name carries no leading separator, a hierarchical one its whole path.
#[tokio::test]
async fn a_selector_that_resolves_to_nothing_names_the_version_or_the_label() {
    let router = router();
    /// One row: how the name is spelled, and the name the sentence quotes back.
    type Row<'a> = (&'a str, &'a str, &'a str);
    let rows: Vec<Row<'_>> = vec![
        ("a one-level name", "selector-plain", "selector-plain"),
        (
            "a one-level name with its slash",
            "/selector-slashed",
            "selector-slashed",
        ),
        ("a two-level hierarchy", "/selector/two", "/selector/two"),
        (
            "a deep hierarchy",
            "/selector/a/b/c/d",
            "/selector/a/b/c/d",
        ),
    ];
    for (case, name, rendered) in rows {
        write(
            &router,
            &json!({ "Name": name, "Type": "String", "Value": "one" }),
        )
        .await;
        write(
            &router,
            &json!({ "Name": name, "Value": "two", "Overwrite": true }),
        )
        .await;

        // Every version number the parameter does not carry, including the one just past
        // its newest and one far beyond it.
        for version in [0_i64, 3, 99] {
            let (status, body) = call(
                &router,
                "GetParameter",
                &json!({ "Name": format!("{name}:{version}") }),
            )
            .await;
            assert_eq!(
                refusal(status, &body),
                (
                    400,
                    "ParameterVersionNotFound".to_string(),
                    version_not_found(rendered, version)
                ),
                "case {case:?} version {version}: expected the version form of the refusal, \
                 got {status} {body}"
            );
        }

        // A selector that is no version number is a label, and the label form says so.
        for label in ["no-such-label", "prod"] {
            let (status, body) = call(
                &router,
                "GetParameter",
                &json!({ "Name": format!("{name}:{label}") }),
            )
            .await;
            assert_eq!(
                refusal(status, &body),
                (
                    400,
                    "ParameterVersionNotFound".to_string(),
                    label_not_found(rendered, label)
                ),
                "case {case:?} label {label}: expected the label form of the refusal, got \
                 {status} {body}"
            );
        }

        // The version the parameter DOES carry still reads back, so no row passes on a
        // read that refuses every selector.
        let (status, read) = call(
            &router,
            "GetParameter",
            &json!({ "Name": format!("{name}:1") }),
        )
        .await;
        assert_eq!(
            (status, read.pointer("/Parameter/Value").and_then(Value::as_str)),
            (200, Some("one")),
            "case {case:?}: the version that exists still reads back, got {status} {read}"
        );

        // The label operations report the same sentence for a version they cannot find.
        for operation in ["LabelParameterVersion", "UnlabelParameterVersion"] {
            let (status, body) = call(
                &router,
                operation,
                &json!({ "Name": name, "ParameterVersion": 42, "Labels": ["stable"] }),
            )
            .await;
            assert_eq!(
                refusal(status, &body),
                (
                    400,
                    "ParameterVersionNotFound".to_string(),
                    version_not_found(rendered, 42)
                ),
                "case {case:?} {operation}: expected the version form of the refusal, got \
                 {status} {body}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Labelling a name that holds no parameter
// ---------------------------------------------------------------------------

/// `LabelParameterVersion` on a name the store holds no parameter for names the name the
/// request asked to label — while a READ of the same name answers the code alone, which is
/// the asymmetry the service really has.
#[tokio::test]
async fn labelling_a_name_that_holds_no_parameter_names_it() {
    let router = router();
    let rows: Vec<(&str, &str)> = vec![
        ("a one-level name", "label-missing"),
        ("a one-level name with its slash", "/label-missing"),
        ("a two-level hierarchy", "/label/missing"),
        ("a deep hierarchy", "/label/a/b/c/d"),
    ];
    for (case, name) in &rows {
        let (status, body) = call(
            &router,
            "LabelParameterVersion",
            &json!({ "Name": name, "Labels": ["stable"] }),
        )
        .await;
        assert_eq!(
            refusal(status, &body),
            (
                400,
                "ParameterNotFound".to_string(),
                format!("Parameter {name} not found.")
            ),
            "case {case:?}: expected the label refusal to name {name}, got {status} {body}"
        );
        let (status, read) = call(&router, "GetParameter", &json!({ "Name": name })).await;
        assert_eq!(
            refusal(status, &read),
            (400, "ParameterNotFound".to_string(), String::new()),
            "case {case:?}: a READ of the same name still carries no message, got {status} \
             {read}"
        );
    }

    // A name the store DOES hold is labelled, so no row passes against a handler that
    // refuses every label call.
    let held = "/label/present";
    write(
        &router,
        &json!({ "Name": held, "Type": "String", "Value": "here" }),
    )
    .await;
    let (status, labelled) = call(
        &router,
        "LabelParameterVersion",
        &json!({ "Name": held, "Labels": ["stable"] }),
    )
    .await;
    assert_eq!(
        (
            status,
            labelled["ParameterVersion"].as_i64(),
            labelled["InvalidLabels"].as_array().map(Vec::len)
        ),
        (200, Some(1), Some(0)),
        "expected the held parameter to be labelled, got {status} {labelled}"
    );
    // And the deleted name refuses the same way a name nobody wrote does.
    let (status, deleted) = call(&router, "DeleteParameter", &json!({ "Name": held })).await;
    assert_eq!(
        status, 200,
        "expected the held parameter to be deleted, got {status} {deleted}"
    );
    let (status, body) = call(
        &router,
        "LabelParameterVersion",
        &json!({ "Name": held, "Labels": ["stable"] }),
    )
    .await;
    assert_eq!(
        refusal(status, &body),
        (
            400,
            "ParameterNotFound".to_string(),
            format!("Parameter {held} not found.")
        ),
        "expected labelling the deleted name to name it, got {status} {body}"
    );
}

// ---------------------------------------------------------------------------
// The filters the two filter-taking reads accept
// ---------------------------------------------------------------------------

/// The key vocabulary each operation prints when it rejects one, in the order it prints
/// them. Source: the `ParameterStringFilter$Key` doc
/// (`smithy_query(query="doc", shape="ParameterStringFilter", member="Key")`): every
/// pattern except `Label` for a describe, and only `Type`, `KeyId` and `Label` for a
/// hierarchy read.
const DESCRIBE_KEYS: &str = "Path, Name, Type, KeyId, Tier, DataType";

/// See [`DESCRIBE_KEYS`].
const BY_PATH_KEYS: &str = "Type, KeyId, Label";

/// A filter key the operation does not accept is refused with a sentence naming the key and
/// THAT operation's legal keys — which differ between the two operations.
#[tokio::test]
async fn a_rejected_filter_key_lists_the_operations_own_keys() {
    let router = router();
    write(
        &router,
        &json!({ "Name": "/filters/one", "Type": "String", "Value": "here" }),
    )
    .await;

    /// One row: the operation, the rejected key, and the list it prints.
    type Row<'a> = (&'a str, &'a str, &'a str);
    let rows: Vec<Row<'_>> = vec![
        // `Label` is the one key a describe refuses; a hierarchy read takes it. Any key
        // outside the modelled `Key` pattern never reaches the handler at all.
        ("DescribeParameters", "Label", DESCRIBE_KEYS),
        // Each of these a describe takes and a hierarchy read refuses.
        ("GetParametersByPath", "Name", BY_PATH_KEYS),
        ("GetParametersByPath", "Path", BY_PATH_KEYS),
        ("GetParametersByPath", "Tier", BY_PATH_KEYS),
        ("GetParametersByPath", "DataType", BY_PATH_KEYS),
        ("GetParametersByPath", "tag:owner", BY_PATH_KEYS),
    ];
    for (operation, key, listed) in rows {
        let mut request = json!({
            "ParameterFilters": [{ "Key": key, "Values": ["anything"] }],
        });
        if operation == "GetParametersByPath" {
            request["Path"] = json!("/filters");
        }
        let (status, body) = call(&router, operation, &request).await;
        assert_eq!(
            refusal(status, &body),
            (
                400,
                "InvalidFilterKey".to_string(),
                format!("The following filter key is not valid: {key}. Valid filter keys include: [{listed}].")
            ),
            "case {operation} with key {key:?}: expected the refusal to list that \
             operation's keys, got {status} {body}"
        );
    }

    // A key each operation DOES accept is served, so no row passes against a handler that
    // refuses every filter.
    let (status, described) = call(
        &router,
        "DescribeParameters",
        &json!({ "ParameterFilters": [{ "Key": "Type", "Values": ["String"] }] }),
    )
    .await;
    assert_eq!(
        (status, described["Parameters"].as_array().map(Vec::len)),
        (200, Some(1)),
        "expected a legal describe filter to be served, got {status} {described}"
    );
    let (status, listed) = call(
        &router,
        "GetParametersByPath",
        &json!({
            "Path": "/filters",
            "ParameterFilters": [{ "Key": "Type", "Values": ["String"] }],
        }),
    )
    .await;
    assert_eq!(
        (status, listed["Parameters"].as_array().map(Vec::len)),
        (200, Some(1)),
        "expected a legal hierarchy filter to be served, got {status} {listed}"
    );
}

/// A filter option a key does not accept is refused with a sentence naming the option and
/// THAT key's legal options. Source: the `ParameterStringFilter$Option` doc — `Equals` and
/// `BeginsWith` in general, `Contains` additionally for `Name`, `Recursive` and `OneLevel`
/// for `Path`, and `Equals` alone for `Label`.
#[tokio::test]
async fn a_rejected_filter_option_lists_the_keys_own_options() {
    let router = router();
    write(
        &router,
        &json!({ "Name": "/options/one", "Type": "String", "Value": "here" }),
    )
    .await;

    /// One row: the operation, the key, the rejected option, and the list it prints.
    type Row<'a> = (&'a str, &'a str, &'a str, &'a str);
    let rows: Vec<Row<'_>> = vec![
        (
            "GetParametersByPath",
            "Label",
            "BeginsWith",
            "Equals",
        ),
        (
            "GetParametersByPath",
            "Type",
            "Contains",
            "Equals, BeginsWith",
        ),
        (
            "DescribeParameters",
            "Path",
            "Equals",
            "Recursive, OneLevel",
        ),
        (
            "DescribeParameters",
            "Name",
            "Recursive",
            "Equals, BeginsWith, Contains",
        ),
        (
            "DescribeParameters",
            "Tier",
            "Contains",
            "Equals, BeginsWith",
        ),
    ];
    for (operation, key, option, listed) in rows {
        let mut request = json!({
            "ParameterFilters": [{ "Key": key, "Option": option, "Values": ["anything"] }],
        });
        if operation == "GetParametersByPath" {
            request["Path"] = json!("/options");
        }
        let (status, body) = call(&router, operation, &request).await;
        assert_eq!(
            refusal(status, &body),
            (
                400,
                "InvalidFilterOption".to_string(),
                format!("The following filter option is not valid: {option}. Valid options include: [{listed}].")
            ),
            "case {operation} with {key:?}/{option:?}: expected the refusal to list that \
             key's options, got {status} {body}"
        );
    }

    // An option the key DOES accept is served.
    let (status, described) = call(
        &router,
        "DescribeParameters",
        &json!({
            "ParameterFilters": [{ "Key": "Name", "Option": "Contains", "Values": ["options"] }],
        }),
    )
    .await;
    assert_eq!(
        (status, described["Parameters"].as_array().map(Vec::len)),
        (200, Some(1)),
        "expected a legal filter option to be served, got {status} {described}"
    );
}

// ---------------------------------------------------------------------------
// The version ceiling, the label quota, the tier ceilings
// ---------------------------------------------------------------------------

/// How many versions of one parameter are retained (`ParameterMaxVersionLimitExceeded`:
/// "Parameter Store retains the 100 most recently created versions of a parameter").
const MAX_VERSIONS: i64 = 100;

/// An overwrite that would displace a LABELLED oldest version is refused with a sentence
/// naming the version whose label is in the way — recomputed from the store, so moving the
/// label moves the version the sentence names.
#[tokio::test]
async fn the_version_ceiling_names_the_oldest_labelled_version() {
    let router = router();
    let name = "/ceiling/versions";
    write(
        &router,
        &json!({ "Name": name, "Type": "String", "Value": "v1" }),
    )
    .await;
    for version in 2..=MAX_VERSIONS {
        write(
            &router,
            &json!({ "Name": name, "Value": format!("v{version}"), "Overwrite": true }),
        )
        .await;
    }

    /// The sentence, around the name with its leading separator dropped and the oldest
    /// version the store holds.
    fn ceiling(name: &str, oldest: i64) -> String {
        format!(
            "You attempted to create a new version of {name} by calling the PutParameter \
             API with the overwrite flag. Version {oldest}, the oldest version, can\u{2019}t \
             be deleted because it has a label associated with it. Move the label to \
             another version of the parameter, and try again."
        )
    }

    // With no label in the way the ceiling simply displaces the oldest version.
    write(
        &router,
        &json!({ "Name": name, "Value": "v101", "Overwrite": true }),
    )
    .await;

    // Label the oldest version the store now holds, and the next overwrite is refused
    // naming exactly that version.
    for oldest in [2_i64, 3] {
        let (status, labelled) = call(
            &router,
            "LabelParameterVersion",
            &json!({ "Name": name, "ParameterVersion": oldest, "Labels": ["keep"] }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case oldest {oldest}: expected the label to attach, got {status} {labelled}"
        );
        let (status, body) = call(
            &router,
            "PutParameter",
            &json!({ "Name": name, "Value": "blocked", "Overwrite": true }),
        )
        .await;
        assert_eq!(
            refusal(status, &body),
            (
                400,
                "ParameterMaxVersionLimitExceeded".to_string(),
                // This refusal drops the leading separator even from a hierarchical
                // name, where the version refusal below keeps it.
                ceiling("ceiling/versions", oldest)
            ),
            "case oldest {oldest}: expected the refusal to name the labelled oldest \
             version, got {status} {body}"
        );
        // Move the label off it, as the sentence instructs: the put is served, the
        // version it refused to displace is gone, and the next oldest is the next case.
        let (status, moved) = call(
            &router,
            "LabelParameterVersion",
            &json!({ "Name": name, "ParameterVersion": MAX_VERSIONS, "Labels": ["keep"] }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case oldest {oldest}: expected the label to move, got {status} {moved}"
        );
        write(
            &router,
            &json!({ "Name": name, "Value": "served", "Overwrite": true }),
        )
        .await;
        let (status, gone) = call(
            &router,
            "GetParameter",
            &json!({ "Name": format!("{name}:{oldest}") }),
        )
        .await;
        assert_eq!(
            refusal(status, &gone),
            (
                400,
                "ParameterVersionNotFound".to_string(),
                version_not_found(name, oldest)
            ),
            "case oldest {oldest}: the displaced version is gone, got {status} {gone}"
        );
    }
}

/// How many labels one version carries (`Labels` is `@length 1..10`:
/// `smithy_query(query="operation", operation="LabelParameterVersion")`).
const MAX_LABELS: usize = 10;

/// A label past the per-version quota is refused with the fixed sentence carrying that
/// quota, whichever way the labels were put on.
#[tokio::test]
async fn the_label_quota_refusal_carries_the_quota() {
    let router = router();
    let quota = format!(
        "A parameter version can have maximum {MAX_LABELS} labels.Move one or more labels \
         to another version and try again."
    );
    // One row: how the first ten labels are attached — all in one call, or one per call.
    let rows: Vec<(&str, usize)> = vec![("ten in one call", MAX_LABELS), ("one per call", 1)];
    for (case, per_call) in rows {
        let name = format!("/labels/{}", case.replace(' ', "-"));
        write(
            &router,
            &json!({ "Name": name, "Type": "String", "Value": "here" }),
        )
        .await;
        let labels: Vec<String> = (0..MAX_LABELS).map(|index| format!("l{index}")).collect();
        for chunk in labels.chunks(per_call) {
            let (status, body) = call(
                &router,
                "LabelParameterVersion",
                &json!({ "Name": name, "Labels": chunk }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected {chunk:?} to attach, got {status} {body}"
            );
        }
        let (status, body) = call(
            &router,
            "LabelParameterVersion",
            &json!({ "Name": name, "Labels": ["one-too-many"] }),
        )
        .await;
        assert_eq!(
            refusal(status, &body),
            (
                400,
                "ParameterVersionLabelLimitExceeded".to_string(),
                quota.clone()
            ),
            "case {case:?}: expected the quota sentence, got {status} {body}"
        );
        // A label the version already holds is not an eleventh, so it is still served.
        let (status, again) = call(
            &router,
            "LabelParameterVersion",
            &json!({ "Name": name, "Labels": ["l0"] }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: re-attaching a held label stays within the quota, got {status} \
             {again}"
        );
    }
}

/// The published content-size limits of the two tiers (`PutParameterRequest$Tier`:
/// "Standard parameters have a content size limit of 4 KB and ... Advanced parameters
/// have a content size limit of 8 KB").
const STANDARD_LIMIT: usize = 4 * 1024;

/// See [`STANDARD_LIMIT`].
const ADVANCED_LIMIT: usize = 8 * 1024;

/// The page a tier refusal points the caller at.
const TIER_GUIDE: &str = "https://docs.aws.amazon.com/systems-manager/latest/userguide/parameter-store-advanced-parameters.html";

/// A value past the tier's content size limit is refused with that tier's sentence: the
/// standard tier's offers the upgrade that lifts the ceiling, the advanced tier's — which
/// no upgrade lifts — names the value as what was too large, and each limit is the
/// published one. Neither sentence echoes the size the request carried, and a refused put
/// leaves no parameter behind.
#[tokio::test]
async fn the_tier_ceiling_refusal_offers_the_upgrade() {
    let router = router();
    let standard = format!(
        "Standard tier parameters support a maximum parameter value of {STANDARD_LIMIT} \
         characters. To create a larger parameter value, upgrade the parameter to use the \
         advanced-parameter tier. For more information, see {TIER_GUIDE}"
    );
    let advanced = format!(
        "The specified parameter value is too large. Advanced-tier parameters support a \
         maximum parameter value of {ADVANCED_LIMIT} characters."
    );

    /// One row: the tier the request names, the value's size, and the sentence (or `None`
    /// where the value is within the limit).
    type Row<'a> = (&'a str, Option<&'a str>, usize, Option<&'a String>);
    let rows: Vec<Row<'_>> = vec![
        ("the standard limit itself", None, STANDARD_LIMIT, None),
        (
            "one character past the standard limit",
            None,
            STANDARD_LIMIT + 1,
            Some(&standard),
        ),
        (
            "an explicitly standard parameter past it",
            Some("Standard"),
            STANDARD_LIMIT + 900,
            Some(&standard),
        ),
        (
            "an advanced parameter past the standard limit",
            Some("Advanced"),
            STANDARD_LIMIT + 1,
            None,
        ),
        (
            "the advanced limit itself",
            Some("Advanced"),
            ADVANCED_LIMIT,
            None,
        ),
        (
            "one character past the advanced limit",
            Some("Advanced"),
            ADVANCED_LIMIT + 1,
            Some(&advanced),
        ),
        (
            "far past the advanced limit",
            Some("Advanced"),
            ADVANCED_LIMIT * 2,
            Some(&advanced),
        ),
        (
            "intelligent tiering past the advanced limit",
            Some("Intelligent-Tiering"),
            ADVANCED_LIMIT + 1,
            Some(&advanced),
        ),
    ];
    for (index, (case, tier, size, expected)) in rows.iter().enumerate() {
        let name = format!("/tiers/case{index}");
        let mut request = json!({ "Name": name, "Type": "String", "Value": "v".repeat(*size) });
        if let Some(tier) = tier {
            request["Tier"] = json!(tier);
        }
        let (status, body) = call(&router, "PutParameter", &request).await;
        match expected {
            None => assert_eq!(
                status, 200,
                "case {case:?}: the value is within the limit, got {status} {body}"
            ),
            Some(sentence) => {
                assert_eq!(
                    refusal(status, &body),
                    (400, "ValidationException".to_string(), (*sentence).clone()),
                    "case {case:?}: expected the tier's own ceiling sentence, got {status} \
                     {body}"
                );
                // The sentence is the tier's, not the request's: the size the caller sent
                // is nowhere in it, and the refused put stored nothing.
                let sent = size.to_string();
                assert!(
                    !error_message(&body).contains(&sent),
                    "case {case:?}: the sentence echoes the request's size {sent}, got {}",
                    error_message(&body)
                );
                let (status, read) = call(&router, "GetParameter", &json!({ "Name": name })).await;
                assert_eq!(
                    (status, error_code(&read).as_str()),
                    (400, "ParameterNotFound"),
                    "case {case:?}: the refused put left a parameter behind, got {status} \
                     {read}"
                );
            }
        }
    }
}

/// A `Policies` document on a parameter that is not advanced is refused with the fixed
/// sentence that says which tier carries policies — and the advanced tier carries them.
#[tokio::test]
async fn policies_outside_the_advanced_tier_name_the_upgrade() {
    let router = router();
    let sentence = format!(
        "You can't assign parameter policies to a standard-parameter tier. You must \
         upgrade the parameter to use the advanced-parameter tier. For more information, \
         see {TIER_GUIDE}"
    );
    let policies = json!([{ "Type": "Expiration", "Version": "1.0",
                            "Attributes": { "Timestamp": "2035-01-01T00:00:00.000Z" } }])
    .to_string();

    // One row: the tier the request names, and whether the policies are accepted.
    let rows: Vec<(&str, Option<&str>, bool)> = vec![
        ("no tier at all", None, false),
        ("an explicitly standard parameter", Some("Standard"), false),
        ("an advanced parameter", Some("Advanced"), true),
    ];
    for (index, (case, tier, accepted)) in rows.iter().enumerate() {
        let name = format!("/policies/case{index}");
        let mut request = json!({
            "Name": name,
            "Type": "String",
            "Value": "guarded",
            "Policies": policies,
        });
        if let Some(tier) = tier {
            request["Tier"] = json!(tier);
        }
        let (status, body) = call(&router, "PutParameter", &request).await;
        if *accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: the advanced tier carries policies, got {status} {body}"
            );
            let (status, read) = call(&router, "GetParameter", &json!({ "Name": name })).await;
            assert_eq!(
                (status, read.pointer("/Parameter/Value").and_then(Value::as_str)),
                (200, Some("guarded")),
                "case {case:?}: the policy-carrying parameter round-trips, got {status} \
                 {read}"
            );
        } else {
            assert_eq!(
                refusal(status, &body),
                (400, "ValidationException".to_string(), sentence.clone()),
                "case {case:?}: expected the upgrade sentence, got {status} {body}"
            );
        }
    }
}
