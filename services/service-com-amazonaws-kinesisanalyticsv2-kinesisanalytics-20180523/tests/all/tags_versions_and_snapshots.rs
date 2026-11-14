// SPDX-License-Identifier: CC0-1.0

//! Tagging, version history and snapshot listing: the readers and writers that answer
//! from the application record rather than from the request that reached them.
//!
//! Each suite asserts a RULE over a set of inputs:
//!
//! * a tag assigned to an application is readable back from it, and only the keys a call
//!   names change — whichever call assigned them (the create, `TagResource`) and whichever
//!   removed them (`UntagResource`);
//! * the ceiling on user-defined tags is the application's total, not one request's;
//! * a `ResourceARN` that names no application of this service is a parameter violation,
//!   and one naming an application that is not stored is a lookup miss — for every
//!   tagging call;
//! * every version an application has is listed oldest-first and is readable by id, and a
//!   version it has LEFT reports the configuration IT ran under;
//! * the version readers are supported only for Managed Service for Apache Flink
//!   applications, so every other runtime family is refused;
//! * the snapshots of an application are listed in name order, and the member is present
//!   even when it is empty.

use serde_json::{json, Value};

use crate::harness::{
    application_arn, call, create, create_startable, error_code, error_message, router, tags_of,
    SETTLE_WAIT,
};

// ---------------------------------------------------------------------------
// Tags
// ---------------------------------------------------------------------------

/// One row of the tag round-trip table: the case name, the tag sets applied in order, and
/// the tags the application then carries. Named because the rows are a table of tables.
type TagRoundTrip<'a> = (&'a str, &'a [&'a [(&'a str, &'a str)]], &'a [(&'a str, &'a str)]);

/// A tag assigned to an application is readable back from it: `TagResource` merges the
/// keys it names into the ones the application already carries, replacing the value of a
/// key already assigned ("If you specify a tag that already exists, the tag value is
/// replaced with the value that you specify in the request" — the `Tag` doc), and
/// `ListTagsForResource` reports exactly what is stored.
#[tokio::test]
async fn tags_assigned_to_an_application_are_read_back_from_it() {
    let router = router();
    let cases: [TagRoundTrip; 5] = [
        ("one", &[&[("team", "analytics")]], &[("team", "analytics")]),
        (
            "several-at-once",
            &[&[("env", "prod"), ("owner", "data"), ("tier", "gold")]],
            &[("env", "prod"), ("owner", "data"), ("tier", "gold")],
        ),
        (
            "two-calls-merge",
            &[&[("env", "prod")], &[("owner", "data")]],
            &[("env", "prod"), ("owner", "data")],
        ),
        (
            "second-call-replaces-a-value",
            &[&[("env", "prod")], &[("env", "staging")]],
            &[("env", "staging")],
        ),
        (
            "empty-value-is-a-value",
            &[&[("marker", "")]],
            &[("marker", "")],
        ),
    ];

    for (case, applied, expected) in cases {
        let name = format!("tagged-{case}");
        create(&router, &name, "FLINK-1_20").await;
        // Before any tag is assigned the member is present and empty, so "no tags" is an
        // answer rather than an absent member.
        assert_eq!(
            tags_of(&router, &name).await,
            Vec::new(),
            "case {case}: expected an untagged application to report no tags"
        );

        for tags in applied {
            let body = json!({
                "ResourceARN": application_arn(&name),
                "Tags": tags.iter().map(|(k, v)| json!({"Key": k, "Value": v})).collect::<Vec<_>>(),
            });
            let (status, answered) = call(&router, "TagResource", &body).await;
            assert_eq!(
                status, 200,
                "case {case}: TagResource got {status} {answered}"
            );
            assert_eq!(
                answered,
                json!({}),
                "case {case}: TagResource answers an empty envelope, got {answered}"
            );
        }

        let expected: Vec<(String, String)> = expected
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        let actual = tags_of(&router, &name).await;
        assert_eq!(
            actual, expected,
            "case {case}: expected the stored tags, got {actual:?}"
        );
    }
}

