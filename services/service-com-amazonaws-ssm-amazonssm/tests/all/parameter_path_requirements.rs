// SPDX-License-Identifier: CC0-1.0

//! What a `Path` that is not a parameter hierarchy is refused with, end to end over the
//! front door.
//!
//! RULE under test: **a `Path` is a parameter-name prefix, so one that does not meet
//! Parameter Store's parameter-name requirements is refused with the one paragraph the
//! service states those requirements in — fixed text, naming neither the rejected path nor
//! the single rule it broke.** A client cannot fix a path from a sentence invented here,
//! and a client that matches on the service's sentence never sees one.
//!
//! The table's rows differ along what the rule generalises over: WHICH requirement the path
//! breaks (no leading separator, a character no name may carry, a space between characters)
//! and the depth and form of the path. Paths that DO meet the requirements are read in the
//! same pass — the account's own hierarchy and the one AWS reserves for its published
//! parameters, which a read reaches by naming it — so no row passes against a handler that
//! refuses every path.
//!
//! Both operations that take a hierarchy are driven: `GetParametersByPath`, whose `Path` is
//! the hierarchy itself, and `DescribeParameters`, whose `Path` FILTER value is one.

use serde_json::{json, Value};

use crate::parameter_store::{call, error_code, error_message};

/// The paragraph Parameter Store states its parameter-name requirements in. Quoted exactly
/// as the service words it, backslashes and all: the quotes around `aws` and `ssm` reach
/// the client escaped.
const NAME_REQUIREMENTS: &str = concat!(
    r#"The parameter doesn't meet the parameter name requirements. "#,
    r#"The parameter name must begin with a forward slash "/". "#,
    r#"It can't be prefixed with \"aws\" or \"ssm\" (case-insensitive). "#,
    r#"It must use only letters, numbers, or the following symbols: . (period), "#,
    r#"- (hyphen), _ (underscore). Special characters are not allowed. "#,
    r#"All sub-paths, if specified, must use the forward slash symbol "/". "#,
    r#"Valid example: /get/parameters2-/by1./path0_."#,
);

/// One row: a path, and which of the requirements it breaks.
struct Row {
    case: &'static str,
    path: &'static str,
}

const REFUSED: &[Row] = &[
    Row {
        case: "a one-level path with no leading separator",
        path: "Dev",
    },
    Row {
        case: "a hierarchy with no leading separator",
        path: "Dev/DBServer/prod",
    },
    Row {
        case: "a deep hierarchy with no leading separator",
        path: "a/b/c/d/e/f/g",
    },
    Row {
        case: "a path carrying a character no parameter name may",
        path: "/Dev/DB$erver",
    },
    Row {
        case: "a path carrying a space between characters",
        path: "/Dev/DB Server",
    },
    Row {
        case: "a path whose separator is not the forward slash",
        path: "/Dev\\DBServer",
    },
    Row {
        case: "a path that is an ARN rather than a hierarchy",
        path: "arn:aws:ssm:us-west-2:000000000000:parameter/Dev",
    },
];

/// Every path that breaks the requirements is refused with the one paragraph, and nothing of
/// the path is in it.
#[tokio::test]
async fn a_path_that_is_no_hierarchy_states_the_name_requirements() {
    let router = crate::parameter_store::router();
    seed(&router).await;

    for row in REFUSED {
        let (status, body) = call(
            &router,
            "GetParametersByPath",
            &json!({ "Path": row.path, "Recursive": true }),
        )
        .await;
        assert_eq!(
            (status, error_code(&body), error_message(&body)),
            (
                400,
                "ValidationException".to_string(),
                NAME_REQUIREMENTS.to_string()
            ),
            "case {:?} ({}): expected the parameter-name requirements, got {status} {body}",
            row.case,
            row.path
        );
        assert!(
            !error_message(&body).contains(row.path),
            "case {:?}: the paragraph quotes no path back, got {}",
            row.case,
            error_message(&body)
        );
    }

    // A `Path` FILTER value is a hierarchy too, so the same paragraph refuses it — carried
    // by the filter-value class, which is what a describe words a bad filter value with.
    for row in REFUSED {
        let (status, body) = call(
            &router,
            "DescribeParameters",
            &json!({ "ParameterFilters": [{ "Key": "Path", "Values": [row.path] }] }),
        )
        .await;
        assert_eq!(
            (status, error_code(&body), error_message(&body)),
            (
                400,
                "InvalidFilterValue".to_string(),
                NAME_REQUIREMENTS.to_string()
            ),
            "case {:?} ({}): expected a describe to state the same requirements, got \
             {status} {body}",
            row.case,
            row.path
        );
    }
}

