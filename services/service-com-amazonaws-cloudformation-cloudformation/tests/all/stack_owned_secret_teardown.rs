// SPDX-License-Identifier: CC0-1.0

//! What a stack's DELETION leaves of the secret it created, end to end over the wire.
//!
//! RULE: **a stack's deletion REMOVES the secret it created from AWS Secrets Manager with no
//! recovery window**, so once the stack reports `DELETE_COMPLETE` that service holds no such
//! secret at all — every read of it is refused as a secret that does not exist, whether it is
//! taken through Secrets Manager itself or through the reserved Parameter Store hierarchy
//! that resolves a name through to it (`/aws/reference/secretsmanager/<secret-name>`). Up to
//! that point the secret is a secret like any other: the reference serves its `AWSCURRENT`
//! value, as a `SecureString` parameter of the SECRET's identity.
//!
//! A secret left merely scheduled for deletion would fail that rule in a way a client sees:
//! it still answers `DescribeSecret`, it still occupies its name, and it is still something
//! the reference resolves to — so a teardown could never be verified, and a redeployment of
//! the same template would collide with a secret the stack was reported as having deleted.
//!
//! The rows differ along what the rule generalises over: which form the declaration states
//! the value in (verbatim, or generated into a declared JSON document), the SHAPE of the
//! secret's name (flat, a hierarchy with separators, one carrying the other characters
//! Secrets Manager admits) and whether the name is the declaration's own or the one
//! CloudFormation generated for a declaration that states none.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::dynamic_references::ssm;
use crate::stack_lifecycle::{
    call, create_stack, describe_stack, member, output_value, router, wait_for_the_transition,
    xml_text, REGION,
};

/// The stack every row is deployed in: one teardown is what the rule is about, so the rows
/// are resources of ONE stack rather than a stack each.
const STACK: &str = "secret-reference-teardown";

/// The reserved Parameter Store hierarchy a secret is read through, as AWS publishes it.
const REFERENCE_ROOT: &str = "/aws/reference/secretsmanager";

/// AWS Secrets Manager's `X-Amz-Target` prefix, from its model's service shape
/// (`com.amazonaws.secretsmanager#secretsmanager`), which speaks awsJson1_1.
const SECRETSMANAGER_TARGET_PREFIX: &str = "secretsmanager";

/// The error code Secrets Manager refuses a read of a secret it does not hold with
/// (`ResourceNotFoundException`, that service's own modelled refusal).
const SECRET_NOT_FOUND: &str = "ResourceNotFoundException";

/// The error code Parameter Store refuses a read of a name it resolves no parameter for with
/// (`ParameterNotFound`, Systems Manager's own modelled refusal of `GetParameter`).
const PARAMETER_NOT_FOUND: &str = "ParameterNotFound";

/// The message that refusal carries for a read of the reserved hierarchy: it names the
/// reference canonically (no leading `/`) and carries Secrets Manager's own sentence for a
/// secret it holds none of.
fn expected_reference_miss(name: &str) -> String {
    format!(
        "An error occurred ({PARAMETER_NOT_FOUND}) when referencing Secrets Manager: Secret \
         {}/{name} not found. Secrets Manager can't find the specified secret.",
        REFERENCE_ROOT.trim_start_matches('/')
    )
}

/// The `Type` a reference is reported as: the value is one a key of the account protects.
const SECURE_STRING: &str = "SecureString";

/// How many reads a poll takes before it gives up — generous, because what it waits for is a
/// CONDITION (the teardown reaching a terminal status) and not a span.
const POLL_ATTEMPTS: usize = 20;

