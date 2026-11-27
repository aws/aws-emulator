// SPDX-License-Identifier: CC0-1.0

//! `DeleteApplicationVersion`, driven end to end over the service's awsQuery HTTP
//! surface.
//!
//! The rule under test: deleting an application version removes exactly the
//! `(ApplicationName, VersionLabel)` record the request names and nothing else,
//! succeeds whatever the label and whether or not that record was there, is
//! refused with the operation's modelled `OperationInProgressException` while an
//! environment of that application that has not reached `Terminated` is deployed
//! from the version, and is refused with `InvalidParameterValue` when the
//! application name resolves to nothing. `DeleteSourceBundle` changes none of it:
//! the flag speaks about an S3 object, and the version record goes either way.
//!
//! The emulator and the XML reader come from [`crate::lifecycle`], so this suite
//! speaks to the service over the same router and in the same dialect the rest of
//! the crate's tests do.

use crate::lifecycle::{wait_for_status, Emulator, SOLUTION_STACK};

/// `VersionLabel`'s modelled length bound is 1..100
/// (`smithy_query(query="operation",operation="DeleteApplicationVersion")`), so a
/// label of exactly 100 characters is the largest one the delete must accept.
fn label_at_max_length() -> String {
    "v".repeat(100)
}

/// Create `application` with `labels`, failing `case` if any call does not answer
/// 200.
async fn given_versions(emulator: &Emulator, case: &str, application: &str, labels: &[String]) {
    let created = emulator
        .call("CreateApplication", &[("ApplicationName", application)])
        .await;
    assert_eq!(
        created.status, 200,
        "case {case:?}: expected 200 from CreateApplication for {application:?}, got {} and body {}",
        created.status, created.body
    );
    for label in labels {
        let version = emulator
            .call(
                "CreateApplicationVersion",
                &[
                    ("ApplicationName", application),
                    ("VersionLabel", label.as_str()),
                    ("SourceBundle.S3Bucket", "bundles"),
                    ("SourceBundle.S3Key", "app.zip"),
                ],
            )
            .await;
        assert_eq!(
            version.status, 200,
            "case {case:?}: expected 200 from CreateApplicationVersion for {label:?}, got {} and body {}",
            version.status, version.body
        );
    }
}

/// The version labels `application` has, newest first — the one reader of this
/// store that is implemented, so a delete that did not persist fails here.
async fn stored_labels(emulator: &Emulator, application: &str) -> Vec<String> {
    let described = emulator
        .call(
            "DescribeApplications",
            &[("ApplicationNames.member.1", application)],
        )
        .await;
    described
        .values_at("Versions/member")
        .into_iter()
        .map(str::to_string)
        .collect()
}

/// `DeleteApplicationVersion`'s parameters for one case.
fn delete_params<'a>(
    application: &'a str,
    label: &'a str,
    delete_source_bundle: Option<&'a str>,
) -> Vec<(&'a str, &'a str)> {
    let mut params = vec![
        ("ApplicationName", application),
        ("VersionLabel", label),
    ];
    if let Some(flag) = delete_source_bundle {
        params.push(("DeleteSourceBundle", flag));
    }
    params
}

/// One row of the success table.
struct DeleteCase {
    case: &'static str,
    /// The labels created on the application, oldest first.
    created: Vec<String>,
    /// The label the delete names.
    delete: String,
    /// `DeleteSourceBundle`, when the request carries it.
    delete_source_bundle: Option<&'static str>,
    /// The labels that must remain afterwards, newest first.
    remaining: Vec<String>,
}

fn labels(labels: &[&str]) -> Vec<String> {
    labels.iter().map(|label| label.to_string()).collect()
}