/// A path that MEETS the requirements is served: the account's own hierarchy, at either
/// depth, and the hierarchy AWS reserves for its published parameters.
#[tokio::test]
async fn a_path_that_meets_the_requirements_is_served() {
    let router = crate::parameter_store::router();
    seed(&router).await;

    // (case, the path, the names it reports recursively — `None` where the hierarchy is
    // AWS's published catalog, whose contents are that catalog's and not this suite's)
    let served: &[(&str, &str, Option<&[&str]>)] = &[
        (
            "a one-level hierarchy",
            "/Dev",
            Some(&["/Dev/DBServer", "/Dev/app.config", "/Dev/nested/deep_value-1"]),
        ),
        (
            "a two-level hierarchy",
            "/Dev/nested",
            Some(&["/Dev/nested/deep_value-1"]),
        ),
        (
            "a path with a trailing separator",
            "/Dev/",
            Some(&["/Dev/DBServer", "/Dev/app.config", "/Dev/nested/deep_value-1"]),
        ),
        (
            "a path with spaces around it",
            "  /Dev/nested  ",
            Some(&["/Dev/nested/deep_value-1"]),
        ),
        (
            "a hierarchy nothing was written under",
            "/Dev/absent",
            Some(&[]),
        ),
        // AWS's reserved hierarchy is reached by naming it: the reserved-prefix rule the
        // paragraph states bounds what a caller may CREATE, not what it may read.
        ("the hierarchy AWS reserves", "/aws/service", None),
    ];
    for (case, path, expected) in served {
        let (status, body) = call(
            &router,
            "GetParametersByPath",
            &json!({ "Path": path, "Recursive": true, "MaxResults": 10 }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?} ({path}): a path meeting the requirements is served, got \
             {status} {body}"
        );
        let mut reported: Vec<String> = body["Parameters"]
            .as_array()
            .map(|parameters| {
                parameters
                    .iter()
                    .filter_map(|parameter| parameter["Name"].as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        reported.sort();
        match expected {
            Some(expected) => {
                let mut want: Vec<String> =
                    expected.iter().map(|name| name.to_string()).collect();
                want.sort();
                assert_eq!(
                    reported, want,
                    "case {case:?} ({path}): expected the hierarchy's own parameters, got \
                     {body}"
                );
            }
            // The published catalog's own contents: the path is served, and everything it
            // reports lies under the path that was asked for.
            None => {
                assert!(
                    !reported.is_empty()
                        && reported
                            .iter()
                            .all(|name| name.starts_with(&format!("{}/", path.trim()))),
                    "case {case:?} ({path}): expected the published hierarchy under the \
                     path, got {reported:?}"
                );
            }
        }
    }
}

/// The parameters the suites above read: one hierarchy at two depths, spanning the name
/// characters the requirements allow.
async fn seed(router: &smithy_server_runtime::MultiServiceRouter) {
    for name in ["/Dev/DBServer", "/Dev/app.config", "/Dev/nested/deep_value-1"] {
        let (status, written) = call(
            router,
            "PutParameter",
            &json!({ "Name": name, "Type": "String", "Value": "seeded" }),
        )
        .await;
        assert_eq!(
            status, 200,
            "expected {name} to be written, got {status} {written}"
        );
    }
}

/// The paragraph is not a sentence this suite made up: a refusal reports it as the whole
/// message, so a client reading `message` gets text it can act on.
#[tokio::test]
async fn the_paragraph_ends_in_a_path_it_would_accept() {
    let router = crate::parameter_store::router();
    seed(&router).await;
    let (_, body) = call(
        &router,
        "GetParametersByPath",
        &json!({ "Path": "no-leading-slash" }),
    )
    .await;
    let message = error_message(&body);
    let example = message
        .rsplit_once("Valid example: ")
        .map(|(_, example)| example.trim_end_matches('.').to_string())
        .unwrap_or_else(|| panic!("the paragraph ends in a valid example, got {message}"));
    let (status, served) = call(
        &router,
        "GetParametersByPath",
        &json!({ "Path": example, "Recursive": true }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the path the paragraph offers as valid is served, got {status} {served} for \
         {example:?}"
    );
    assert_eq!(
        served["Parameters"].as_array().map(Vec::len),
        Some(0),
        "nothing was written under the example path, got {served}"
    );
    assert!(
        served.get("NextToken").is_none_or(Value::is_null),
        "an empty hierarchy hands out no token, got {served}"
    );
}