/// Tags supplied to `CreateApplication` are the same store the tagging calls read and
/// write: the create's tags are listed, `TagResource` adds to them and `UntagResource`
/// removes from them.
#[tokio::test]
async fn tags_supplied_at_create_are_the_tags_the_tagging_calls_read() {
    let router = router();
    for (case, created_with) in [
        ("one-at-create", vec![("team", "analytics")]),
        (
            "three-at-create",
            vec![("team", "analytics"), ("env", "prod"), ("tier", "gold")],
        ),
    ] {
        let name = format!("create-tagged-{case}");
        let (status, body) = call(
            &router,
            "CreateApplication",
            &json!({
                "ApplicationName": name,
                "RuntimeEnvironment": "FLINK-1_15",
                "ServiceExecutionRole": "arn:aws:iam::000000000000:role/kda-service-role",
                "Tags": created_with.iter().map(|(k, v)| json!({"Key": k, "Value": v})).collect::<Vec<_>>(),
            }),
        )
        .await;
        assert_eq!(status, 200, "case {case}: CreateApplication got {body}");

        let mut expected: Vec<(String, String)> = created_with
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        expected.sort();
        let actual = tags_of(&router, &name).await;
        assert_eq!(
            actual, expected,
            "case {case}: expected the tags the create stored, got {actual:?}"
        );

        let (status, answered) = call(
            &router,
            "TagResource",
            &json!({
                "ResourceARN": application_arn(&name),
                "Tags": [{"Key": "added", "Value": "later"}],
            }),
        )
        .await;
        assert_eq!(status, 200, "case {case}: TagResource got {answered}");
        expected.push(("added".to_string(), "later".to_string()));
        expected.sort();
        let actual = tags_of(&router, &name).await;
        assert_eq!(
            actual, expected,
            "case {case}: expected the create's tags plus the added one, got {actual:?}"
        );
    }
}