#[tokio::test]
async fn deleting_a_version_removes_exactly_the_record_it_names() {
    let max_label = label_at_max_length();
    let cases = vec![
        DeleteCase {
            case: "the application's only version",
            created: labels(&["v1"]),
            delete: "v1".to_string(),
            delete_source_bundle: None,
            remaining: Vec::new(),
        },
        DeleteCase {
            case: "the newest of several",
            created: labels(&["v1", "v2", "v3"]),
            delete: "v3".to_string(),
            delete_source_bundle: None,
            remaining: labels(&["v2", "v1"]),
        },
        DeleteCase {
            case: "the middle of several",
            created: labels(&["v1", "v2", "v3"]),
            delete: "v2".to_string(),
            delete_source_bundle: None,
            remaining: labels(&["v3", "v1"]),
        },
        DeleteCase {
            case: "the oldest of several",
            created: labels(&["v1", "v2", "v3"]),
            delete: "v1".to_string(),
            delete_source_bundle: None,
            remaining: labels(&["v3", "v2"]),
        },
        DeleteCase {
            case: "a label carrying the punctuation a semantic version uses",
            created: labels(&["1.0.0-beta_1", "1.0.0"]),
            delete: "1.0.0-beta_1".to_string(),
            delete_source_bundle: None,
            remaining: labels(&["1.0.0"]),
        },
        DeleteCase {
            case: "a label at the modelled maximum length",
            created: vec![max_label.clone(), "v2".to_string()],
            delete: max_label.clone(),
            delete_source_bundle: None,
            remaining: labels(&["v2"]),
        },
        DeleteCase {
            case: "DeleteSourceBundle=true",
            created: labels(&["v1", "v2"]),
            delete: "v1".to_string(),
            delete_source_bundle: Some("true"),
            remaining: labels(&["v2"]),
        },
        DeleteCase {
            case: "DeleteSourceBundle=false",
            created: labels(&["v1", "v2"]),
            delete: "v1".to_string(),
            delete_source_bundle: Some("false"),
            remaining: labels(&["v2"]),
        },
        DeleteCase {
            // The recorded case this issue was filed from: the label names no
            // stored version, so the delete has nothing to do and says so with a
            // success, leaving the application exactly as it was.
            case: "a label the application does not have",
            created: labels(&["v1", "v2"]),
            delete: "absent".to_string(),
            delete_source_bundle: None,
            remaining: labels(&["v2", "v1"]),
        },
        DeleteCase {
            case: "a label the application does not have, with DeleteSourceBundle=true",
            created: labels(&["v1"]),
            delete: "absent".to_string(),
            delete_source_bundle: Some("true"),
            remaining: labels(&["v1"]),
        },
        DeleteCase {
            case: "an application with no versions at all",
            created: Vec::new(),
            delete: "v1".to_string(),
            delete_source_bundle: None,
            remaining: Vec::new(),
        },
    ];

    for DeleteCase {
        case,
        created,
        delete,
        delete_source_bundle,
        remaining,
    } in &cases
    {
        let emulator = Emulator::new();
        given_versions(&emulator, case, "app", created).await;

        let deleted = emulator
            .call(
                "DeleteApplicationVersion",
                &delete_params("app", delete, *delete_source_bundle),
            )
            .await;
        assert_eq!(
            deleted.status, 200,
            "case {case:?}: expected HTTP 200 from DeleteApplicationVersion, got {} and body {}",
            deleted.status, deleted.body
        );

        assert_eq!(
            &stored_labels(&emulator, "app").await,
            remaining,
            "case {case:?}: unexpected remaining versions after deleting {delete:?}"
        );

        // The application itself survives the deletion of one of its versions.
        let described = emulator.call("DescribeApplications", &[]).await;
        assert_eq!(
            described.values_at("Applications/member/ApplicationName"),
            vec!["app"],
            "case {case:?}: the application must still be there, got {}",
            described.body
        );

        // And the delete is idempotent: calling it again answers the same way
        // and leaves the same store behind.
        let again = emulator
            .call(
                "DeleteApplicationVersion",
                &delete_params("app", delete, *delete_source_bundle),
            )
            .await;
        assert_eq!(
            again.status, 200,
            "case {case:?}: expected HTTP 200 from the repeated delete, got {} and body {}",
            again.status, again.body
        );
        assert_eq!(
            &stored_labels(&emulator, "app").await,
            remaining,
            "case {case:?}: the repeated delete must have changed nothing"
        );
    }
}

