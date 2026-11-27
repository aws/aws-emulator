// SPDX-License-Identifier: CC0-1.0

//! The `GetDetector` response projection, end to end over the wire.
//!
//! Every request goes through [`smithy_server_runtime::MultiServiceRouter::dispatch`]
//! (the harness lives in the `detector_lifecycle` suite), so routing, restJson1
//! serialization and the handler are exercised together.
//!
//! The rules under test, each asserted over a SET of inputs rather than one recorded
//! call:
//!
//! 1. Every node of the deprecated `DataSources` view reports the STATUS of the
//!    feature that governs it: one stored switch, read back under either name. The
//!    node carries only that status — the model declares no other member on it
//!    (`smithy_query(shape="DataSourceConfigurationsResult")`) — so the instant the
//!    switch last CHANGED is the `Features` view's to report, and it moves only when
//!    the switch changes.
//! 2. A data source's reported status is the stored configuration, never a function
//!    of the detector's own `Status`; the three foundational sources (CloudTrail
//!    management events, DNS logs, VPC flow logs) have no switch at all and always
//!    report `ENABLED`.
//! 3. `Features` reports every `DetectorFeatureResult` member exactly once, in the
//!    service's canonical order, each entry carrying `name`, `status` and
//!    `updatedAt`.
//! 4. `Tags` is always a map — empty, not absent, for a detector nothing tagged.
//! 5. `DataSources.MalwareProtection.ServiceRole` is the Malware Protection
//!    service-linked role, a different role from the detector-level `ServiceRole`.

use serde_json::{json, Value};

use super::detector_lifecycle::{call, create_detector, new_router, Router};

/// The `DataSources` nodes a request can switch, paired with the `DetectorFeature`
/// that governs each and the status a bare `CreateDetector` leaves it in.
///
/// The node paths are the `jsonName`s of `DataSourceConfigurationsResult` and its
/// nested result shapes (`smithy_query(shape="DataSourceConfigurationsResult")`);
/// the feature names are `DetectorFeature` members
/// (`smithy_query(shape="DetectorFeature")`). A feature the result shape has no node
/// for — RDS login activity, for one — is reported under `Features` alone, which is
/// the whole-set rule below.
const GOVERNED_NODES: [(&str, &[&str], &str); 3] = [
    ("S3_DATA_EVENTS", &["s3Logs"], "ENABLED"),
    ("EKS_AUDIT_LOGS", &["kubernetes", "auditLogs"], "ENABLED"),
    (
        "EBS_MALWARE_PROTECTION",
        &["malwareProtection", "scanEc2InstanceWithFindings", "ebsVolumes"],
        "ENABLED",
    ),
];

/// The optional features a bare create leaves DISABLED; every other reported
/// feature starts ENABLED. Runtime Monitoring is the exception `CreateDetector`
/// documents to "all the optional features are enabled by default", and AI Analyst
/// is the one the Investigation API documents by telling a caller to enable it on
/// the detector before using that API.
const OPT_IN_FEATURES: [&str; 2] = ["RUNTIME_MONITORING", "AI_ANALYST"];

/// The data sources GuardDuty always consumes: no `DetectorFeature` member and no
/// `DataSourceConfigurations` member names them, so nothing can switch them off.
const FOUNDATIONAL_NODES: [(&str, &str); 3] = [
    ("CLOUD_TRAIL", "cloudTrail"),
    ("DNS_LOGS", "dnsLogs"),
    ("FLOW_LOGS", "flowLogs"),
];

