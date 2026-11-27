// SPDX-License-Identifier: CC0-1.0

//! The GuardDuty member-detector configuration loop: `CreateMembers` →
//! `UpdateMemberDetectors` → `GetMemberDetectors`.
//!
//! Every request goes over the wire through `MultiServiceRouter::dispatch`, the same
//! path the production router uses, so these tests exercise routing, restJson1
//! (de)serialization, modelled-constraint validation and the handlers together.
//!
//! The rules under test, each asserted over a SET of inputs rather than one recorded
//! call:
//!
//! 1. A member account's configuration is ONE stored switch per feature: an update
//!    writes it and a read reports it back under both views — the `Features` list and
//!    the deprecated `DataSources` nodes — which therefore agree about its status. A
//!    data-source node carries that status alone, the model declaring no other member
//!    on it (`smithy_query(shape="DataSourceConfigurationsResult")`), so the instant
//!    the switch last changed is the `Features` entry's, and it moves only when the
//!    switch CHANGES.
//! 2. An update configures ONLY the switches it names, for ONLY the member accounts it
//!    names: every other feature, and every other member, keeps what it had.
//! 3. A member account's configuration starts where a detector created without a
//!    `Features` list starts: every reported feature enabled except the opt-in ones.
//!    The reported set is every `OrgFeature` member, once each, in canonical order.
//! 4. Configuring a member is not changing the RELATIONSHIP: `GetMembers` reports the
//!    same member record after an update as before it.
//! 5. An account whose member-account detector the administrator does not maintain is
//!    not an error for either operation: the call answers HTTP 200 and names the
//!    account in `unprocessedAccounts` — once per DISTINCT id, in request order — and
//!    persists nothing for it. The member accounts of this suite's fixture are the
//!    ones GuardDuty MONITORS under the administrator, which is the relationship a
//!    member account's detector exists in; the `member_detector_empty_report` suite
//!    owns the partition the read makes over every other relationship.
//! 6. A request the service cannot understand — a value that cannot be an account id,
//!    a `DetectorId` this account/Region does not own, a feature name outside the
//!    `OrgFeature` enum, a status outside `FeatureStatus`, or the two Runtime
//!    Monitoring features asked for together — is refused with HTTP 400 under the
//!    modelled `BadRequestException` shape, and configures nothing.

use std::sync::Arc;

use serde_json::{json, Value};

use crate::client_rejections::{account_ids_format, invalid_parameter, not_owned};
use crate::detector_lifecycle::{
    call, call_in, enabled_detector, error_code, error_shape, router_over, Router,
    ABSENT_DETECTOR_ID,
};
use crate::member_relationship_scope::monitor_member;
// The member-suite table, helpers and projections live in one place, so the member
// suites cannot drift on what a malformed or non-member account id is.
use crate::member_association::{
    administrator, create_members, disassociate_members, get_members, invite_members, member_of,
    unprocessed_ids, unprocessed_result, MALFORMED_ACCOUNT_IDS, MEMBERS, NON_MEMBER_ACCOUNT_IDS,
    NOT_AN_ASSOCIATED_MEMBER,
};

/// The modelled error SHAPE every GuardDuty client rejection is enveloped in, as
/// `x-amzn-errortype` carries it.
const ENVELOPE: &str = "BadRequestException";

/// The error class a HANDLER rejection carries in the body's `__type`.
const CLASS: &str = "InvalidInputException";

/// Every feature an administrator configures for a member account, in the order a
/// read reports them: the `OrgFeature` members in model order
/// (`smithy_query(shape="OrgFeature")`), which is what
/// `MemberFeaturesConfiguration.Name` and `MemberFeaturesConfigurationResult.Name`
/// target.
///
/// A NARROWER catalog than `DetectorFeature`: `AI_ANALYST` is a detector feature
/// and not an `OrgFeature`, which is why it is a refusal row below.
const MEMBER_FEATURES: [&str; 8] = [
    "S3_DATA_EVENTS",
    "EKS_AUDIT_LOGS",
    "EBS_MALWARE_PROTECTION",
    "RDS_LOGIN_EVENTS",
    "LAMBDA_NETWORK_LOGS",
    "EKS_RUNTIME_MONITORING",
    "RUNTIME_MONITORING",
    "AI_PROTECTION",
];

/// The optional features a member account starts with DISABLED; every other
/// reported feature starts ENABLED. Runtime Monitoring is the one exception
/// `CreateDetector` documents to "all the optional features are enabled by default".
const OPT_IN_FEATURES: [&str; 1] = ["RUNTIME_MONITORING"];

/// Every additional configuration a member-account request may name: the
/// `OrgFeatureAdditionalConfiguration` members
/// (`smithy_query(shape="OrgFeatureAdditionalConfiguration")`), which
/// `MemberAdditionalConfiguration.Name` targets.
const MEMBER_ADDITIONAL_CONFIGURATIONS: [&str; 4] = [
    "EKS_ADDON_MANAGEMENT",
    "ECS_FARGATE_AGENT_MANAGEMENT",
    "EC2_AGENT_MANAGEMENT",
    "RDS_DATA_RISK",
];