/// `UntagResource` removes the keys it names and leaves every other tag assigned; a key
/// the application carries no tag under leaves it exactly as it was.
#[tokio::test]
async fn untagging_removes_only_the_keys_it_names() {
    let router = router();
    let stored = [("env", "prod"), ("owner", "data"), ("tier", "gold")];
    // (case, the keys removed, the keys that remain)
    let cases: [(&str, &[&str], &[&str]); 5] = [
        ("one-key", &["env"], &["owner", "tier"]),
        ("two-keys", &["env", "tier"], &["owner"]),
        ("every-key", &["env", "owner", "tier"], &[]),
        ("unknown-key", &["absent"], &["env", "owner", "tier"]),
        (
            "known-and-unknown",
            &["owner", "absent"],
            &["env", "tier"],
        ),
    ];

    for (case, removed, remaining) in cases {
        let name = format!("untagged-{case}");
        create(&router, &name, "FLINK-1_20").await;
        let (status, answered) = call(
            &router,
            "TagResource",
            &json!({
                "ResourceARN": application_arn(&name),
                "Tags": stored.iter().map(|(k, v)| json!({"Key": k, "Value": v})).collect::<Vec<_>>(),
            }),
        )
        .await;
        assert_eq!(status, 200, "case {case}: TagResource got {answered}");

        let (status, answered) = call(
            &router,
            "UntagResource",
            &json!({"ResourceARN": application_arn(&name), "TagKeys": removed}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: UntagResource got {status} {answered}"
        );
        assert_eq!(
            answered,
            json!({}),
            "case {case}: UntagResource answers an empty envelope, got {answered}"
        );

        let expected: Vec<(String, String)> = stored
            .iter()
            .filter(|(key, _)| remaining.contains(key))
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        let actual = tags_of(&router, &name).await;
        assert_eq!(
            actual, expected,
            "case {case}: expected only the keys that were not removed, got {actual:?}"
        );
    }
}

/// "The maximum number of user-defined application tags is 50" (the `TagResource` doc):
/// what the ceiling counts is the set the APPLICATION would then carry, so a call is
/// refused by what it would leave rather than by how many tags it supplies — and a call
/// that only replaces values of keys already assigned is accepted at the ceiling.
#[tokio::test]
async fn the_user_tag_ceiling_counts_the_tags_the_application_would_carry() {
    let router = router();
    // (case, tags already assigned, tags the call supplies, whether the call supplies the
    // keys already assigned rather than fresh ones, expected http status)
    let cases: [(&str, usize, usize, bool, u16); 6] = [
        ("under-the-ceiling", 10, 10, false, 200),
        ("exactly-the-ceiling", 25, 25, false, 200),
        ("one-past-the-ceiling", 25, 26, false, 400),
        ("far-past-the-ceiling", 40, 60, false, 400),
        ("replacing-at-the-ceiling", 50, 50, true, 200),
        ("one-fresh-key-past-the-ceiling", 50, 1, false, 400),
    ];

    for (case, held, supplied, replacing, expected) in cases {
        let name = format!("ceiling-{case}");
        let mut body = json!({
            "ApplicationName": name,
            "RuntimeEnvironment": "FLINK-1_20",
            "ServiceExecutionRole": "arn:aws:iam::000000000000:role/kda-service-role",
        });
        body["Tags"] = json!((0..held)
            .map(|n| json!({"Key": format!("held{n}"), "Value": "held"}))
            .collect::<Vec<_>>());
        let (status, created) = call(&router, "CreateApplication", &body).await;
        assert_eq!(status, 200, "case {case}: CreateApplication got {created}");

        // A call that supplies the keys already assigned only REPLACES their values, so
        // the set it would leave is no larger; one that supplies fresh keys grows it.
        // What the ceiling bounds is that resulting set.
        let prefix = if replacing { "held" } else { "fresh" };
        let (status, answered) = call(
            &router,
            "TagResource",
            &json!({
                "ResourceARN": application_arn(&name),
                "Tags": (0..supplied)
                    .map(|n| json!({"Key": format!("{prefix}{n}"), "Value": "supplied"}))
                    .collect::<Vec<_>>(),
            }),
        )
        .await;
        assert_eq!(
            status, expected,
            "case {case}: expected {expected}, got {status} {answered}"
        );
        if expected == 400 {
            assert_eq!(
                error_code(&answered),
                "TooManyTagsException",
                "case {case}: expected TooManyTagsException, got {answered}"
            );
            // A refused call assigns nothing: the application still carries what it held.
            assert_eq!(
                tags_of(&router, &name).await.len(),
                held,
                "case {case}: a refused TagResource must assign nothing"
            );
        } else {
            let expected_total = if replacing {
                held.max(supplied)
            } else {
                held + supplied
            };
            assert_eq!(
                tags_of(&router, &name).await.len(),
                expected_total,
                "case {case}: expected the merged tag set to hold {expected_total} tags"
            );
        }
    }
}

/// A `ResourceARN` that is not the ARN of one of this service's applications names
/// nothing to tag — a parameter violation — while one that IS such an ARN but names an
/// application this service does not hold is a lookup miss. The same two refusals from
/// every tagging call.
#[tokio::test]
async fn a_resource_arn_naming_no_stored_application_is_refused() {
    let router = router();
    create(&router, "tag-target", "FLINK-1_20").await;

    // (case, the ARN, the expected error code)
    let cases: [(&str, String, &str); 6] = [
        (
            "another-vendor",
            "arn:aws:kinesis:us-east-1:000000000000:stream/not-an-application".to_string(),
            "InvalidArgumentException",
        ),
        (
            "another-resource-type",
            "arn:aws:kinesisanalytics:us-east-1:000000000000:snapshot/snap".to_string(),
            "InvalidArgumentException",
        ),
        (
            "no-resource-id",
            "arn:aws:kinesisanalytics:us-east-1:000000000000:application/".to_string(),
            "InvalidArgumentException",
        ),
        (
            "absent-application",
            application_arn("never-created"),
            "ResourceNotFoundException",
        ),
        (
            "another-region",
            "arn:aws:kinesisanalytics:eu-west-1:000000000000:application/tag-target".to_string(),
            "ResourceNotFoundException",
        ),
        (
            "another-account",
            "arn:aws:kinesisanalytics:us-east-1:999999999999:application/tag-target".to_string(),
            "ResourceNotFoundException",
        ),
    ];

    for (case, resource_arn, expected) in cases {
        // (operation, the body that names the ARN)
        let calls = [
            (
                "TagResource",
                json!({"ResourceARN": resource_arn, "Tags": [{"Key": "k", "Value": "v"}]}),
            ),
            (
                "UntagResource",
                json!({"ResourceARN": resource_arn, "TagKeys": ["k"]}),
            ),
            (
                "ListTagsForResource",
                json!({"ResourceARN": resource_arn}),
            ),
        ];
        for (operation, body) in calls {
            let (status, refused) = call(&router, operation, &body).await;
            assert_eq!(
                (status, error_code(&refused).as_str()),
                (400, expected),
                "case {case}/{operation}: expected 400 {expected}, got {status} {refused}"
            );
            assert!(
                !error_message(&refused).is_empty(),
                "case {case}/{operation}: expected a message, got {refused}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Version history
// ---------------------------------------------------------------------------

/// Bump `name`'s version by applying a parallelism update against `current`, and answer
/// the version the service moved it to.
async fn update_parallelism(
    router: &smithy_server_runtime::MultiServiceRouter,
    name: &str,
    current: i64,
    parallelism: i64,
) -> i64 {
    let (status, updated) = call(
        router,
        "UpdateApplication",
        &json!({
            "ApplicationName": name,
            "CurrentApplicationVersionId": current,
            "ApplicationConfigurationUpdate": {
                "FlinkApplicationConfigurationUpdate": {
                    "ParallelismConfigurationUpdate": {
                        "ConfigurationTypeUpdate": "CUSTOM",
                        "ParallelismUpdate": parallelism,
                    }
                }
            },
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {name:?} v{current}: UpdateApplication got {updated}"
    );
    updated["ApplicationDetail"]["ApplicationVersionId"]
        .as_i64()
        .unwrap_or_else(|| panic!("case {name:?}: expected a version id, got {updated}"))
}

/// The parallelism one version of an application ran under, as
/// `DescribeApplicationVersion` reports it.
async fn version_parallelism(
    router: &smithy_server_runtime::MultiServiceRouter,
    name: &str,
    version_id: i64,
) -> (String, i64) {
    let (status, body) = call(
        router,
        "DescribeApplicationVersion",
        &json!({"ApplicationName": name, "ApplicationVersionId": version_id}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {name:?} v{version_id}: DescribeApplicationVersion got {body}"
    );
    let detail = &body["ApplicationVersionDetail"];
    assert_eq!(
        detail["ApplicationVersionId"].as_i64(),
        Some(version_id),
        "case {name:?} v{version_id}: expected the version asked for, got {detail}"
    );
    let described = &detail["ApplicationConfigurationDescription"]
        ["FlinkApplicationConfigurationDescription"]["ParallelismConfigurationDescription"];
    (
        described["ConfigurationType"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        described["Parallelism"].as_i64().unwrap_or_default(),
    )
}

/// Every version an application has is listed oldest-first and is readable by id, and a
/// version it has LEFT reports the configuration that version ran under rather than the
/// one in force.
#[tokio::test]
async fn every_version_an_application_has_is_listed_and_readable_by_id() {
    let router = router();
    // (case, how many updates land after the create)
    for (case, updates) in [("one-version", 0_i64), ("two-versions", 1), ("four-versions", 3)] {
        let name = format!("versioned-{case}");
        create(&router, &name, "FLINK-1_20").await;

        // Each update raises the parallelism, so every version ran under a value of its
        // own and a reader of an old version cannot be answered from the current record.
        for update in 0..updates {
            let version = update_parallelism(&router, &name, update + 1, update + 2).await;
            assert_eq!(
                version,
                update + 2,
                "case {case}: expected the next version id, got {version}"
            );
        }

        let (status, listed) = call(
            &router,
            "ListApplicationVersions",
            &json!({"ApplicationName": name}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: ListApplicationVersions got {listed}"
        );
        let summaries = listed["ApplicationVersionSummaries"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case}: expected the summaries member, got {listed}"))
            .clone();
        assert_eq!(
            summaries
                .iter()
                .map(|summary| summary["ApplicationVersionId"].as_i64())
                .collect::<Vec<_>>(),
            (1..=updates + 1).map(Some).collect::<Vec<_>>(),
            "case {case}: expected every version oldest-first, got {listed}"
        );
        for summary in &summaries {
            assert_eq!(
                summary["ApplicationStatus"].as_str(),
                Some("READY"),
                "case {case}: expected the status of the version summarised, got {summary}"
            );
        }
        assert!(
            listed.get("NextToken").is_none_or(Value::is_null),
            "case {case}: a page that holds every version carries no token, got {listed}"
        );

        // The create's version ran under the service's defaults; each update's version
        // under the parallelism that update asked for.
        assert_eq!(
            version_parallelism(&router, &name, 1).await,
            ("DEFAULT".to_string(), 1),
            "case {case}: expected the configuration version 1 ran under"
        );
        for update in 0..updates {
            let version = update + 2;
            assert_eq!(
                version_parallelism(&router, &name, version).await,
                ("CUSTOM".to_string(), update + 2),
                "case {case}: expected the configuration version {version} ran under"
            );
        }
    }
}

/// A version id the application was never on is a lookup miss, at every end of its
/// history; the modelled range itself is refused at the boundary.
#[tokio::test]
async fn a_version_the_application_never_had_is_refused() {
    let router = router();
    let name = "version-misses";
    create(&router, name, "FLINK-1_20").await;
    update_parallelism(&router, name, 1, 2).await;

    // (case, the version id asked for, expected http status, expected error code)
    let cases: [(&str, i64, u16, &str); 4] = [
        ("past-the-end", 3, 400, "ResourceNotFoundException"),
        ("far-past-the-end", 999_999_999, 400, "ResourceNotFoundException"),
        ("below-the-modelled-range", 0, 400, "ValidationException"),
        (
            "above-the-modelled-range",
            1_000_000_000,
            400,
            "ValidationException",
        ),
    ];
    for (case, version_id, expected_status, expected_code) in cases {
        let (status, refused) = call(
            &router,
            "DescribeApplicationVersion",
            &json!({"ApplicationName": name, "ApplicationVersionId": version_id}),
        )
        .await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (expected_status, expected_code),
            "case {case}: expected {expected_status} {expected_code}, got {status} {refused}"
        );
    }

    // An application that is not stored at all is a miss for both version readers.
    for operation in ["DescribeApplicationVersion", "ListApplicationVersions"] {
        let (status, refused) = call(
            &router,
            operation,
            &json!({"ApplicationName": "never-created", "ApplicationVersionId": 1}),
        )
        .await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (400, "ResourceNotFoundException"),
            "case absent-application/{operation}: got {status} {refused}"
        );
    }
}

/// `ListApplicationVersions` pages at `Limit` and resumes from the token it issued, so a
/// client walking the pages sees every version exactly once; a token it never issued names
/// no page.
#[tokio::test]
async fn version_pages_are_bounded_by_limit_and_resumed_by_the_token() {
    let router = router();
    let name = "version-pages";
    create(&router, name, "FLINK-1_20").await;
    for update in 0..4 {
        update_parallelism(&router, name, update + 1, update + 2).await;
    }

    for limit in [1_i64, 2, 3, 5, 50] {
        let mut walked: Vec<i64> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut body = json!({"ApplicationName": name, "Limit": limit});
            if let Some(token) = token.as_deref() {
                body["NextToken"] = json!(token);
            }
            let (status, page) = call(&router, "ListApplicationVersions", &body).await;
            assert_eq!(status, 200, "case limit {limit}: got {status} {page}");
            let summaries = page["ApplicationVersionSummaries"]
                .as_array()
                .unwrap_or_else(|| panic!("case limit {limit}: expected summaries, got {page}"))
                .clone();
            assert!(
                summaries.len() <= limit as usize,
                "case limit {limit}: a page holds at most Limit entries, got {page}"
            );
            walked.extend(
                summaries
                    .iter()
                    .filter_map(|summary| summary["ApplicationVersionId"].as_i64()),
            );
            token = page["NextToken"].as_str().map(str::to_string);
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            walked,
            (1..=5).collect::<Vec<i64>>(),
            "case limit {limit}: expected every version once, in order"
        );
    }

    let (status, refused) = call(
        &router,
        "ListApplicationVersions",
        &json!({"ApplicationName": name, "NextToken": "not-a-page"}),
    )
    .await;
    assert_eq!(
        (status, error_code(&refused).as_str()),
        (400, "InvalidArgumentException"),
        "case bad-token: expected InvalidArgumentException, got {status} {refused}"
    );
}

/// The version readers are "supported only for Managed Service for Apache Flink", so an
/// application of another runtime family is refused with the modelled
/// `UnsupportedOperationException` — whatever version it names — while every version of a
/// Flink application is served.
#[tokio::test]
async fn version_reads_are_supported_only_for_the_flink_runtime_family() {
    let router = router();
    // (the RuntimeEnvironment, whether its versions can be read) — one row per family the
    // `RuntimeEnvironment` enum carries
    // (`smithy_query(query="shape",shape="RuntimeEnvironment")`): the plain `FLINK-`
    // applications and the `ZEPPELIN-FLINK-` Studio notebooks.
    let cases: [(&str, bool); 6] = [
        ("FLINK-1_15", true),
        ("FLINK-1_20", true),
        ("FLINK-2_3", true),
        ("ZEPPELIN-FLINK-1_0", false),
        ("ZEPPELIN-FLINK-2_0", false),
        ("ZEPPELIN-FLINK-3_0", false),
    ];

    for (runtime, supported) in cases {
        let name = format!("runtime-{}", runtime.to_lowercase().replace('_', "-"));
        create(&router, &name, runtime).await;

        for (operation, body) in [
            (
                "DescribeApplicationVersion",
                json!({"ApplicationName": name, "ApplicationVersionId": 1}),
            ),
            (
                "ListApplicationVersions",
                json!({"ApplicationName": name}),
            ),
        ] {
            let (status, answered) = call(&router, operation, &body).await;
            if supported {
                assert_eq!(
                    status, 200,
                    "case {runtime}/{operation}: expected 200, got {status} {answered}"
                );
                continue;
            }
            assert_eq!(
                (status, error_code(&answered).as_str()),
                (400, "UnsupportedOperationException"),
                "case {runtime}/{operation}: expected UnsupportedOperationException, got \
                 {status} {answered}"
            );
            assert_eq!(
                error_message(&answered),
                format!(
                    "{operation} is not supported for applications with runtime environment \
                     {runtime}."
                ),
                "case {runtime}/{operation}: expected the refusal to name the operation and \
                 the runtime, got {answered}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Snapshot listing
// ---------------------------------------------------------------------------

/// `ListApplicationSnapshots` reports the snapshots taken of one application in name
/// order, pages at `Limit`, and reports the member even for an application that has none.
#[tokio::test]
async fn the_snapshots_of_an_application_are_listed_in_name_order() {
    let router = router();
    let name = "snapshot-listing";
    create_startable(&router, name, "FLINK-1_20").await;

    // An application that has taken no snapshot answers the member, empty: "no snapshots"
    // is an answer a paginating client can walk.
    let (status, listed) = call(
        &router,
        "ListApplicationSnapshots",
        &json!({"ApplicationName": name}),
    )
    .await;
    assert_eq!(status, 200, "case empty: got {status} {listed}");
    assert_eq!(
        listed["SnapshotSummaries"],
        json!([]),
        "case empty: expected a present but empty list, got {listed}"
    );

    let (status, started) = call(&router, "StartApplication", &json!({"ApplicationName": name})).await;
    assert_eq!(status, 200, "StartApplication got {status} {started}");
    tokio::time::sleep(SETTLE_WAIT).await;

    // Taken out of name order, so what the reader reports cannot be the order they were
    // created in.
    let taken = ["snap-c", "snap-a", "snap-b"];
    for snapshot in taken {
        let (status, answered) = call(
            &router,
            "CreateApplicationSnapshot",
            &json!({"ApplicationName": name, "SnapshotName": snapshot}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {snapshot}: CreateApplicationSnapshot got {status} {answered}"
        );
    }

    let (status, listed) = call(
        &router,
        "ListApplicationSnapshots",
        &json!({"ApplicationName": name}),
    )
    .await;
    assert_eq!(status, 200, "case listed: got {status} {listed}");
    let mut expected: Vec<&str> = taken.to_vec();
    expected.sort_unstable();
    assert_eq!(
        listed["SnapshotSummaries"]
            .as_array()
            .unwrap_or_else(|| panic!("case listed: expected summaries, got {listed}"))
            .iter()
            .map(|summary| summary["SnapshotName"].as_str().unwrap_or_default())
            .collect::<Vec<_>>(),
        expected,
        "case listed: expected the snapshots in name order, got {listed}"
    );

    // Every summary is the record the writer stored, which `DescribeApplicationSnapshot`
    // reports from the same place.
    for summary in listed["SnapshotSummaries"].as_array().unwrap() {
        let snapshot = summary["SnapshotName"].as_str().unwrap_or_default();
        assert_eq!(
            summary["ApplicationVersionId"].as_i64(),
            Some(1),
            "case {snapshot}: expected the version it was taken of, got {summary}"
        );
        assert_eq!(
            summary["RuntimeEnvironment"].as_str(),
            Some("FLINK-1_20"),
            "case {snapshot}: expected the application's runtime, got {summary}"
        );
        let (status, described) = call(
            &router,
            "DescribeApplicationSnapshot",
            &json!({"ApplicationName": name, "SnapshotName": snapshot}),
        )
        .await;
        assert_eq!(status, 200, "case {snapshot}: got {status} {described}");
        assert_eq!(
            described["SnapshotDetails"], *summary,
            "case {snapshot}: the list and the reader report one record"
        );
    }

    // Paging walks every snapshot exactly once, whatever the page size.
    for limit in [1_i64, 2, 3, 50] {
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut body = json!({"ApplicationName": name, "Limit": limit});
            if let Some(token) = token.as_deref() {
                body["NextToken"] = json!(token);
            }
            let (status, page) = call(&router, "ListApplicationSnapshots", &body).await;
            assert_eq!(status, 200, "case limit {limit}: got {status} {page}");
            let summaries = page["SnapshotSummaries"]
                .as_array()
                .unwrap_or_else(|| panic!("case limit {limit}: expected summaries, got {page}"))
                .clone();
            assert!(
                summaries.len() <= limit as usize,
                "case limit {limit}: a page holds at most Limit entries, got {page}"
            );
            walked.extend(
                summaries
                    .iter()
                    .map(|summary| summary["SnapshotName"].as_str().unwrap_or_default().to_string()),
            );
            token = page["NextToken"].as_str().map(str::to_string);
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            walked,
            expected
                .iter()
                .map(|name| name.to_string())
                .collect::<Vec<String>>(),
            "case limit {limit}: expected every snapshot once, in name order"
        );
    }

    // This operation models no `ResourceNotFoundException`, so a name it holds no
    // application for is the parameter violation its model does declare.
    let (status, refused) = call(
        &router,
        "ListApplicationSnapshots",
        &json!({"ApplicationName": "never-created"}),
    )
    .await;
    assert_eq!(
        (status, error_code(&refused).as_str()),
        (400, "InvalidArgumentException"),
        "case absent-application: got {status} {refused}"
    );
}