/// Every feature a detector reports, in the order it reports them: the three
/// foundational sources first, in the order `DataSourceConfigurationsResult` lists
/// them (`cloudTrail`, `dnsLogs`, `flowLogs`), then every `DetectorFeature` member
/// in model order. The names are the `DetectorFeatureResult` enum's values
/// (`smithy_query(shape="DetectorFeatureResult")`).
const REPORTED_FEATURES: [&str; 12] = [
    "CLOUD_TRAIL",
    "DNS_LOGS",
    "FLOW_LOGS",
    "S3_DATA_EVENTS",
    "EKS_AUDIT_LOGS",
    "EBS_MALWARE_PROTECTION",
    "RDS_LOGIN_EVENTS",
    "LAMBDA_NETWORK_LOGS",
    "EKS_RUNTIME_MONITORING",
    "RUNTIME_MONITORING",
    "AI_PROTECTION",
    "AI_ANALYST",
];

/// The role name and service principal of the Malware Protection service-linked
/// role, which `MalwareProtectionConfigurationResult.ServiceRole` ("The GuardDuty
/// Malware Protection service role") reports.
const MALWARE_PROTECTION_ROLE_SUFFIX: &str =
    "role/aws-service-role/malware-protection.guardduty.amazonaws.com/AWSServiceRoleForAmazonGuardDutyMalwareProtection";

/// Create a detector from `body` and read it back, returning the `GetDetector`
/// response.
async fn created_then_read(router: &Router, body: Value) -> Value {
    let (status, created) = create_detector(router, body.clone()).await;
    assert_eq!(status, 200, "create {body}: expected 200, got {status} {created:?}");
    let id = created["detectorId"]
        .as_str()
        .unwrap_or_else(|| panic!("create {body}: expected a detectorId, got {created:?}"))
        .to_string();
    read(router, &id).await
}

async fn read(router: &Router, id: &str) -> Value {
    let (status, got) = call(router, "GET", &format!("/detector/{id}"), None).await;
    assert_eq!(status, 200, "get-detector {id}: expected 200, got {status} {got:?}");
    got
}

async fn update(router: &Router, id: &str, body: Value) {
    let (status, got) = call(router, "POST", &format!("/detector/{id}"), Some(body.clone())).await;
    assert_eq!(
        status, 200,
        "update-detector {id} with {body}: expected 200, got {status} {got:?}"
    );
}

/// The detector id in a `CreateDetector` response.
fn detector_id(created: &Value) -> String {
    created["detectorId"]
        .as_str()
        .unwrap_or_else(|| panic!("expected a detectorId, got {created:?}"))
        .to_string()
}

/// Walk a `dataSources` node path.
fn node<'a>(got: &'a Value, path: &[&str]) -> &'a Value {
    let mut cursor = &got["dataSources"];
    for segment in path {
        cursor = &cursor[*segment];
    }
    cursor
}

/// One reported feature entry.
fn feature<'a>(got: &'a Value, name: &str) -> &'a Value {
    got["features"]
        .as_array()
        .and_then(|features| {
            features
                .iter()
                .find(|entry| entry["name"].as_str() == Some(name))
        })
        .unwrap_or_else(|| panic!("feature {name} is not reported at all: got {got:?}"))
}

/// The members a `dataSources` node carries: its `Status`, and nothing else. The
/// result shapes under `DataSourceConfigurationsResult` declare `Status` — and, for
/// the EBS volumes node, the `Reason` a source was not enabled, which nothing stopped
/// here (`smithy_query(shape="DataSourceConfigurationsResult")`).
const NODE_MEMBERS: [&str; 2] = ["status", "reason"];

/// Assert a `dataSources` node carries the members the model declares for it, and no
/// instant of its own: a data source is a projection of the feature that governs it,
/// and `UpdatedAt` is that feature's member.
fn assert_node_members(value: &Value, case: &str) {
    let object = value
        .as_object()
        .unwrap_or_else(|| panic!("{case}: expected a data-source node object, got {value:?}"));
    for member in object.keys() {
        assert!(
            NODE_MEMBERS.contains(&member.as_str()),
            "{case}: a data-source node carries only {NODE_MEMBERS:?}, got the member {member:?} \
             in {value:?}"
        );
    }
}