/// The `DataSources` nodes of a member read that a feature governs, paired with
/// that feature. The node paths are the `jsonName`s of
/// `DataSourceConfigurationsResult` and its nested result shapes
/// (`smithy_query(shape="DataSourceConfigurationsResult")`). A feature the result
/// shape has no node for — RDS login activity, for one — is reported under `Features`
/// alone, which the whole-set rule below covers.
const GOVERNED_NODES: [(&str, &[&str]); 3] = [
    ("S3_DATA_EVENTS", &["s3Logs"]),
    ("EKS_AUDIT_LOGS", &["kubernetes", "auditLogs"]),
    (
        "EBS_MALWARE_PROTECTION",
        &[
            "malwareProtection",
            "scanEc2InstanceWithFindings",
            "ebsVolumes",
        ],
    ),
];

/// The data sources GuardDuty always consumes: no `OrgFeature` member and no
/// `DataSourceConfigurations` member names them, so nothing can switch them off.
const FOUNDATIONAL_NODES: [&str; 3] = ["cloudTrail", "dnsLogs", "flowLogs"];

/// `UpdateMemberDetectors` over the wire, with whatever configuration members
/// `extra` names beside the required `accountIds`.
pub(crate) async fn update_member_detectors(
    router: &Router,
    detector_id: &str,
    account_ids: &[&str],
    extra: Value,
) -> (u16, Value) {
    let mut body = json!({"accountIds": account_ids});
    for (key, value) in extra.as_object().into_iter().flatten() {
        body[key.as_str()] = value.clone();
    }
    call(
        router,
        "POST",
        &format!("/detector/{detector_id}/member/detector/update"),
        Some(body),
    )
    .await
}

/// `GetMemberDetectors` over the wire.
pub(crate) async fn get_member_detectors(
    router: &Router,
    detector_id: &str,
    account_ids: &[&str],
) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/detector/{detector_id}/member/detector/get"),
        Some(json!({"accountIds": account_ids})),
    )
    .await
}

/// Create an enabled detector, record [`MEMBERS`] against it, INVITE them and put
/// each one into the MONITORED relationship — the relationship GuardDuty maintains a
/// member account's own detector in, and so the one whose configuration the
/// administrator writes and reads here.
///
/// The monitored relationship is reached when the member ACCOUNT answers the
/// administrator's invitation, which is that account's own call to make and not one
/// this emulator can be asked to make on its behalf; the `member_relationship_scope`
/// suite owns the one state seam every suite drives it through. The fixture keeps the
/// engine handle that seam needs, so it hands back the router with the detector.
async fn detector_with_monitored_members() -> (Router, String) {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_over(Arc::clone(&engine));
    let detector_id = enabled_detector(&router).await;
    let (status, body) = create_members(&router, &detector_id, &MEMBERS).await;
    assert_eq!(
        status, 200,
        "create-members: expected 200, got {status} {body:?}"
    );
    assert!(
        unprocessed_ids(&body).is_empty(),
        "create-members: expected every account processed, got {body:?}"
    );
    let ids: Vec<&str> = MEMBERS.iter().map(|(id, _)| *id).collect();
    let (status, body) = invite_members(&router, &detector_id, &ids).await;
    assert_eq!(
        status, 200,
        "invite-members: expected 200, got {status} {body:?}"
    );
    assert!(
        unprocessed_ids(&body).is_empty(),
        "invite-members: expected every account processed, got {body:?}"
    );
    for (account_id, _) in MEMBERS {
        monitor_member(&engine, &detector_id, account_id);
    }
    (router, detector_id)
}

/// Every recorded account id, in the order [`MEMBERS`] lists them.
fn all_ids() -> Vec<&'static str> {
    MEMBERS.iter().map(|(id, _)| *id).collect()
}

/// The configuration `GetMemberDetectors` reports for `account_id`.
async fn configuration(router: &Router, detector_id: &str, account_id: &str) -> Value {
    let (status, body) = get_member_detectors(router, detector_id, &[account_id]).await;
    assert_eq!(
        status, 200,
        "case {account_id}: get-member-detectors expected 200, got {status} {body:?}"
    );
    member_of(&body, account_id).unwrap_or_else(|| {
        panic!("case {account_id}: expected a member configuration, got {body:?}")
    })
}

/// One reported `features` entry of a member configuration.
fn feature<'a>(configuration: &'a Value, name: &str) -> &'a Value {
    configuration["features"]
        .as_array()
        .and_then(|features| {
            features
                .iter()
                .find(|entry| entry["name"].as_str() == Some(name))
        })
        .unwrap_or_else(|| panic!("feature {name} is not reported at all: got {configuration:?}"))
}

/// Walk a `dataSources` node path of a member configuration.
fn node<'a>(configuration: &'a Value, path: &[&str]) -> &'a Value {
    let mut cursor = &configuration["dataSources"];
    for segment in path {
        cursor = &cursor[*segment];
    }
    cursor
}

/// The status a feature entry reports.
fn feature_status(configuration: &Value, name: &str, case: &str) -> String {
    let entry = feature(configuration, name);
    entry["status"]
        .as_str()
        .unwrap_or_else(|| panic!("{case}: expected a FeatureStatus for {name}, got {entry:?}"))
        .to_string()
}