/// How a row's declaration states the secret's value, and so what the reference must serve.
#[derive(Debug, Clone, Copy)]
enum Expected {
    /// `SecretString`: the value the declaration stated, which the reference serves byte for
    /// byte.
    Verbatim(&'static str),
    /// `GenerateSecretString` with a `SecretStringTemplate` and a `GenerateStringKey`: the
    /// declared document, with a generated password of the declared length put in under that
    /// key.
    Generated {
        /// A member of the declared document, which the generation leaves as declared.
        username: &'static str,
        /// The key the generated password is put under.
        key: &'static str,
        /// The `PasswordLength` the declaration asked for.
        length: usize,
    },
}

/// One declared secret of the stack.
#[derive(Debug, Clone, Copy)]
struct Row {
    case: &'static str,
    /// The logical id the template declares it under, which also names its output.
    logical_id: &'static str,
    /// The `Name` the declaration states, or `""` for the row that states none and so is
    /// created under the name CloudFormation generates.
    declared_name: &'static str,
    /// The declaration's `Properties`, as the JSON text the template carries.
    properties: &'static str,
    expected: Expected,
}

const ROWS: &[Row] = &[
    Row {
        // The recorded shape: a credential document stated verbatim, under a flat name.
        case: "a value stated verbatim, under a flat name",
        logical_id: "Verbatim",
        declared_name: "secret-reference-teardown-secret",
        properties: r#"{
            "Name": "secret-reference-teardown-secret",
            "Description": "the value the declaration states itself",
            "SecretString": "{\"username\":\"probeadmin\",\"password\":\"a-declared-example\"}"
        }"#,
        expected: Expected::Verbatim(
            "{\"username\":\"probeadmin\",\"password\":\"a-declared-example\"}",
        ),
    },
    Row {
        // A name in a HIERARCHY: a secret's name may carry separators, and the whole level
        // below the reserved hierarchy is the secret's id.
        case: "a generated password in a declared document, under a hierarchical name",
        logical_id: "Hierarchical",
        declared_name: "teardown/team/db-credentials",
        properties: r#"{
            "Name": "teardown/team/db-credentials",
            "GenerateSecretString": {
                "SecretStringTemplate": "{\"username\":\"probeadmin\"}",
                "GenerateStringKey": "password",
                "PasswordLength": 24
            }
        }"#,
        expected: Expected::Generated {
            username: "probeadmin",
            key: "password",
            length: 24,
        },
    },
    Row {
        // The other characters Secrets Manager admits in a name ("/_+=.@-"), on a plain-text
        // value rather than a document.
        case: "a plain-text value, under a name of the other admitted characters",
        logical_id: "Dotted",
        declared_name: "teardown.api_token+v2",
        properties: r#"{
            "Name": "teardown.api_token+v2",
            "SecretString": "a-plain-text-example-value"
        }"#,
        expected: Expected::Verbatim("a-plain-text-example-value"),
    },
    Row {
        // No `Name` at all: the secret is created under the name CloudFormation generated,
        // which is the name its reference is read under too.
        case: "a declaration that states no name at all",
        logical_id: "Generated",
        declared_name: "",
        properties: r#"{
            "Description": "created under the name CloudFormation generated",
            "SecretString": "a-value-under-a-generated-name"
        }"#,
        expected: Expected::Verbatim("a-value-under-a-generated-name"),
    },
];

/// The template declaring every row, and publishing each secret's ARN as an output — "Ref
/// returns the ARN of the secret", which is the handle every Secrets Manager call takes.
fn template() -> String {
    let mut resources = serde_json::Map::new();
    let mut outputs = serde_json::Map::new();
    for row in ROWS {
        let properties: Value =
            serde_json::from_str(row.properties).expect("a row's properties are JSON");
        resources.insert(
            row.logical_id.to_string(),
            json!({"Type": "AWS::SecretsManager::Secret", "Properties": properties}),
        );
        outputs.insert(
            arn_output(row),
            json!({"Value": {"Ref": row.logical_id}}),
        );
    }
    json!({"Resources": resources, "Outputs": outputs}).to_string()
}

/// The output key a row's ARN is published under.
fn arn_output(row: &Row) -> String {
    format!("{}Arn", row.logical_id)
}

/// Call one AWS Secrets Manager operation through the same router, signed for
/// `secretsmanager` the way a client's SDK signs it: the status and the parsed answer.
///
/// `pub(crate)`: `tests/all.rs` is this binary's crate root, so a sibling suite that reads
/// the secret a stack created addresses that service through THIS call rather than carrying
/// a second copy of it.
pub(crate) async fn secretsmanager(
    router: &MultiServiceRouter,
    operation: &str,
    request: &Value,
) -> (u16, Value) {
    let http_request = http::Request::builder()
        .method("POST")
        .uri(format!("https://secretsmanager.{REGION}.amazonaws.com/"))
        .header("host", format!("secretsmanager.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header(
            "x-amz-target",
            format!("{SECRETSMANAGER_TARGET_PREFIX}.{operation}"),
        )
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/secretsmanager/\
                 aws4_request, SignedHeaders=host;x-amz-target, Signature=0123456789abcdef"
            ),
        )
        .body(bytes::Bytes::from(request.to_string()))
        .expect("a well-formed Secrets Manager request");
    let response = router.dispatch(http_request).await;
    let status_code = response.status().as_u16();
    let raw = response.into_body();
    let parsed = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw).unwrap_or(Value::Null)
    };
    (status_code, parsed)
}