/// The epoch instant a `features` entry carries (`DetectorFeatureConfigurationResult`
/// models `UpdatedAt` as a `Timestamp`, so the wire carries epoch seconds).
fn feature_instant(value: &Value, case: &str) -> f64 {
    let instant = value["updatedAt"]
        .as_f64()
        .unwrap_or_else(|| panic!("{case}: expected an updatedAt timestamp, got {value:?}"));
    assert!(
        instant > 0.0,
        "{case}: expected a positive epoch instant, got {instant}"
    );
    instant
}

/// How close two epoch instants are before they name the same one: the instants this
/// service stores are milliseconds, so anything under one is the same instant.
const SAME_INSTANT: f64 = 0.001;

// ---------------------------------------------------------------------------
// Rules 1 and 2.
// ---------------------------------------------------------------------------

/// Each switchable data source reports its governing feature's status, and that
/// feature's own entry carries the instant the switch last changed — written through
/// either view, read back through both.
#[tokio::test]
async fn a_data_source_node_reports_the_status_of_the_feature_that_governs_it() {
    for (feature_name, path, default_status) in GOVERNED_NODES {
        let case = format!("{feature_name} -> dataSources.{}", path.join("."));
        let router = new_router();
        let (_, created) = create_detector(&router, json!({"enable": true})).await;
        let id = detector_id(&created);

        // Both views start at the status a bare create left the switch in, and agree
        // on the instant.
        let got = read(&router, &id).await;
        let mut last = assert_views_agree(&got, feature_name, path, default_status, &case);

        // Walk the whole value space of the switch, ending where it started.
        let other = if default_status == "ENABLED" { "DISABLED" } else { "ENABLED" };
        for target in [other, default_status, other] {
            update(
                &router,
                &id,
                json!({"features": [{"name": feature_name, "status": target}]}),
            )
            .await;
            let got = read(&router, &id).await;
            let moved = assert_views_agree(&got, feature_name, path, target, &case);
            assert!(
                moved > last,
                "{case}: a status change to {target} must move updatedAt: was {last:?}, got {moved:?}"
            );
            last = moved;

            // Re-applying the SAME status changes nothing, so the instant records
            // when the switch last CHANGED, not when it was last written.
            update(
                &router,
                &id,
                json!({"features": [{"name": feature_name, "status": target}]}),
            )
            .await;
            let got = read(&router, &id).await;
            let again = assert_views_agree(&got, feature_name, path, target, &case);
            assert!(
                (again - last).abs() < SAME_INSTANT,
                "{case}: re-applying {target} must not move updatedAt: was {last:?}, got {again:?}"
            );
        }
    }
}

/// Assert both views of one switch report `expected`, and return the instant the
/// FEATURE view reports for it.
fn assert_views_agree(
    got: &Value,
    feature_name: &str,
    path: &[&str],
    expected: &str,
    case: &str,
) -> f64 {
    let node = node(got, path);
    assert_eq!(
        node["status"],
        json!(expected),
        "{case}: dataSources.{} expected {expected}, got {got:?}",
        path.join(".")
    );
    assert_node_members(node, case);
    let entry = feature(got, feature_name);
    assert_eq!(
        entry["status"],
        json!(expected),
        "{case}: feature {feature_name} expected {expected}, got {got:?}"
    );
    feature_instant(entry, case)
}

/// Writing the switch through the deprecated `DataSources` input is read back
/// through both views too.
#[tokio::test]
async fn the_deprecated_data_sources_input_writes_the_same_switch() {
    // (the DataSources body, the feature it writes, the node it is read back at)
    let cases: [(Value, &str, &[&str]); 3] = [
        (json!({"s3Logs": {"enable": false}}), "S3_DATA_EVENTS", &["s3Logs"]),
        (
            json!({"kubernetes": {"auditLogs": {"enable": false}}}),
            "EKS_AUDIT_LOGS",
            &["kubernetes", "auditLogs"],
        ),
        (
            json!({"malwareProtection": {"scanEc2InstanceWithFindings": {"ebsVolumes": false}}}),
            "EBS_MALWARE_PROTECTION",
            &["malwareProtection", "scanEc2InstanceWithFindings", "ebsVolumes"],
        ),
    ];

    for (data_sources, feature_name, path) in cases {
        let case = format!("dataSources {data_sources} -> {feature_name}");
        let router = new_router();
        let got =
            created_then_read(&router, json!({"enable": true, "dataSources": data_sources})).await;
        assert_views_agree(&got, feature_name, path, "DISABLED", &case);
    }
}