/// The epoch instant a `features` entry carries
/// (`MemberFeaturesConfigurationResult.UpdatedAt` is a `Timestamp`, so the wire
/// carries epoch seconds).
fn feature_instant(configuration: &Value, name: &str, case: &str) -> f64 {
    let entry = feature(configuration, name);
    let instant = entry["updatedAt"]
        .as_f64()
        .unwrap_or_else(|| panic!("{case}: expected an updatedAt timestamp for {name}, got {entry:?}"));
    assert!(
        instant > 0.0,
        "{case}: expected a positive epoch instant for {name}, got {instant}"
    );
    instant
}

/// Assert both views of one member switch report `expected`, and return the instant
/// the FEATURE view reports for it.
fn assert_views_agree(
    configuration: &Value,
    feature_name: &str,
    path: &[&str],
    expected: &str,
    case: &str,
) -> f64 {
    let node = node(configuration, path);
    assert_eq!(
        node["status"],
        json!(expected),
        "{case}: dataSources.{} expected {expected}, got {configuration:?}",
        path.join(".")
    );
    assert_eq!(
        feature_status(configuration, feature_name, case),
        expected,
        "{case}: feature {feature_name} expected {expected}, got {configuration:?}"
    );
    // The node reports the status and nothing else: `UpdatedAt` is a member of the
    // `Features` view, which is where the instant the switch last changed is read.
    assert!(
        node["updatedAt"].is_null(),
        "{case}: dataSources.{} models no updatedAt, got {node:?}",
        path.join(".")
    );
    feature_instant(configuration, feature_name, case)
}

/// The status a member account's configuration starts a feature in.
fn default_status(feature_name: &str) -> &'static str {
    if OPT_IN_FEATURES.contains(&feature_name) {
        "DISABLED"
    } else {
        "ENABLED"
    }
}

/// The other value of the `FeatureStatus` enum.
fn opposite(status: &str) -> &'static str {
    if status == "ENABLED" {
        "DISABLED"
    } else {
        "ENABLED"
    }
}

// ---------------------------------------------------------------------------
// Rules 1 and 2: write a switch, read it back through both views.
// ---------------------------------------------------------------------------

/// Every configurable member feature round-trips, through its whole value space,
/// and is read back under both views.
#[tokio::test]
async fn every_member_feature_round_trips_through_both_views() {
    for feature_name in MEMBER_FEATURES {
        let (router, detector_id) = detector_with_monitored_members().await;
        let account_id = MEMBERS[0].0;
        let start = default_status(feature_name);
        let case = format!("member {account_id} feature {feature_name}");

        // Runtime Monitoring already covers Amazon EKS, so the two cannot both end up
        // enabled: the EKS Runtime Monitoring switch a recorded member starts ENABLED
        // goes off before this row walks the Runtime Monitoring switch through its
        // own value space.
        if feature_name == "RUNTIME_MONITORING" {
            let (status, body) = update_member_detectors(
                &router,
                &detector_id,
                &[account_id],
                json!({"features": [{"name": "EKS_RUNTIME_MONITORING", "status": "DISABLED"}]}),
            )
            .await;
            assert_eq!(
                status, 200,
                "{case}: switching EKS Runtime Monitoring off first expected 200, got {status} \
                 {body:?}"
            );
        }

        // What every OTHER feature holds once the row's own preparation is done: the
        // default, except for the EKS Runtime Monitoring switch a Runtime Monitoring
        // row had to turn off above.
        let prepared = |other: &str| -> &'static str {
            if feature_name == "RUNTIME_MONITORING" && other == "EKS_RUNTIME_MONITORING" {
                "DISABLED"
            } else {
                default_status(other)
            }
        };

        let got = configuration(&router, &detector_id, account_id).await;
        let mut last = feature_instant(&got, feature_name, &case);
        assert_eq!(
            feature_status(&got, feature_name, &case),
            start,
            "{case}: expected the default status {start}, got {got:?}"
        );

        // Walk the whole value space of the switch, ending where it started.
        for target in [opposite(start), start, opposite(start)] {
            let (status, body) = update_member_detectors(
                &router,
                &detector_id,
                &[account_id],
                json!({"features": [{"name": feature_name, "status": target}]}),
            )
            .await;
            assert_eq!(
                status, 200,
                "{case} -> {target}: expected 200, got {status} {body:?}"
            );
            assert!(
                unprocessed_ids(&body).is_empty(),
                "{case} -> {target}: expected the member processed, got {body:?}"
            );

            let got = configuration(&router, &detector_id, account_id).await;
            assert_eq!(
                feature_status(&got, feature_name, &case),
                target,
                "{case}: expected the update to be read back as {target}, got {got:?}"
            );
            let moved = feature_instant(&got, feature_name, &case);
            assert!(
                moved > last,
                "{case} -> {target}: a status change must move updatedAt: was {last}, got {moved}"
            );
            last = moved;

            // Re-applying the SAME status changes nothing, so the instant records
            // when the switch last CHANGED, not when it was last written.
            let (status, _) = update_member_detectors(
                &router,
                &detector_id,
                &[account_id],
                json!({"features": [{"name": feature_name, "status": target}]}),
            )
            .await;
            assert_eq!(status, 200, "{case} -> {target} again: expected 200");
            let again = configuration(&router, &detector_id, account_id).await;
            let unmoved = feature_instant(&again, feature_name, &case);
            assert!(
                (unmoved - last).abs() < 0.001,
                "{case} -> {target} again: re-applying the same status must not move updatedAt: \
                 was {last}, got {unmoved}"
            );

            // Rule 2: every OTHER feature keeps what it had.
            for other in MEMBER_FEATURES {
                if other == feature_name {
                    continue;
                }
                assert_eq!(
                    feature_status(&again, other, &case),
                    prepared(other),
                    "{case} -> {target}: expected feature {other} to be untouched, got {again:?}"
                );
            }
        }
    }
}