#[tokio::test]
async fn a_deleted_version_is_gone_from_the_store_not_just_from_the_listing() {
    let case = "deleted version";
    let emulator = Emulator::new();
    given_versions(&emulator, case, "app", &labels(&["v1"])).await;

    // Before the delete the version can be deployed, which is what makes the
    // refusal afterwards evidence that the record itself is gone.
    let deleted = emulator
        .call(
            "DeleteApplicationVersion",
            &delete_params("app", "v1", None),
        )
        .await;
    assert_eq!(
        deleted.status, 200,
        "case {case:?}: expected HTTP 200 from DeleteApplicationVersion, got {} and body {}",
        deleted.status, deleted.body
    );

    let refused = emulator
        .call(
            "CreateEnvironment",
            &[
                ("ApplicationName", "app"),
                ("EnvironmentName", "app-env"),
                ("SolutionStackName", SOLUTION_STACK.as_str()),
                ("VersionLabel", "v1"),
            ],
        )
        .await;
    assert_eq!(
        refused.status, 400,
        "case {case:?}: expected HTTP 400 from CreateEnvironment on the deleted version, got {} and body {}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error_code(),
        "InvalidParameterValue",
        "case {case:?}: unexpected error code in {}",
        refused.body
    );

    // The label is free again, so the same label can be created anew and comes
    // back from the store with the new record's description.
    let recreated = emulator
        .call(
            "CreateApplicationVersion",
            &[
                ("ApplicationName", "app"),
                ("VersionLabel", "v1"),
                ("Description", "the second v1"),
            ],
        )
        .await;
    assert_eq!(
        recreated.status, 200,
        "case {case:?}: expected HTTP 200 re-creating the deleted label, got {} and body {}",
        recreated.status, recreated.body
    );
    assert_eq!(
        recreated.expect("ApplicationVersion/Description", case),
        "the second v1",
        "case {case:?}: the re-created version must be the new record, got {}",
        recreated.body
    );
    assert_eq!(
        stored_labels(&emulator, "app").await,
        labels(&["v1"]),
        "case {case:?}: the re-created version must be stored"
    );
}

#[tokio::test]
async fn deleting_a_version_leaves_the_same_label_in_other_applications_alone() {
    let case = "same label in two applications";
    let emulator = Emulator::new();
    given_versions(&emulator, case, "left", &labels(&["v1", "v2"])).await;
    given_versions(&emulator, case, "right", &labels(&["v1", "v2"])).await;

    let deleted = emulator
        .call(
            "DeleteApplicationVersion",
            &delete_params("left", "v1", None),
        )
        .await;
    assert_eq!(
        deleted.status, 200,
        "case {case:?}: expected HTTP 200 from DeleteApplicationVersion, got {} and body {}",
        deleted.status, deleted.body
    );
    assert_eq!(
        stored_labels(&emulator, "left").await,
        labels(&["v2"]),
        "case {case:?}: only the named application's version may go"
    );
    assert_eq!(
        stored_labels(&emulator, "right").await,
        labels(&["v2", "v1"]),
        "case {case:?}: the other application's identically labelled version must stay"
    );
}

/// One row of the refusal table: the request, and the status and awsQuery error
/// code it must be refused with.
struct RefusalCase {
    case: &'static str,
    application: &'static str,
    label: &'static str,
    delete_source_bundle: Option<&'static str>,
    status: u16,
    code: &'static str,
}