/// The `__type` discriminator of an awsJson error body, without its namespace.
///
/// `pub(crate)`: `tests/all.rs` is this binary's crate root, so a sibling suite reading an
/// awsJson refusal reads THIS split rather than carrying a second copy of it.
pub(crate) fn error_code(case: &str, parsed: &Value) -> String {
    let raw = parsed
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: no __type in error body {parsed}"));
    raw.split('#').next_back().unwrap_or(raw).to_string()
}

/// A required string member of an answer.
fn text(case: &str, parsed: &Value, pointer: &str) -> String {
    parsed
        .pointer(pointer)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: no {pointer} in {parsed}"))
        .to_string()
}

/// Read the stack until it reports a status that is no longer in progress, and answer that
/// read's body. Waits for the CONDITION: each attempt looks again.
///
/// `pub(crate)`: a sibling suite that deploys and then tears down a stack waits for the same
/// condition, so it reads this loop rather than carrying a second copy of it.
pub(crate) async fn poll_to_terminal(
    router: &MultiServiceRouter,
    case: &str,
    stack: &str,
) -> String {
    let mut described = String::new();
    for _ in 0..POLL_ATTEMPTS {
        wait_for_the_transition().await;
        let (status_code, body) = describe_stack(router, stack).await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: DescribeStacks must answer 200, got {status_code} {body}"
        );
        described = body;
        if !xml_text(&described, "StackStatus")
            .unwrap_or_default()
            .ends_with("_IN_PROGRESS")
        {
            break;
        }
    }
    described
}

/// What a row's secret is addressed by once the stack has created it: the ARN the stack
/// published, the name Secrets Manager created it under, and the reserved-path name a client
/// reads it through.
#[derive(Debug, Clone)]
struct Provisioned {
    case: &'static str,
    arn: String,
    name: String,
    reference: String,
}

/// Assert the value the reference served is the one the row's declaration asked for.
fn assert_value(case: &str, expected: Expected, value: &str) {
    match expected {
        Expected::Verbatim(stated) => assert_eq!(
            value, stated,
            "case {case:?}: expected the reference to serve the declared value {stated:?}, \
             got {value:?}"
        ),
        Expected::Generated {
            username,
            key,
            length,
        } => {
            let document: Value = serde_json::from_str(value).unwrap_or_else(|error| {
                panic!(
                    "case {case:?}: expected the generated value to be the declared JSON \
                     document, got {value:?} ({error})"
                )
            });
            assert_eq!(
                document.get("username").and_then(Value::as_str),
                Some(username),
                "case {case:?}: expected the declared document's members to survive the \
                 generation, got {document}"
            );
            let generated = document
                .get(key)
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {case:?}: no {key} in {document}"));
            assert_eq!(
                generated.len(),
                length,
                "case {case:?}: expected a generated password of {length} characters under \
                 {key:?}, got {generated:?} ({} characters)",
                generated.len()
            );
        }
    }
}

