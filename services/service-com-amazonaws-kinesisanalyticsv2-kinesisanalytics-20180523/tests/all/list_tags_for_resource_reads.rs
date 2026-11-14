// SPDX-License-Identifier: CC0-1.0

//! `ListTagsForResource` as a REPORTING operation over the application record.
//!
//! The rule these suites assert, one level above the recorded Studio-notebook case:
//! `ListTagsForResource` reports the key-value tags the named application's record holds —
//! whatever runtime family that application runs on and whichever call assigned them — in
//! one stable key order, and, being a read, leaves the record exactly as it found it, so a
//! later caller sees what it saw.
//!
//! The suites in `tags_versions_and_snapshots.rs` cover the round trip of one assignment
//! order through the tagging calls; these cover what the read itself owes: the ORDER it
//! reports, the FAMILIES it serves, and the fact that reading changes nothing.

use std::sync::Arc;

use serde_json::json;
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_kinesisanalyticsv2_kinesisanalytics_20180523::ops::types::RUNTIME_ENVIRONMENT_VALUES;
use service_com_amazonaws_kinesisanalyticsv2_kinesisanalytics_20180523::state;

use crate::harness::{
    application_arn, call, create, create_body, describe, error_code, list_tags, router, router_on,
    tags_of,
};

/// Assign `tags` to `name` through `TagResource`, asserting the call succeeded.
async fn assign(router: &MultiServiceRouter, case: &str, name: &str, tags: &[(&str, &str)]) {
    let (status, body) = call(
        router,
        "TagResource",
        &json!({
            "ResourceARN": application_arn(name),
            "Tags": tags.iter().map(|(k, v)| json!({"Key": k, "Value": v})).collect::<Vec<_>>(),
        }),
    )
    .await;
    assert_eq!(status, 200, "case {case}: TagResource got {status} {body}");
}

/// Store an application of runtime family `runtime` under `name`, carrying the single tag
/// `at-create` with `tag_value`, and assert the create stored it.
///
/// `CreateApplication` no longer provisions the retired SQL runtime family, so for a family
/// the create refuses the application is created through the real create path and its stored
/// `RuntimeEnvironment` is re-stamped in the store — which is what an application created
/// before that family was retired looks like to every reader, the same route
/// `sql_input_and_configuration_defaults.rs` takes.
async fn store_tagged_application(
    engine: &Arc<engine::InMemoryEngine>,
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    runtime: &str,
    tag_value: &str,
) {
    let tagged = |runtime: &str| {
        let mut body = create_body(name, runtime);
        body["Tags"] = json!([{"Key": "at-create", "Value": tag_value}]);
        body
    };
    let (status, answered) = call(router, "CreateApplication", &tagged(runtime)).await;
    if status == 200 {
        return;
    }

    let (status, retried) = call(router, "CreateApplication", &tagged("FLINK-1_20")).await;
    assert_eq!(
        status, 200,
        "case {case} ({runtime}): CreateApplication refused the row with {answered} and the \
         stand-in with {retried}"
    );
    let store = state::service_state(&**engine).expect("the service holds state");
    let mut guard = store.lock();
    let application = guard
        .applications
        .get_mut(name)
        .unwrap_or_else(|| panic!("case {case} ({runtime}): the application was just created"));
    application.runtime_environment = runtime.to_string();
}

/// One row of the reporting-order table: the case name and the tag sets assigned in order.
type AssignmentOrder<'a> = (&'a str, &'a [&'a [(&'a str, &'a str)]]);

/// Any two applications carrying the same tags report them identically: the read answers in
/// key order, so the order the keys were assigned in — one call or several, ascending or
/// descending, with a value replaced afterwards — is not visible to the caller.
#[tokio::test]
async fn tags_are_reported_in_key_order_whatever_order_they_were_assigned() {
    let router = router();
    // The same three tags reached each application by a different route.
    let expected: Vec<(String, String)> = [("alpha", "1"), ("mid", "2"), ("zeta", "3")]
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    let cases: [AssignmentOrder; 5] = [
        (
            "ascending-in-one-call",
            &[&[("alpha", "1"), ("mid", "2"), ("zeta", "3")]],
        ),
        (
            "descending-in-one-call",
            &[&[("zeta", "3"), ("mid", "2"), ("alpha", "1")]],
        ),
        (
            "one-key-per-call-descending",
            &[&[("zeta", "3")], &[("mid", "2")], &[("alpha", "1")]],
        ),
        (
            "interleaved-calls",
            &[&[("mid", "2")], &[("zeta", "3"), ("alpha", "1")]],
        ),
        (
            "a-replaced-value-keeps-its-key-in-place",
            &[
                &[("alpha", "superseded"), ("mid", "2"), ("zeta", "3")],
                &[("alpha", "1")],
            ],
        ),
    ];

    for (case, assigned) in cases {
        let name = format!("order-{case}");
        create(&router, &name, "FLINK-1_20").await;
        for tags in assigned {
            assign(&router, case, &name, tags).await;
        }
        let actual = tags_of(&router, &name).await;
        assert_eq!(
            actual, expected,
            "case {case}: expected the tags in key order, got {actual:?}"
        );
    }
}