#[tokio::test]
async fn a_version_that_cannot_be_deleted_is_refused_with_the_services_own_codes() {
    let emulator = Emulator::new();
    given_versions(&emulator, "refusals", "app", &labels(&["v1", "v2"])).await;
    // `v1` is deployed to an environment that is still running, which is what the
    // operation documents as undeletable.
    let environment = emulator
        .call(
            "CreateEnvironment",
            &[
                ("ApplicationName", "app"),
                ("EnvironmentName", "app-env"),
                ("SolutionStackName", SOLUTION_STACK.as_str()),
                ("VersionLabel", "v1"),
            ],
        )
        .await;
    assert_eq!(
        environment.status, 200,
        "expected 200 from CreateEnvironment, got {} and body {}",
        environment.status, environment.body
    );

    let cases = [
        RefusalCase {
            case: "an application that does not exist",
            application: "nope",
            label: "v1",
            delete_source_bundle: None,
            status: 400,
            code: "InvalidParameterValue",
        },
        RefusalCase {
            case: "an application that does not exist, with DeleteSourceBundle=true",
            application: "nope",
            label: "v1",
            delete_source_bundle: Some("true"),
            status: 400,
            code: "InvalidParameterValue",
        },
        RefusalCase {
            case: "a version a running environment is deployed from",
            application: "app",
            label: "v1",
            delete_source_bundle: None,
            // `OperationInProgressException` carries the awsQuery code
            // `OperationInProgressFailure` (its `awsQueryError` trait).
            status: 400,
            code: "OperationInProgressFailure",
        },
        RefusalCase {
            case: "a version a running environment is deployed from, with DeleteSourceBundle=true",
            application: "app",
            label: "v1",
            delete_source_bundle: Some("true"),
            status: 400,
            code: "OperationInProgressFailure",
        },
    ];

    for RefusalCase {
        case,
        application,
        label,
        delete_source_bundle,
        status,
        code,
    } in &cases
    {
        let refused = emulator
            .call(
                "DeleteApplicationVersion",
                &delete_params(application, label, *delete_source_bundle),
            )
            .await;
        assert_eq!(
            refused.status, *status,
            "case {case:?}: expected HTTP {status}, got {} and body {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            *code,
            "case {case:?}: unexpected error code in {}",
            refused.body
        );
        assert!(
            refused.value("Error/Message").is_some(),
            "case {case:?}: a refusal must say what to fix, got body {}",
            refused.body
        );
    }

    // Nothing the refusals named was removed.
    assert_eq!(
        stored_labels(&emulator, "app").await,
        labels(&["v2", "v1"]),
        "case \"refusals\": a refused delete must not have removed anything"
    );

    // The version not deployed anywhere is deletable on the same store.
    let deleted = emulator
        .call(
            "DeleteApplicationVersion",
            &delete_params("app", "v2", None),
        )
        .await;
    assert_eq!(
        deleted.status, 200,
        "case \"undeployed version\": expected HTTP 200, got {} and body {}",
        deleted.status, deleted.body
    );
    assert_eq!(
        stored_labels(&emulator, "app").await,
        labels(&["v1"]),
        "case \"undeployed version\": the undeployed version must be gone"
    );
}

#[tokio::test]
async fn a_version_becomes_deletable_once_its_environment_has_terminated() {
    let case = "terminating environment";
    let emulator = Emulator::new();
    given_versions(&emulator, case, "app", &labels(&["v1"])).await;
    let environment = emulator
        .call(
            "CreateEnvironment",
            &[
                ("ApplicationName", "app"),
                ("EnvironmentName", "app-env"),
                ("SolutionStackName", SOLUTION_STACK.as_str()),
                ("VersionLabel", "v1"),
            ],
        )
        .await;
    assert_eq!(
        environment.status, 200,
        "case {case:?}: expected 200 from CreateEnvironment, got {} and body {}",
        environment.status, environment.body
    );

    // A termination can only take down an environment whose launch has got past
    // its initial phase, which is what the clock owes it: waited for as a
    // CONDITION, never as a fixed time.
    wait_for_status(&emulator, case, "app-env", "Ready").await;
    let terminating = emulator
        .call("TerminateEnvironment", &[("EnvironmentName", "app-env")])
        .await;
    assert_eq!(
        terminating.status, 200,
        "case {case:?}: expected 200 from TerminateEnvironment, got {} and body {}",
        terminating.status, terminating.body
    );
    assert_eq!(
        terminating.value("Status"),
        Some("Terminating"),
        "case {case:?}: the environment must be shutting down, got {}",
        terminating.body
    );

    // An environment that has not finished shutting down is still deployed from
    // the version, so the delete is still refused.
    let refused = emulator
        .call(
            "DeleteApplicationVersion",
            &delete_params("app", "v1", None),
        )
        .await;
    assert_eq!(
        refused.status, 400,
        "case {case:?}: expected HTTP 400 while the environment is terminating, got {} and body {}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error_code(),
        "OperationInProgressFailure",
        "case {case:?}: unexpected error code in {}",
        refused.body
    );

    // Once the clock has made the termination due, nothing running is deployed
    // from the version and the same call succeeds — waited for as the CONDITION
    // the environment reaches, never as a fixed time.
    wait_for_status(&emulator, case, "app-env", "Terminated").await;
    let deleted = emulator
        .call(
            "DeleteApplicationVersion",
            &delete_params("app", "v1", None),
        )
        .await;
    assert_eq!(
        deleted.status, 200,
        "case {case:?}: expected HTTP 200 once the environment terminated, got {} and body {}",
        deleted.status, deleted.body
    );
    assert!(
        stored_labels(&emulator, "app").await.is_empty(),
        "case {case:?}: the version must be gone, got {:?}",
        stored_labels(&emulator, "app").await
    );
}