/// A switch a request names through the `Features` list is read back through the
/// deprecated `DataSources` nodes as well, and the two agree on the instant.
#[tokio::test]
async fn a_member_data_source_node_reports_the_feature_that_governs_it() {
    for (feature_name, path) in GOVERNED_NODES {
        let (router, detector_id) = detector_with_monitored_members().await;
        let account_id = MEMBERS[1].0;
        let case = format!("member {account_id} dataSources.{}", path.join("."));
        let start = default_status(feature_name);

        let got = configuration(&router, &detector_id, account_id).await;
        let before = assert_views_agree(&got, feature_name, path, start, &case);

        let target = opposite(start);
        let (status, body) = update_member_detectors(
            &router,
            &detector_id,
            &[account_id],
            json!({"features": [{"name": feature_name, "status": target}]}),
        )
        .await;
        assert_eq!(status, 200, "{case}: expected 200, got {status} {body:?}");

        let got = configuration(&router, &detector_id, account_id).await;
        let after = assert_views_agree(&got, feature_name, path, target, &case);
        assert!(
            after > before,
            "{case}: a status change must move the instant both views report: was {before}, got {after}"
        );
    }
}

/// Writing a member switch through the deprecated `DataSources` INPUT writes the
/// same stored switch, read back through both views.
#[tokio::test]
async fn the_deprecated_data_sources_input_writes_the_same_member_switch() {
    // (the DataSources body, the feature it writes, the node it is read back at)
    let cases: [(Value, &str, &[&str]); 3] = [
        (
            json!({"s3Logs": {"enable": false}}),
            "S3_DATA_EVENTS",
            &["s3Logs"],
        ),
        (
            json!({"kubernetes": {"auditLogs": {"enable": false}}}),
            "EKS_AUDIT_LOGS",
            &["kubernetes", "auditLogs"],
        ),
        (
            json!({"malwareProtection": {"scanEc2InstanceWithFindings": {"ebsVolumes": false}}}),
            "EBS_MALWARE_PROTECTION",
            &[
                "malwareProtection",
                "scanEc2InstanceWithFindings",
                "ebsVolumes",
            ],
        ),
    ];

    for (data_sources, feature_name, path) in cases {
        let (router, detector_id) = detector_with_monitored_members().await;
        let account_id = MEMBERS[2].0;
        let case = format!("dataSources {data_sources} -> {feature_name}");

        let (status, body) = update_member_detectors(
            &router,
            &detector_id,
            &[account_id],
            json!({"dataSources": data_sources}),
        )
        .await;
        assert_eq!(status, 200, "{case}: expected 200, got {status} {body:?}");

        let got = configuration(&router, &detector_id, account_id).await;
        assert_views_agree(&got, feature_name, path, "DISABLED", &case);
    }
}