#[tokio::test]
async fn a_deleted_stacks_secret_stops_being_served_through_parameter_store() {
    let router = router();
    let (status_code, body) = create_stack(&router, STACK, &template(), &[]).await;
    assert_eq!(
        status_code, 200,
        "CreateStack must answer 200, got {status_code} {body}"
    );
    let described = poll_to_terminal(&router, "the stack", STACK).await;
    assert_eq!(
        xml_text(&described, "StackStatus").as_deref(),
        Some("CREATE_COMPLETE"),
        "expected the create to complete, got {described}"
    );

    // BEFORE the deletion: every secret the stack created is served, and the reserved
    // Parameter Store hierarchy resolves each one through to Secrets Manager.
    let mut provisioned = Vec::new();
    for row in ROWS {
        let case = row.case;
        let arn = output_value(&described, &arn_output(row))
            .unwrap_or_else(|| panic!("case {case:?}: the stack published no ARN in {described}"));
        let (status_code, answered) =
            secretsmanager(&router, "DescribeSecret", &json!({"SecretId": arn})).await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: expected Secrets Manager to hold the secret the stack created, \
             got {status_code} {answered}"
        );
        let name = text(case, &answered, "/Name");
        if !row.declared_name.is_empty() {
            assert_eq!(
                name, row.declared_name,
                "case {case:?}: expected the secret to be created under the declared name, \
                 got {name:?}"
            );
        } else {
            assert!(
                name.starts_with(STACK) && name.len() > STACK.len(),
                "case {case:?}: expected a generated name built on the stack's, got {name:?}"
            );
        }

        let reference = format!("{REFERENCE_ROOT}/{name}");
        let (status_code, served) = ssm(
            &router,
            "GetParameter",
            &json!({"Name": reference, "WithDecryption": true}),
        )
        .await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: expected GetParameter({reference}) to resolve the secret, got \
             {status_code} {served}"
        );
        assert_eq!(
            text(case, &served, "/Parameter/Type"),
            SECURE_STRING,
            "case {case:?}: expected a reference to be reported as {SECURE_STRING}, got {served}"
        );
        assert_eq!(
            text(case, &served, "/Parameter/Name"),
            reference,
            "case {case:?}: expected the reference's name reported as asked, got {served}"
        );
        assert_eq!(
            text(case, &served, "/Parameter/ARN"),
            arn,
            "case {case:?}: expected the reference to carry the SECRET's ARN, got {served}"
        );
        assert_value(case, row.expected, &text(case, &served, "/Parameter/Value"));

        provisioned.push(Provisioned {
            case,
            arn,
            name,
            reference,
        });
    }

    // The teardown itself.
    let (status_code, body) = call(&router, "DeleteStack", &[member("StackName", STACK)]).await;
    assert_eq!(
        status_code, 200,
        "DeleteStack must answer 200, got {status_code} {body}"
    );
    let (status_code, described) = describe_stack(&router, STACK).await;
    assert_eq!(
        status_code, 200,
        "DescribeStacks must answer 200 while the teardown runs, got {status_code} {described}"
    );
    let stack_id = xml_text(&described, "StackId").unwrap_or_default();
    let described = poll_to_terminal(&router, "the teardown", &stack_id).await;
    assert_eq!(
        xml_text(&described, "StackStatus").as_deref(),
        Some("DELETE_COMPLETE"),
        "expected the teardown to reach DELETE_COMPLETE, got {described}"
    );

    // AFTER it: there is no such secret any more, whichever way it is addressed — so the
    // reference resolves nothing either.
    for secret in &provisioned {
        let case = secret.case;
        for addressed in [&secret.arn, &secret.name] {
            for operation in ["DescribeSecret", "GetSecretValue"] {
                let (status_code, refused) =
                    secretsmanager(&router, operation, &json!({"SecretId": addressed})).await;
                assert_eq!(
                    (status_code, error_code(case, &refused).as_str()),
                    (400, SECRET_NOT_FOUND),
                    "case {case:?}: expected {operation}({addressed}) on the deleted stack's \
                     secret to be refused with {SECRET_NOT_FOUND}, got {status_code} {refused}"
                );
            }
        }
        let (status_code, refused) = ssm(
            &router,
            "GetParameter",
            &json!({"Name": secret.reference, "WithDecryption": true}),
        )
        .await;
        assert_eq!(
            (status_code, error_code(case, &refused).as_str()),
            (400, PARAMETER_NOT_FOUND),
            "case {case:?}: expected GetParameter({}) to be refused with \
             {PARAMETER_NOT_FOUND} once the stack that owned the secret is deleted, got \
             {status_code} {refused}",
            secret.reference
        );
        let expected = expected_reference_miss(&secret.name);
        assert_eq!(
            refused
                .get("Message")
                .or_else(|| refused.get("message"))
                .and_then(Value::as_str),
            Some(expected.as_str()),
            "case {case:?}: expected the refusal to state why the reference resolved to \
             nothing ({expected:?}), got {refused}"
        );
    }
}