/// The reported statuses are the stored CONFIGURATION: disabling the detector moves
/// its own `Status` and nothing else. The foundational sources have no switch, so
/// they report `ENABLED` whatever the detector's `Enable` was.
#[tokio::test]
async fn a_data_source_status_does_not_follow_the_detectors_enable_flag() {
    for enable in [true, false] {
        let case = format!("enable {enable}");
        let router = new_router();
        let (_, created) = create_detector(&router, json!({"enable": enable})).await;
        let id = detector_id(&created);
        let got = read(&router, &id).await;
        assert_eq!(
            got["status"],
            json!(if enable { "ENABLED" } else { "DISABLED" }),
            "{case}: the detector's own status is what Enable sets, got {got:?}"
        );
        for (feature_name, json_name) in FOUNDATIONAL_NODES {
            assert_eq!(
                node(&got, &[json_name])["status"],
                json!("ENABLED"),
                "{case}: dataSources.{json_name} has no switch, so it reports ENABLED, got {got:?}"
            );
            assert_eq!(
                feature(&got, feature_name)["status"],
                json!("ENABLED"),
                "{case}: feature {feature_name} has no switch, so it reports ENABLED, got {got:?}"
            );
            assert_node_members(node(&got, &[json_name]), &case);
        }
        for (feature_name, path, default_status) in GOVERNED_NODES {
            assert_eq!(
                node(&got, path)["status"],
                json!(default_status),
                "{case}: dataSources.{} must report the status a bare create left it in \
                 ({default_status}), got {got:?}",
                path.join(".")
            );
            assert_eq!(
                feature(&got, feature_name)["status"],
                json!(default_status),
                "{case}: feature {feature_name} must report {default_status}, got {got:?}"
            );
        }

        // And toggling the detector itself leaves every data source where it was.
        update(&router, &id, json!({"enable": !enable})).await;
        let after = read(&router, &id).await;
        assert_eq!(
            after["dataSources"], got["dataSources"],
            "{case}: toggling Enable must not rewrite the dataSources configuration"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 3.
// ---------------------------------------------------------------------------

/// `Features` reports the whole `DetectorFeatureResult` set, once each, in the
/// canonical order, for every shape of create and after an update.
#[tokio::test]
async fn features_are_reported_over_the_whole_set_in_canonical_order() {
    let creates: [Value; 4] = [
        json!({"enable": true}),
        json!({"enable": false}),
        json!({"enable": true, "features": [{"name": "S3_DATA_EVENTS", "status": "DISABLED"}]}),
        json!({"enable": true, "dataSources": {"s3Logs": {"enable": true}}}),
    ];

    for body in creates {
        let case = format!("create {body}");
        let router = new_router();
        let (_, created) = create_detector(&router, body.clone()).await;
        let id = detector_id(&created);

        for stage in ["after create", "after update"] {
            let got = read(&router, &id).await;
            let reported: Vec<String> = got["features"]
                .as_array()
                .unwrap_or_else(|| panic!("{case} {stage}: expected a features list, got {got:?}"))
                .iter()
                .map(|entry| entry["name"].as_str().unwrap_or_default().to_string())
                .collect();
            assert_eq!(
                reported,
                REPORTED_FEATURES.map(String::from).to_vec(),
                "{case} {stage}: features must be the whole DetectorFeatureResult set, once each, \
                 in canonical order, got {got:?}"
            );
            for name in REPORTED_FEATURES {
                let entry = feature(&got, name);
                assert!(
                    matches!(entry["status"].as_str(), Some("ENABLED" | "DISABLED")),
                    "{case} {stage}: feature {name} must report a FeatureStatus, got {entry:?}"
                );
            feature_instant(entry, &format!("{case} {stage}: feature {name}"));
            }
            if stage == "after create" {
                update(
                    &router,
                    &id,
                    json!({"features": [{"name": "RUNTIME_MONITORING", "status": "ENABLED"},
                                        {"name": "EKS_RUNTIME_MONITORING", "status": "DISABLED"}]}),
                )
                .await;
            }
        }
    }
}

/// A bare create enables every feature it reports except the opt-in ones, and an
/// opt-in feature is reported enabled as soon as a request asks for it.
#[tokio::test]
async fn a_bare_create_enables_every_reported_feature_except_the_opt_in_ones() {
    let router = new_router();
    let got = created_then_read(&router, json!({"enable": true})).await;
    for name in REPORTED_FEATURES {
        let expected = if OPT_IN_FEATURES.contains(&name) {
            "DISABLED"
        } else {
            "ENABLED"
        };
        assert_eq!(
            feature(&got, name)["status"],
            json!(expected),
            "bare create: feature {name} expected {expected}, got {got:?}"
        );
    }

    for name in OPT_IN_FEATURES {
        let router = new_router();
        let got = created_then_read(
            &router,
            json!({"enable": true, "features": [{"name": name, "status": "ENABLED"}]}),
        )
        .await;
        assert_eq!(
            feature(&got, name)["status"],
            json!("ENABLED"),
            "create asking for {name}: expected ENABLED, got {got:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 4.
// ---------------------------------------------------------------------------
/// `Tags` is always a map: a detector nothing tagged reports `{}`, not nothing.
#[tokio::test]
async fn a_detector_always_reports_a_tag_map() {
    let cases: [Value; 4] = [
        json!({}),
        json!({"one": "1"}),
        json!({"team": "security", "env": "test", "owner": "sre"}),
        json!({"empty-value": ""}),
    ];

    for tags in cases {
        let case = format!("tags {tags}");
        let router = new_router();
        let mut body = json!({"enable": true});
        let expected = tags.as_object().cloned().unwrap_or_default();
        if !expected.is_empty() {
            body["tags"] = tags.clone();
        }
        let got = created_then_read(&router, body).await;
        assert_eq!(
            got["tags"],
            Value::Object(expected),
            "{case}: expected the tag map the create stored, got {got:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 5.
// ---------------------------------------------------------------------------

/// Malware Protection reads through its OWN service-linked role, under its own
/// service principal — not the detector-level GuardDuty role.
#[tokio::test]
async fn malware_protection_reports_its_own_service_linked_role() {
    let creates: [Value; 3] = [
        json!({"enable": true}),
        json!({"enable": false}),
        json!({
            "enable": true,
            "dataSources": {"malwareProtection": {"scanEc2InstanceWithFindings": {"ebsVolumes": false}}},
        }),
    ];

    for body in creates {
        let case = format!("create {body}");
        let router = new_router();
        let got = created_then_read(&router, body.clone()).await;
        let detector_role = got["serviceRole"].as_str().unwrap_or_default().to_string();
        let malware_role = got["dataSources"]["malwareProtection"]["serviceRole"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let account = detector_role
            .split(':')
            .nth(4)
            .unwrap_or_default()
            .to_string();
        assert_eq!(
            malware_role,
            format!("arn:aws:iam::{account}:{MALWARE_PROTECTION_ROLE_SUFFIX}"),
            "{case}: expected the Malware Protection service-linked role, got {got:?}"
        );
        assert_ne!(
            malware_role, detector_role,
            "{case}: the Malware Protection role is a DIFFERENT role from the detector's, got {got:?}"
        );
    }
}