/// An update configures only the MEMBERS it names.
#[tokio::test]
async fn an_update_configures_only_the_member_accounts_it_names() {
    let (router, detector_id) = detector_with_monitored_members().await;
    // A strict, NON-CONTIGUOUS subset, so the call is observable both ways and the
    // configured members are not merely a prefix of the stored order.
    let configured = [MEMBERS[0].0, MEMBERS[2].0];
    let feature_name = "S3_DATA_EVENTS";

    let (status, body) = update_member_detectors(
        &router,
        &detector_id,
        &configured,
        json!({"features": [{"name": feature_name, "status": "DISABLED"}]}),
    )
    .await;
    assert_eq!(status, 200, "mixed update: expected 200, got {status} {body:?}");
    assert!(
        unprocessed_ids(&body).is_empty(),
        "mixed update: expected every member processed, got {body:?}"
    );

    for (account_id, _) in MEMBERS {
        let expected = if configured.contains(&account_id) {
            "DISABLED"
        } else {
            default_status(feature_name)
        };
        let case = format!("member {account_id}");
        let got = configuration(&router, &detector_id, account_id).await;
        assert_eq!(
            feature_status(&got, feature_name, &case),
            expected,
            "{case}: expected {feature_name} {expected}, got {got:?}"
        );
    }

    // One read of several accounts reports one entry per account, in request order.
    let ids = all_ids();
    let (status, body) = get_member_detectors(&router, &detector_id, &ids).await;
    assert_eq!(status, 200, "batch read: expected 200, got {status} {body:?}");
    let reported: Vec<String> = body["members"]
        .as_array()
        .unwrap_or_else(|| panic!("batch read: expected a members list, got {body:?}"))
        .iter()
        .map(|entry| entry["accountId"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        reported,
        ids.iter().map(|id| id.to_string()).collect::<Vec<_>>(),
        "batch read: expected one entry per account in request order, got {body:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule 3: where a member account's configuration starts.
// ---------------------------------------------------------------------------

/// A recorded member reports every `OrgFeature` member once, in canonical order,
/// enabled except the opt-in ones; the foundational data sources have no switch and
/// report `ENABLED`.
#[tokio::test]
async fn a_recorded_member_starts_in_the_default_configuration() {
    let (router, detector_id) = detector_with_monitored_members().await;

    for (account_id, _) in MEMBERS {
        let case = format!("member {account_id}");
        let got = configuration(&router, &detector_id, account_id).await;
        let reported: Vec<String> = got["features"]
            .as_array()
            .unwrap_or_else(|| panic!("{case}: expected a features list, got {got:?}"))
            .iter()
            .map(|entry| entry["name"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            reported,
            MEMBER_FEATURES.map(String::from).to_vec(),
            "{case}: features must be the whole OrgFeature set, once each, in canonical order, \
             got {got:?}"
        );
        for feature_name in MEMBER_FEATURES {
            let expected = default_status(feature_name);
            assert_eq!(
                feature_status(&got, feature_name, &case),
                expected,
                "{case}: expected {feature_name} to start {expected}, got {got:?}"
            );
            feature_instant(&got, feature_name, &case);
        }
        for json_name in FOUNDATIONAL_NODES {
            assert_eq!(
                node(&got, &[json_name])["status"],
                json!("ENABLED"),
                "{case}: dataSources.{json_name} has no switch, so it reports ENABLED, got {got:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 4: configuring a member is not changing the relationship.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn configuring_a_member_leaves_its_member_record_alone() {
    let (router, detector_id) = detector_with_monitored_members().await;
    let ids = all_ids();

    let (status, before) = get_members(&router, &detector_id, &ids).await;
    assert_eq!(
        status, 200,
        "get-members before: expected 200, got {status} {before:?}"
    );

    // Rows differing along the COUNT of accounts one call names and along the view
    // the configuration is written through.
    let batches: [(&[&str], Value); 3] = [
        (
            &ids[..1],
            // Runtime Monitoring already covers Amazon EKS, so the EKS Runtime
            // Monitoring switch a recorded member starts ENABLED goes off in the
            // same call that enables Runtime Monitoring.
            json!({"features": [{"name": "RUNTIME_MONITORING", "status": "ENABLED"},
                                {"name": "EKS_RUNTIME_MONITORING", "status": "DISABLED"}]}),
        ),
        (
            &ids[..2],
            json!({"dataSources": {"s3Logs": {"enable": false}}}),
        ),
        (
            &ids[..],
            json!({"features": [{"name": "AI_PROTECTION", "status": "DISABLED"}]}),
        ),
    ];

    for (batch, body) in batches {
        let (status, answered) = update_member_detectors(&router, &detector_id, batch, body.clone()).await;
        assert_eq!(
            status, 200,
            "case {batch:?} {body}: expected 200, got {status} {answered:?}"
        );
        let (_, after) = get_members(&router, &detector_id, &ids).await;
        assert_eq!(
            after, before,
            "case {batch:?} {body}: configuring a member must not move its member record, \
             got {after:?} after {before:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 5: an account that is not an associated member is REPORTED, not refused.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_account_that_is_not_an_associated_member_is_reported_by_both_operations() {
    let (router, detector_id) = detector_with_monitored_members().await;
    let reason = NOT_AN_ASSOCIATED_MEMBER;

    // The administrator's own account is not one of its members either, so it is a
    // row of this table beside the ids nothing recorded, and so are a member a
    // disassociation already moved and one recorded and not invited yet.
    let administrator = administrator();
    let (disassociated, _) = MEMBERS[1];
    let (status, body) = disassociate_members(&router, &detector_id, &[disassociated]).await;
    assert_eq!(status, 200, "disassociate: expected 200, got {status}");
    assert!(
        unprocessed_ids(&body).is_empty(),
        "disassociate: expected an invited member processed, got {body:?}"
    );
    let recorded_only = "707070707070";
    let (status, body) =
        create_members(&router, &detector_id, &[(recorded_only, "recorded@example.com")]).await;
    assert_eq!(status, 200, "create-members: expected 200, got {status} {body:?}");

    let mut rows: Vec<(&str, &str)> = NON_MEMBER_ACCOUNT_IDS.to_vec();
    rows.push((administrator.as_str(), "the administrator's own account"));
    rows.push((disassociated, "a member a disassociation already moved"));
    rows.push((recorded_only, "a member recorded and not invited yet"));

    for (account_id, why) in rows {
        for operation in ["update", "get"] {
            let (status, body) = if operation == "update" {
                update_member_detectors(
                    &router,
                    &detector_id,
                    &[account_id],
                    json!({"features": [{"name": "S3_DATA_EVENTS", "status": "DISABLED"}]}),
                )
                .await
            } else {
                get_member_detectors(&router, &detector_id, &[account_id]).await
            };
            assert_eq!(
                status, 200,
                "case {operation} {account_id} ({why}): expected 200, got {status} {body:?}"
            );
            assert_eq!(
                unprocessed_ids(&body),
                vec![account_id.to_string()],
                "case {operation} {account_id} ({why}): expected exactly that account unprocessed, \
                 got {body:?}"
            );
            assert_eq!(
                unprocessed_result(&body, account_id).as_deref(),
                Some(reason),
                "case {operation} {account_id} ({why}): expected the not-a-member reason, got {body:?}"
            );
            assert!(
                member_of(&body, account_id).is_none(),
                "case {operation} {account_id} ({why}): expected no configuration reported, got {body:?}"
            );
        }
    }
}

#[tokio::test]
async fn only_the_accounts_that_are_not_members_are_reported_once_in_request_order() {
    let (router, detector_id) = detector_with_monitored_members().await;
    let member_id = MEMBERS[2].0;
    let absent_one = NON_MEMBER_ACCOUNT_IDS[0].0;
    let absent_two = NON_MEMBER_ACCOUNT_IDS[1].0;

    // A mixed request: the member is configured, the rest reported, in request order,
    // and a repeated id is reported once because `AccountIds` names a SET.
    let requested = [absent_two, member_id, absent_one, absent_two, member_id];
    let (status, body) = update_member_detectors(
        &router,
        &detector_id,
        &requested,
        json!({"features": [{"name": "LAMBDA_NETWORK_LOGS", "status": "DISABLED"}]}),
    )
    .await;
    assert_eq!(status, 200, "mixed update: expected 200, got {status} {body:?}");
    assert_eq!(
        unprocessed_ids(&body),
        vec![absent_two.to_string(), absent_one.to_string()],
        "mixed update: expected each absent account once, in request order, got {body:?}"
    );

    let (status, read) = get_member_detectors(&router, &detector_id, &requested).await;
    assert_eq!(status, 200, "mixed read: expected 200, got {status} {read:?}");
    assert_eq!(
        unprocessed_ids(&read),
        vec![absent_two.to_string(), absent_one.to_string()],
        "mixed read: expected each absent account once, in request order, got {read:?}"
    );
    let reported: Vec<String> = read["members"]
        .as_array()
        .unwrap_or_else(|| panic!("mixed read: expected a members list, got {read:?}"))
        .iter()
        .map(|entry| entry["accountId"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        reported,
        vec![member_id.to_string()],
        "mixed read: expected the member reported once, got {read:?}"
    );
    let case = format!("member {member_id}");
    let got = configuration(&router, &detector_id, member_id).await;
    assert_eq!(
        feature_status(&got, "LAMBDA_NETWORK_LOGS", &case),
        "DISABLED",
        "{case}: expected the member in a mixed request to be configured, got {got:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule 1, continued: a feature's additional configuration is also ONE stored
// entry per name.
// ---------------------------------------------------------------------------

/// A feature's `AdditionalConfiguration` round-trips, and is stored ONE entry per
/// name: each entry configures one `OrgFeatureAdditionalConfiguration` member, so a
/// repeated name is one configuration with the last entry naming it winning, and
/// the stored set is never longer than that enum's own member list.
#[tokio::test]
async fn a_features_additional_configuration_is_one_stored_entry_per_name() {
    // Rows differing along the COUNT of entries sent, the number of DISTINCT names
    // among them and the status each asks for. A row is `(the request's
    // additionalConfiguration, the stored set it must read back as, why)`.
    type Row = (Value, &'static [(&'static str, &'static str)], &'static str);
    let rows: [Row; 4] = [
        (
            json!([{"name": "EKS_ADDON_MANAGEMENT", "status": "ENABLED"}]),
            &[("EKS_ADDON_MANAGEMENT", "ENABLED")],
            "one entry",
        ),
        (
            json!([{"name": "EKS_ADDON_MANAGEMENT", "status": "DISABLED"},
                   {"name": "EC2_AGENT_MANAGEMENT", "status": "ENABLED"}]),
            &[
                ("EC2_AGENT_MANAGEMENT", "ENABLED"),
                ("EKS_ADDON_MANAGEMENT", "DISABLED"),
            ],
            "two distinct names",
        ),
        (
            json!([{"name": "EKS_ADDON_MANAGEMENT", "status": "DISABLED"},
                   {"name": "EKS_ADDON_MANAGEMENT", "status": "ENABLED"}]),
            &[("EKS_ADDON_MANAGEMENT", "ENABLED")],
            "one name twice: the last entry naming it wins",
        ),
        (
            json!([{"name": "EKS_ADDON_MANAGEMENT", "status": "ENABLED"},
                   {"name": "ECS_FARGATE_AGENT_MANAGEMENT", "status": "ENABLED"},
                   {"name": "EC2_AGENT_MANAGEMENT", "status": "DISABLED"},
                   {"name": "RDS_DATA_RISK", "status": "ENABLED"}]),
            &[
                ("EC2_AGENT_MANAGEMENT", "DISABLED"),
                ("ECS_FARGATE_AGENT_MANAGEMENT", "ENABLED"),
                ("EKS_ADDON_MANAGEMENT", "ENABLED"),
                ("RDS_DATA_RISK", "ENABLED"),
            ],
            "every name the enum carries",
        ),
    ];

    for (additional, expected, why) in rows {
        let (router, detector_id) = detector_with_monitored_members().await;
        let account_id = MEMBERS[0].0;
        let case = format!("member {account_id} additionalConfiguration {additional} ({why})");

        let (status, body) = update_member_detectors(
            &router,
            &detector_id,
            &[account_id],
            json!({"features": [{"name": "EKS_RUNTIME_MONITORING", "status": "DISABLED"},
                                {"name": "RUNTIME_MONITORING",
                                 "status": "ENABLED",
                                 "additionalConfiguration": additional.clone()}]}),
        )
        .await;
        assert_eq!(status, 200, "{case}: expected 200, got {status} {body:?}");

        let got = configuration(&router, &detector_id, account_id).await;
        let entry = feature(&got, "RUNTIME_MONITORING");
        let stored: Vec<(String, String)> = entry["additionalConfiguration"]
            .as_array()
            .unwrap_or_else(|| panic!("{case}: expected an additionalConfiguration list, got {entry:?}"))
            .iter()
            .map(|cfg| {
                (
                    cfg["name"].as_str().unwrap_or_default().to_string(),
                    cfg["status"].as_str().unwrap_or_default().to_string(),
                )
            })
            .collect();
        let want: Vec<(String, String)> = expected
            .iter()
            .map(|(name, status)| (name.to_string(), status.to_string()))
            .collect();
        assert_eq!(
            stored, want,
            "{case}: expected one stored entry per distinct name, got {entry:?}"
        );
        assert!(
            stored.len() <= MEMBER_ADDITIONAL_CONFIGURATIONS.len(),
            "{case}: the stored set must never be longer than the enum's own member list, got {stored:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 6: requests the service does not understand are refused 400.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_value_that_cannot_be_an_account_id_is_refused() {
    let (router, detector_id) = detector_with_monitored_members().await;
    // Both operations carry the ids in their `accountIds` list, which is the
    // parameter GuardDuty's format rejection names.
    let expected = account_ids_format("accountIds");
    let member_id = MEMBERS[0].0;
    let case_feature = "S3_DATA_EVENTS";

    for (account_id, why) in MALFORMED_ACCOUNT_IDS {
        // The whole call is refused, so a well-formed id sent alongside is not
        // configured either.
        for operation in ["update", "get"] {
            let (status, body) = if operation == "update" {
                update_member_detectors(
                    &router,
                    &detector_id,
                    &[member_id, account_id],
                    json!({"features": [{"name": case_feature, "status": "DISABLED"}]}),
                )
                .await
            } else {
                get_member_detectors(&router, &detector_id, &[member_id, account_id]).await
            };
            assert_eq!(
                status, 400,
                "case {operation} {account_id} ({why}): expected 400, got {status} {body:?}"
            );
            assert_eq!(
                error_code(&body),
                CLASS,
                "case {operation} {account_id} ({why}): expected the {CLASS} class, got {body:?}"
            );
            assert_eq!(
                error_shape(&body),
                ENVELOPE,
                "case {operation} {account_id} ({why}): expected the {ENVELOPE} envelope, got {body:?}"
            );
            assert_eq!(
                body["message"].as_str(),
                Some(expected.as_str()),
                "case {operation} {account_id} ({why}): expected the parameter message, got {body:?}"
            );
        }

        let case = format!("member {member_id} after refusing {account_id}");
        let got = configuration(&router, &detector_id, member_id).await;
        assert_eq!(
            feature_status(&got, case_feature, &case),
            default_status(case_feature),
            "{case} ({why}): expected a refused call to configure nothing, got {got:?}"
        );
    }
}

#[tokio::test]
async fn a_detector_this_account_does_not_own_is_refused() {
    let (router, detector_id) = detector_with_monitored_members().await;
    let member_id = MEMBERS[0].0;

    // A detector another Region's endpoint serves is not owned here: a detector is a
    // regional resource.
    let (status, elsewhere) = call_in(
        &router,
        "eu-west-1",
        "POST",
        "/detector",
        Some(json!({"enable": true})),
    )
    .await;
    assert_eq!(
        status, 200,
        "create-detector in eu-west-1: expected 200, got {status} {elsewhere:?}"
    );
    let other_region_id = elsewhere["detectorId"]
        .as_str()
        .unwrap_or_else(|| {
            panic!("create-detector in eu-west-1: expected a detectorId, got {elsewhere:?}")
        })
        .to_string();

    let rejections: [(&str, &str); 3] = [
        (
            "deadbeefdeadbeefdeadbeefdeadbeeg",
            "32 characters, but 'g' is not hexadecimal",
        ),
        (ABSENT_DETECTOR_ID, "a well-formed id nothing minted"),
        (
            other_region_id.as_str(),
            "a detector another Region's endpoint serves",
        ),
    ];

    for (candidate, why) in rejections {
        let expected = if candidate.len() == 32 && candidate.bytes().all(|b| b.is_ascii_hexdigit()) {
            not_owned("detectorId")
        } else {
            invalid_parameter("detectorId")
        };
        for operation in ["update", "get"] {
            let (status, body) = if operation == "update" {
                update_member_detectors(
                    &router,
                    candidate,
                    &[member_id],
                    json!({"features": [{"name": "S3_DATA_EVENTS", "status": "DISABLED"}]}),
                )
                .await
            } else {
                get_member_detectors(&router, candidate, &[member_id]).await
            };
            assert_eq!(
                status, 400,
                "case {operation} {candidate} ({why}): expected 400, got {status} {body:?}"
            );
            assert_eq!(
                error_code(&body),
                CLASS,
                "case {operation} {candidate} ({why}): expected the {CLASS} class, got {body:?}"
            );
            assert_eq!(
                error_shape(&body),
                ENVELOPE,
                "case {operation} {candidate} ({why}): expected the {ENVELOPE} envelope, got {body:?}"
            );
            assert_eq!(
                body["message"].as_str(),
                Some(expected.as_str()),
                "case {operation} {candidate} ({why}): expected the classified message, got {body:?}"
            );
        }
    }

    // The detector named by a refused call is untouched.
    let case = format!("member {member_id}");
    let got = configuration(&router, &detector_id, member_id).await;
    assert_eq!(
        feature_status(&got, "S3_DATA_EVENTS", &case),
        "ENABLED",
        "{case}: expected a refused call to configure nothing, got {got:?}"
    );
}

/// A feature list this service cannot accept is refused, and configures nothing:
/// a name outside the `OrgFeature` enum (including a `DetectorFeature` that is not
/// one), a status outside `FeatureStatus`, the two Runtime Monitoring features
/// asked for together — which `UpdateMemberDetectors` documents as an error because
/// Runtime Monitoring already includes threat detection for Amazon EKS resources —
/// and a list carrying more entries than there are values for it to configure.
#[tokio::test]
async fn a_feature_list_this_service_cannot_accept_is_refused() {
    let (router, detector_id) = detector_with_monitored_members().await;
    let member_id = MEMBERS[0].0;

    // One entry past the number of distinct values each list can configure, so the
    // row fails ONLY on the count: every entry below is otherwise acceptable.
    let too_many_features: Value = Value::Array(
        (0..=MEMBER_FEATURES.len())
            .map(|_| json!({"name": "S3_DATA_EVENTS", "status": "DISABLED"}))
            .collect(),
    );
    let too_many_additional: Value = json!([{
        "name": "RUNTIME_MONITORING",
        "status": "ENABLED",
        "additionalConfiguration": Value::Array(
            (0..=MEMBER_ADDITIONAL_CONFIGURATIONS.len())
                .map(|_| json!({"name": "EKS_ADDON_MANAGEMENT", "status": "ENABLED"}))
                .collect(),
        ),
    }]);

    let rows: Vec<(Value, &str)> = vec![
        (
            json!([{"name": "AI_ANALYST", "status": "ENABLED"}]),
            "a DetectorFeature that is not an OrgFeature member",
        ),
        (
            json!([{"name": "CLOUD_TRAIL", "status": "ENABLED"}]),
            "a foundational data source, which no feature member names",
        ),
        (
            json!([{"name": "NOT_A_FEATURE", "status": "ENABLED"}]),
            "a name no enum carries",
        ),
        (
            json!([{"name": "s3_data_events", "status": "ENABLED"}]),
            "an OrgFeature member in the wrong case",
        ),
        (
            json!([{"name": "S3_DATA_EVENTS", "status": "enabled"}]),
            "a FeatureStatus member in the wrong case",
        ),
        (
            json!([{"name": "S3_DATA_EVENTS", "status": "PENDING"}]),
            "a status no enum carries",
        ),
        (
            json!([{"name": "RUNTIME_MONITORING", "status": "ENABLED"},
                   {"name": "EKS_RUNTIME_MONITORING", "status": "ENABLED"}]),
            "both Runtime Monitoring features at once",
        ),
        (
            too_many_features,
            "more feature entries than there are features to configure",
        ),
        (
            too_many_additional,
            "more additionalConfiguration entries than there are configurations",
        ),
    ];

    for (features, why) in rows {
        let (status, body) = update_member_detectors(
            &router,
            &detector_id,
            &[member_id],
            json!({"features": features.clone()}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {features} ({why}): expected 400, got {status} {body:?}"
        );
        assert_eq!(
            error_shape(&body),
            ENVELOPE,
            "case {features} ({why}): expected the {ENVELOPE} envelope, got {body:?}"
        );
        assert!(
            !body["message"]
                .as_str()
                .unwrap_or_default()
                .is_empty(),
            "case {features} ({why}): expected a message saying what was wrong, got {body:?}"
        );

        // Nothing the refused request named was configured.
        let case = format!("member {member_id} after refusing {features}");
        let got = configuration(&router, &detector_id, member_id).await;
        for feature_name in MEMBER_FEATURES {
            assert_eq!(
                feature_status(&got, feature_name, &case),
                default_status(feature_name),
                "{case} ({why}): expected a refused call to configure nothing, got {got:?}"
            );
        }
    }
}