/// Tagging is not a runtime-family-specific capability the way the version readers are: an
/// application of EVERY family this service stores reports the tags it carries, whether they
/// were supplied to the create or assigned afterwards.
///
/// The rows are the whole `RuntimeEnvironment` enum, read off the generated value set, so
/// the retired `SQL-` family, every `FLINK-` version and the `ZEPPELIN-FLINK-` Studio
/// notebooks are each covered.
#[tokio::test]
async fn an_application_of_every_runtime_family_reports_its_tags() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(Arc::clone(&engine));

    for (index, runtime) in RUNTIME_ENVIRONMENT_VALUES.iter().enumerate() {
        let case = format!("family-{index}");
        let name = format!("family-tagged-{index}");
        store_tagged_application(&engine, &router, &case, &name, runtime, runtime).await;

        let actual = tags_of(&router, &name).await;
        assert_eq!(
            actual,
            vec![("at-create".to_string(), runtime.to_string())],
            "case {case} ({runtime}): expected the tag the create stored, got {actual:?}"
        );

        assign(&router, &case, &name, &[("after-create", "assigned")]).await;
        let actual = tags_of(&router, &name).await;
        assert_eq!(
            actual,
            vec![
                ("after-create".to_string(), "assigned".to_string()),
                ("at-create".to_string(), runtime.to_string()),
            ],
            "case {case} ({runtime}): expected both tags, got {actual:?}"
        );
    }
}

/// Reading the tags of an application changes nothing a later caller sees: repeated reads
/// answer identically, the application's version is not bumped by a read, and the tags the
/// record holds are the same after the reads as before them.
#[tokio::test]
async fn reading_the_tags_of_an_application_leaves_its_record_unchanged() {
    let router = router();
    for (case, assigned) in [
        ("untagged", &[] as &[(&str, &str)]),
        ("tagged", &[("env", "prod"), ("owner", "data")]),
    ] {
        let name = format!("read-only-{case}");
        create(&router, &name, "FLINK-1_20").await;
        if !assigned.is_empty() {
            assign(&router, case, &name, assigned).await;
        }
        let expected: Vec<(String, String)> = assigned
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        let version_before = describe(&router, &name).await["ApplicationVersionId"].clone();

        for read in 0..3 {
            let actual = tags_of(&router, &name).await;
            assert_eq!(
                actual, expected,
                "case {case}: read {read} must report the stored tags, got {actual:?}"
            );
        }

        let version_after = describe(&router, &name).await["ApplicationVersionId"].clone();
        assert_eq!(
            version_after, version_before,
            "case {case}: reading tags must not bump the application version, got \
             {version_after} after {version_before}"
        );
        let actual = tags_of(&router, &name).await;
        assert_eq!(
            actual, expected,
            "case {case}: the tags after the reads must be the ones before them, got {actual:?}"
        );
    }
}

/// A read that misses creates nothing: a `ResourceARN` naming an application this service
/// does not hold is refused, the name is still absent afterwards, and an application created
/// under that name then starts out carrying no tags.
#[tokio::test]
async fn a_read_that_misses_creates_no_application() {
    let router = router();
    let name = "never-created-then-created";
    let (status, body) = list_tags(&router, &application_arn(name)).await;
    assert_eq!(
        status, 400,
        "case miss: ListTagsForResource got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "ResourceNotFoundException",
        "case miss: expected the lookup miss, got {body}"
    );

    let (status, body) = call(
        &router,
        "DescribeApplication",
        &json!({"ApplicationName": name}),
    )
    .await;
    assert_eq!(
        status, 400,
        "case miss: the read must not have created the application, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "ResourceNotFoundException",
        "case miss: expected the application to still be absent, got {body}"
    );

    create(&router, name, "FLINK-1_20").await;
    let actual = tags_of(&router, name).await;
    assert_eq!(
        actual,
        Vec::new(),
        "case miss: expected the created application to carry no tags, got {actual:?}"
    );
}
