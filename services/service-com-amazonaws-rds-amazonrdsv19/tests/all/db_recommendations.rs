// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for this service's recommendations: `DescribeDBRecommendations`, and
//! the configuration a DB instance or DB cluster is created and modified with that the
//! analysis behind it reads.
//!
//! Every call goes over the wire: the service is mounted on a `MultiServiceRouter` exactly
//! as the production router mounts it (through `db_instances::emulator`, the one mounting
//! helper this test target has), and each request is a real awsQuery form POST carrying
//! the SigV4 credential scope the router keys on. The assertions read the XML the emulator
//! answers with, so a handler that computed nothing could not pass them.
//!
//! The rule under test: this service raises one recommendation per issue its configuration
//! analysis finds in a resource it holds, reports the recommendations of the region the
//! request is signed for, narrowed by `Filters` and by the `LastUpdatedAfter` /
//! `LastUpdatedBefore` window and paged by `MaxRecords` / `Marker`, and reports a raised
//! recommendation as `resolved` once the configuration it found no longer holds. A
//! resource configured with none of the issues has no recommendations.
//!
//! The tables below span what that rule generalises over: every check of the published
//! analysis and the setting value that holds it, a configuration that holds none / one /
//! some / all of them, both resource families the analysis reads, each documented filter
//! name and the value sets two of them publish, the four shapes of the update window, both
//! published `MaxRecords` bounds plus one value past each, a page boundary and the marker
//! that resumes from it, the regional scope of the report, and the round trip of a setting
//! turned on, off and on again.

use service_com_amazonaws_rds_amazonrdsv19::state::{
    CONFIGURATION_CHECKS, MAX_RECORDS_RANGE, RECOMMENDATION_CATEGORIES, RECOMMENDATION_LOCALES,
    RECOMMENDATION_SEVERITIES, RECOMMENDATION_SOURCE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::db_instances::{
    call, call_in, emulator, expect_error, expect_ok, optional_text, subtrees, text, texts,
    wait_for_settle, REGION,
};

/// A second region, so the regional scope of the report is observable.
const OTHER_REGION: &str = "eu-west-1";

/// A call that carries no parameters beyond `Action` and `Version`.
const NO_PARAMS: [(&str, &str); 0] = [];

/// An instant before anything these tests create, for the lower end of an update window.
const LONG_AGO: &str = "2020-01-01T00:00:00Z";

/// An instant after everything these tests create, for the upper end of one.
const FAR_AHEAD: &str = "2999-01-01T00:00:00Z";

/// The `CreateDBInstance` settings that hold EVERY check of the analysis: no automated
/// backups, one Availability Zone, no encryption, no Enhanced Monitoring, no Performance
/// Insights, no deletion protection, no automatic minor version upgrades, and a public
/// address.
const ALL_ISSUES: [(&str, &str); 8] = [
    ("BackupRetentionPeriod", "0"),
    ("MultiAZ", "false"),
    ("StorageEncrypted", "false"),
    ("MonitoringInterval", "0"),
    ("EnablePerformanceInsights", "false"),
    ("DeletionProtection", "false"),
    ("AutoMinorVersionUpgrade", "false"),
    ("PubliclyAccessible", "true"),
];

/// The same settings, each at the value that holds NO check.
const NO_ISSUES: [(&str, &str); 8] = [
    ("BackupRetentionPeriod", "7"),
    ("MultiAZ", "true"),
    ("StorageEncrypted", "true"),
    ("MonitoringInterval", "60"),
    ("EnablePerformanceInsights", "true"),
    ("DeletionProtection", "true"),
    ("AutoMinorVersionUpgrade", "true"),
    ("PubliclyAccessible", "false"),
];

/// One `<DBRecommendation>` of an answer, as the members a test reads off it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Reported {
    recommendation_id: String,
    type_id: String,
    severity: String,
    category: String,
    status: String,
    resource_arn: String,
}

/// Every `<DBRecommendation>` the answer carries, in document order.
fn reported(xml: &str) -> Vec<Reported> {
    recommendations(xml)
        .iter()
        .map(|found| Reported {
            recommendation_id: text(found, "RecommendationId"),
            type_id: text(found, "TypeId"),
            severity: text(found, "Severity"),
            category: text(found, "Category"),
            status: text(found, "Status"),
            resource_arn: text(found, "ResourceArn"),
        })
        .collect()
}

/// The `<DBRecommendations>` list members of an answer, in document order.
///
/// `DBRecommendationList`'s member carries no `xmlName`, so each recommendation is a
/// `<member>` of `<DBRecommendations>` on the wire — and a recommendation carries lists of
/// its own, whose entries are `<member>` too. The shared helpers refuse an element that
/// nests inside itself, so the entries of the OUTER list are picked out by depth.
fn recommendations(xml: &str) -> Vec<String> {
    match subtrees(xml, "DBRecommendations").first() {
        None => Vec::new(),
        Some(list) => list_members(list),
    }
}

/// The `<member>` subtrees that are DIRECT entries of the list element `xml` is, in
/// document order. An entry's own nested lists are left inside it.
fn list_members(xml: &str) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    let mut depth = 0usize;
    let mut entry: Option<usize> = None;
    let mut cursor = 0usize;
    while let Some(offset) = xml[cursor..].find('<') {
        let open = cursor + offset;
        let Some(width) = xml[open..].find('>') else { break };
        let close = open + width;
        let tag = &xml[open + 1..close];
        cursor = close + 1;
        // A declaration, a comment or a self-closing element changes no depth, and an
        // empty `<member/>` carries nothing a test reads.
        if tag.starts_with('?') || tag.starts_with('!') || tag.ends_with('/') {
            continue;
        }
        // The element NAME, without the attributes a tag can carry: the emulator puts the
        // service's XML namespace on the first entry of a list.
        let name = tag
            .split_once(char::is_whitespace)
            .map_or(tag, |(name, _)| name);
        match name.strip_prefix('/') {
            None => {
                depth += 1;
                if depth == 2 && name == "member" && entry.is_none() {
                    entry = Some(open);
                }
            }
            Some(closed) => {
                if depth == 2 && closed == "member" {
                    if let Some(start) = entry.take() {
                        found.push(xml[start..cursor].to_string());
                    }
                }
                depth = depth.saturating_sub(1);
            }
        }
    }
    found
}

/// The type ids the answer reports for `resource_arn`, sorted.
fn type_ids_for(xml: &str, resource_arn: &str) -> Vec<String> {
    let mut found: Vec<String> = reported(xml)
        .into_iter()
        .filter(|record| record.resource_arn == resource_arn)
        .map(|record| record.type_id)
        .collect();
    found.sort();
    found
}

/// Every type id the published analysis can raise, sorted.
fn every_type_id() -> Vec<String> {
    let mut ids: Vec<String> = CONFIGURATION_CHECKS
        .iter()
        .map(|check| check.type_id.to_string())
        .collect();
    ids.sort();
    ids
}

/// `CreateDBInstance` with the three required members plus `extra`, waited out until the
/// instance has reached `available` — which is when the analysis has run over it.
async fn create_instance(router: &MultiServiceRouter, identifier: &str, extra: &[(&str, &str)]) {
    let mut params: Vec<(&str, String)> = vec![
        ("DBInstanceIdentifier", identifier.to_string()),
        ("Engine", "mysql".to_string()),
        ("DBInstanceClass", "db.t3.micro".to_string()),
    ];
    params.extend(
        extra
            .iter()
            .map(|(name, value)| (*name, (*value).to_string())),
    );
    expect_ok(
        &format!("create {identifier}"),
        call(router, "CreateDBInstance", &params).await,
    );
    wait_for_settle();
}

/// The `DbiResourceId` of a stored DB instance, as `DescribeDBInstances` reports it.
async fn dbi_resource_id(router: &MultiServiceRouter, identifier: &str) -> String {
    let body = expect_ok(
        &format!("describe {identifier}"),
        call(
            router,
            "DescribeDBInstances",
            &[("DBInstanceIdentifier", identifier)],
        )
        .await,
    );
    text(&body, "DbiResourceId")
}

/// The ARN of a resource of this service, as its own record computes it.
fn resource_arn(resource_type: &str, name: &str) -> String {
    format!("arn:aws:rds:{REGION}:000000000000:{resource_type}:{name}")
}

/// `DescribeDBRecommendations` for the whole region.
async fn describe(router: &MultiServiceRouter) -> String {
    expect_ok(
        "describe the recommendations",
        call(router, "DescribeDBRecommendations", &NO_PARAMS).await,
    )
}

/// One configuration the analysis is run over, and the checks it must hold.
struct ConfigurationCase {
    case: &'static str,
    identifier: &'static str,
    extra: &'static [(&'static str, &'static str)],
    /// The type ids the analysis must raise for it, sorted.
    expected: &'static [&'static str],
}

/// The configurations below span a resource that holds no check, one, two, several and
/// every one of them, and each row names the settings it differs from the others in.
const CONFIGURATION_CASES: &[ConfigurationCase] = &[
    ConfigurationCase {
        case: "every setting the analysis reads is already in place",
        identifier: "emu-rec-clean",
        extra: &NO_ISSUES,
        expected: &[],
    },
    ConfigurationCase {
        case: "every setting holds its check",
        identifier: "emu-rec-all",
        extra: &ALL_ISSUES,
        expected: &[
            "auto-minor-version-upgrade-turned-off",
            "automated-backups-turned-off",
            "deletion-protection-turned-off",
            "enhanced-monitoring-turned-off",
            "performance-insights-turned-off",
            "publicly-accessible",
            "single-az-deployment",
            "storage-not-encrypted",
        ],
    },
    ConfigurationCase {
        case: "only the automated backups are turned off",
        identifier: "emu-rec-backups",
        extra: &[
            ("BackupRetentionPeriod", "0"),
            ("MultiAZ", "true"),
            ("StorageEncrypted", "true"),
            ("MonitoringInterval", "60"),
            ("EnablePerformanceInsights", "true"),
            ("DeletionProtection", "true"),
            ("AutoMinorVersionUpgrade", "true"),
            ("PubliclyAccessible", "false"),
        ],
        expected: &["automated-backups-turned-off"],
    },
    ConfigurationCase {
        case: "the two monitoring settings are turned off",
        identifier: "emu-rec-monitoring",
        extra: &[
            ("BackupRetentionPeriod", "7"),
            ("MultiAZ", "true"),
            ("StorageEncrypted", "true"),
            ("MonitoringInterval", "0"),
            ("EnablePerformanceInsights", "false"),
            ("DeletionProtection", "true"),
            ("AutoMinorVersionUpgrade", "true"),
            ("PubliclyAccessible", "false"),
        ],
        expected: &[
            "enhanced-monitoring-turned-off",
            "performance-insights-turned-off",
        ],
    },
    ConfigurationCase {
        case: "a DB instance created with no setting named at all",
        identifier: "emu-rec-default",
        extra: &[],
        // The documented defaults of `CreateDBInstance`: one day of automated backups, a
        // single Availability Zone, no encryption, Enhanced Monitoring and Performance
        // Insights off, no deletion protection, automatic minor version upgrades on, and
        // — with no DB subnet group named for a non-Aurora engine — a public address.
        expected: &[
            "deletion-protection-turned-off",
            "enhanced-monitoring-turned-off",
            "performance-insights-turned-off",
            "publicly-accessible",
            "single-az-deployment",
            "storage-not-encrypted",
        ],
    },
];

/// The analysis raises exactly the checks the resource's stored configuration holds.
#[tokio::test]
async fn the_analysis_raises_the_checks_the_configuration_holds() {
    let router = emulator();
    for row in CONFIGURATION_CASES {
        create_instance(&router, row.identifier, row.extra).await;
    }
    let body = describe(&router).await;
    for row in CONFIGURATION_CASES {
        let arn = resource_arn("db", row.identifier);
        let found = type_ids_for(&body, &arn);
        let expected: Vec<String> = row.expected.iter().map(|id| id.to_string()).collect();
        assert_eq!(
            found, expected,
            "case {:?}: expected the analysis to raise {expected:?} for {arn}, got {found:?} \
             in {body}",
            row.case
        );
    }
    // The all-issues row holds every check the analysis publishes, so the catalog and the
    // report cannot drift apart.
    assert_eq!(
        type_ids_for(&body, &resource_arn("db", "emu-rec-all")),
        every_type_id(),
        "the resource that holds every check must be reported one recommendation per \
         published check, got {body}"
    );
}

/// Every reported recommendation carries the members it was computed from, classified with
/// values the model publishes.
#[tokio::test]
async fn a_recommendation_reports_what_it_was_computed_from() {
    let router = emulator();
    create_instance(&router, "emu-rec-wire", &ALL_ISSUES).await;
    let arn = resource_arn("db", "emu-rec-wire");
    let body = describe(&router).await;

    for record in reported(&body) {
        let case = record.type_id.clone();
        assert_eq!(
            record.resource_arn, arn,
            "case {case:?}: expected the recommendation to be about {arn}, got {:?}",
            record.resource_arn
        );
        assert_eq!(
            record.status, "active",
            "case {case:?}: a newly raised recommendation is active, got {:?}",
            record.status
        );
        assert!(
            RECOMMENDATION_SEVERITIES.contains(&record.severity.as_str()),
            "case {case:?}: severity {:?} must be one of {RECOMMENDATION_SEVERITIES:?}",
            record.severity
        );
        assert!(
            RECOMMENDATION_CATEGORIES.contains(&record.category.as_str()),
            "case {case:?}: category {:?} must be one of {RECOMMENDATION_CATEGORIES:?}",
            record.category
        );
        assert!(
            !record.recommendation_id.is_empty(),
            "case {case:?}: a recommendation carries the id it is named by"
        );
    }

    // The members of one check, read off its own subtree: the one raised because the
    // instance keeps no automated backups.
    let backups = the_recommendation(&body, "automated-backups-turned-off");
    for member in [
        "Detection",
        "Recommendation",
        "Description",
        "Reason",
        "Impact",
        "TypeDetection",
        "TypeRecommendation",
        "CreatedTime",
        "UpdatedTime",
    ] {
        assert!(
            !text(&backups, member).is_empty(),
            "the backups recommendation reports {member}, got {backups}"
        );
    }
    assert_eq!(
        text(&backups, "Source"),
        RECOMMENDATION_SOURCE,
        "the recommendation names the service that generated it, got {backups}"
    );
    // The detection and the reason state the setting the check read and the value it
    // found, which is the instance's stored `BackupRetentionPeriod` of 0.
    for member in ["Detection", "Reason"] {
        let value = text(&backups, member);
        assert!(
            value.contains("BackupRetentionPeriod") && value.contains('0') && value.contains(&arn),
            "the {member} names the setting read, the value found and the resource, got \
             {value:?}"
        );
    }
    // Its action is the one modify call that changes that setting, with the instance it
    // names and the member it sets.
    let action = the_action(&backups);
    assert_eq!(
        text(&action, "Operation"),
        "ModifyDBInstance",
        "the action on a DB instance names the operation that modifies it, got {action}"
    );
    assert_eq!(
        text(&action, "Status"),
        "ready",
        "an action of a recommendation nothing has applied is ready, got {action}"
    );
    assert_eq!(
        texts(&the_subtree(&action, "ApplyModes"), "member"),
        vec!["immediately".to_string()],
        "a setting one modify call changes is applied immediately, got {action}"
    );
    let parameters = keyed_values(&action, "Parameters");
    assert_eq!(
        parameters,
        vec![
            ("DBInstanceIdentifier".to_string(), "emu-rec-wire".to_string()),
            ("BackupRetentionPeriod".to_string(), "7".to_string()),
        ],
        "the action's parameters name the resource and the value to set, got {action}"
    );
    assert_eq!(
        keyed_values(&action, "ContextAttributes"),
        vec![("BackupRetentionPeriod".to_string(), "0".to_string())],
        "the action's context attribute carries the value observed, got {action}"
    );

    // A setting that cannot be changed in place is reported as a manual action with no
    // operation at all, rather than one no call could perform.
    let encryption = the_recommendation(&body, "storage-not-encrypted");
    let manual = the_action(&encryption);
    assert_eq!(
        texts(&the_subtree(&manual, "ApplyModes"), "member"),
        vec!["manual".to_string()],
        "a setting no modify call can change is a manual action, got {manual}"
    );
    assert_eq!(
        optional_text(&manual, "Operation"),
        None,
        "a manual action names no API operation, got {manual}"
    );
    assert!(
        keyed_values(&manual, "Parameters").is_empty(),
        "a manual action names no request parameters, got {manual}"
    );
}

/// The one recommendation the answer carries for `type_id`.
fn the_recommendation(xml: &str, type_id: &str) -> String {
    let found: Vec<String> = recommendations(xml)
        .into_iter()
        .filter(|found| text(found, "TypeId") == type_id)
        .collect();
    assert_eq!(
        found.len(),
        1,
        "expected exactly one {type_id} recommendation, got {} in {xml}",
        found.len()
    );
    found.into_iter().next().expect("checked just above")
}

/// The one `<tag>` subtree of `xml`.
fn the_subtree(xml: &str, tag: &str) -> String {
    let found = subtrees(xml, tag);
    assert_eq!(
        found.len(),
        1,
        "expected exactly one {tag}, got {} in {xml}",
        found.len()
    );
    found.into_iter().next().expect("checked just above")
}

/// The one `<RecommendedActions>` entry a recommendation carries.
fn the_action(recommendation: &str) -> String {
    let found = list_members(&the_subtree(recommendation, "RecommendedActions"));
    assert_eq!(
        found.len(),
        1,
        "expected exactly one recommended action, got {} in {recommendation}",
        found.len()
    );
    found.into_iter().next().expect("checked just above")
}

/// The `(Key, Value)` pairs of the `<tag>` list of `xml`, in document order, or none where
/// it carries no such list at all.
fn keyed_values(xml: &str, tag: &str) -> Vec<(String, String)> {
    match subtrees(xml, tag).first() {
        None => Vec::new(),
        Some(list) => list_members(list)
            .iter()
            .map(|found| (text(found, "Key"), text(found, "Value")))
            .collect(),
    }
}

/// A setting turned on resolves the recommendation it raised, and turning it off again
/// raises the same record anew — the status follows the configuration, and the id does not
/// change under a caller.
#[tokio::test]
async fn a_setting_turned_back_on_resolves_the_recommendation_it_raised() {
    let router = emulator();
    create_instance(&router, "emu-rec-cycle", &ALL_ISSUES).await;
    let raised = the_recommendation(&describe(&router).await, "automated-backups-turned-off");
    let identity = text(&raised, "RecommendationId");
    assert_eq!(
        text(&raised, "Status"),
        "active",
        "the check holds, so its recommendation is active, got {raised}"
    );

    // One modify call turns the setting on — exactly the action the recommendation names.
    // Enabling automated backups "can result in a brief I/O suspension", so that change is
    // "applied during the next maintenance window unless the ApplyImmediately parameter is
    // enabled for this request" — and this test is about the setting being on.
    expect_ok(
        "turn automated backups on",
        call(
            &router,
            "ModifyDBInstance",
            &[
                ("DBInstanceIdentifier", "emu-rec-cycle"),
                ("ApplyImmediately", "true"),
                ("BackupRetentionPeriod", "7"),
            ],
        )
        .await,
    );
    wait_for_settle();
    let body = describe(&router).await;
    let completed = the_recommendation(&body, "automated-backups-turned-off");
    assert_eq!(
        text(&completed, "Status"),
        "resolved",
        "the check no longer holds, so its recommendation is completed, got {completed}"
    );
    assert_eq!(
        text(&completed, "RecommendationId"),
        identity,
        "the record keeps the id it was raised under, got {completed}"
    );
    assert_eq!(
        text(&the_action(&completed), "Status"),
        "resolved",
        "the action of a completed recommendation is resolved, got {completed}"
    );
    assert_eq!(
        type_ids_for(&body, &resource_arn("db", "emu-rec-cycle")),
        every_type_id(),
        "resolving a recommendation leaves the record behind rather than dropping it, got \
         {body}"
    );

    // Turning it off again raises the same record: one check holds at most one
    // recommendation per resource.
    expect_ok(
        "turn automated backups off again",
        call(
            &router,
            "ModifyDBInstance",
            &[
                ("DBInstanceIdentifier", "emu-rec-cycle"),
                ("ApplyImmediately", "true"),
                ("BackupRetentionPeriod", "0"),
            ],
        )
        .await,
    );
    wait_for_settle();
    let reraised = the_recommendation(&describe(&router).await, "automated-backups-turned-off");
    assert_eq!(
        text(&reraised, "Status"),
        "active",
        "the check holds again, so its recommendation is active again, got {reraised}"
    );
    assert_eq!(
        text(&reraised, "RecommendationId"),
        identity,
        "the re-raised record is the one that was raised before, got {reraised}"
    );
}

/// A DB cluster's configuration is analysed by the same checks, and a setting the family
/// does not configure holds none of them.
#[tokio::test]
async fn a_db_cluster_is_analysed_by_the_settings_it_configures() {
    let router = emulator();
    expect_ok(
        "create the cluster",
        call(
            &router,
            "CreateDBCluster",
            &[
                ("DBClusterIdentifier", "emu-rec-cluster"),
                ("Engine", "aurora-mysql"),
                // The settings the checks read, each at the value that holds its check or
                // clears it. `BackupRetentionPeriod` is left at its default: a DB cluster
                // keeps at least one day of automated backups, so its check cannot hold.
                ("StorageEncrypted", "false"),
                ("MonitoringInterval", "0"),
                ("EnablePerformanceInsights", "false"),
                ("DeletionProtection", "true"),
                ("AutoMinorVersionUpgrade", "false"),
            ],
        )
        .await,
    );
    wait_for_settle();
    let body = describe(&router).await;
    let arn = resource_arn("cluster", "emu-rec-cluster");
    assert_eq!(
        type_ids_for(&body, &arn),
        vec![
            "auto-minor-version-upgrade-turned-off".to_string(),
            "enhanced-monitoring-turned-off".to_string(),
            "performance-insights-turned-off".to_string(),
            "storage-not-encrypted".to_string(),
        ],
        "a cluster is analysed by the settings it configures, and `MultiAZ` and \
         `PubliclyAccessible` are not among them when the create names neither, got {body}"
    );
    let upgrades = the_recommendation(&body, "auto-minor-version-upgrade-turned-off");
    let action = the_action(&upgrades);
    assert_eq!(
        text(&action, "Operation"),
        "ModifyDBCluster",
        "the action on a DB cluster names the operation that modifies it, got {action}"
    );
    assert_eq!(
        keyed_values(&action, "Parameters").first().cloned(),
        Some((
            "DBClusterIdentifier".to_string(),
            "emu-rec-cluster".to_string()
        )),
        "the action names the cluster it is about, got {action}"
    );

    // The DB instance filter never selects a cluster's recommendations, and the cluster
    // one does.
    let resource_id = text(
        &expect_ok(
            "describe the cluster",
            call(
                &router,
                "DescribeDBClusters",
                &[("DBClusterIdentifier", "emu-rec-cluster")],
            )
            .await,
        ),
        "DbClusterResourceId",
    );
    for (case, filter_name, expected) in [
        ("the cluster filter selects it", "cluster-resource-id", 4),
        ("the DB instance filter does not", "dbi-resource-id", 0),
    ] {
        let narrowed = expect_ok(
            case,
            call(
                &router,
                "DescribeDBRecommendations",
                &[
                    ("Filters.Filter.1.Name", filter_name.to_string()),
                    ("Filters.Filter.1.Values.Value.1", resource_id.clone()),
                ],
            )
            .await,
        );
        assert_eq!(
            type_ids_for(&narrowed, &arn).len(),
            expected,
            "case {case:?}: expected {expected} records, got {narrowed}"
        );
    }
}

/// One filter narrowing, and what it must select.
struct FilterCase {
    case: &'static str,
    /// The filter parameters, with `{dbi}` standing for the DB instance's resource id and
    /// `{id}` for the recommendation id of the backups check.
    params: &'static [(&'static str, &'static str)],
    /// The type ids the narrowed report must carry, sorted, or `None` for every one of
    /// them.
    expected: Option<&'static [&'static str]>,
}

/// The rows span each documented filter name, a narrowing that selects all / some / none
/// of the report, a filter value outside a published set (a match term, not a refusal),
/// two filters that intersect, and the four shapes of the update window.
const FILTER_CASES: &[FilterCase] = &[
    FilterCase {
        case: "no filter at all reports every recommendation",
        params: &[],
        expected: None,
    },
    FilterCase {
        case: "recommendation-id selects the one record it names",
        params: &[
            ("Filters.Filter.1.Name", "recommendation-id"),
            ("Filters.Filter.1.Values.Value.1", "{id}"),
        ],
        expected: Some(&["automated-backups-turned-off"]),
    },
    FilterCase {
        case: "status selects every active record",
        params: &[
            ("Filters.Filter.1.Name", "status"),
            ("Filters.Filter.1.Values.Value.1", "active"),
        ],
        expected: None,
    },
    FilterCase {
        case: "a status nothing is in selects nothing",
        params: &[
            ("Filters.Filter.1.Name", "status"),
            ("Filters.Filter.1.Values.Value.1", "dismissed"),
        ],
        expected: Some(&[]),
    },
    FilterCase {
        case: "a status no recommendation can be in selects nothing",
        params: &[
            ("Filters.Filter.1.Name", "status"),
            // "Filter values are case-sensitive", and a value outside the published set
            // is a match term that matches nothing rather than a refusal.
            ("Filters.Filter.1.Values.Value.1", "ACTIVE"),
        ],
        expected: Some(&[]),
    },
    FilterCase {
        case: "a severity no recommendation can have selects nothing",
        params: &[
            ("Filters.Filter.1.Name", "severity"),
            ("Filters.Filter.1.Values.Value.1", "critical"),
        ],
        expected: Some(&[]),
    },
    FilterCase {
        case: "severity selects the records of that urgency",
        params: &[
            ("Filters.Filter.1.Name", "severity"),
            ("Filters.Filter.1.Values.Value.1", "high"),
        ],
        expected: Some(&[
            "automated-backups-turned-off",
            "publicly-accessible",
            "storage-not-encrypted",
        ]),
    },
    FilterCase {
        case: "two values of one filter select the union",
        params: &[
            ("Filters.Filter.1.Name", "severity"),
            ("Filters.Filter.1.Values.Value.1", "low"),
            ("Filters.Filter.1.Values.Value.2", "informational"),
        ],
        expected: Some(&[
            "deletion-protection-turned-off",
            "enhanced-monitoring-turned-off",
            "performance-insights-turned-off",
        ]),
    },
    FilterCase {
        case: "type-id selects the records that check raised",
        params: &[
            ("Filters.Filter.1.Name", "type-id"),
            ("Filters.Filter.1.Values.Value.1", "single-az-deployment"),
        ],
        expected: Some(&["single-az-deployment"]),
    },
    FilterCase {
        case: "dbi-resource-id selects the records about that DB instance",
        params: &[
            ("Filters.Filter.1.Name", "dbi-resource-id"),
            ("Filters.Filter.1.Values.Value.1", "{dbi}"),
        ],
        expected: None,
    },
    FilterCase {
        case: "a dbi-resource-id no instance has selects nothing",
        params: &[
            ("Filters.Filter.1.Name", "dbi-resource-id"),
            ("Filters.Filter.1.Values.Value.1", "db-NOSUCHRESOURCEID"),
        ],
        expected: Some(&[]),
    },
    FilterCase {
        case: "cluster-resource-id never selects a DB instance's records",
        params: &[
            ("Filters.Filter.1.Name", "cluster-resource-id"),
            ("Filters.Filter.1.Values.Value.1", "{dbi}"),
        ],
        expected: Some(&[]),
    },
    FilterCase {
        case: "a parameter group ARN selects none of them",
        params: &[
            ("Filters.Filter.1.Name", "pg-arn"),
            (
                "Filters.Filter.1.Values.Value.1",
                "arn:aws:rds:us-west-2:000000000000:pg:default",
            ),
        ],
        expected: Some(&[]),
    },
    FilterCase {
        case: "a cluster parameter group ARN selects none of them",
        params: &[
            ("Filters.Filter.1.Name", "cluster-pg-arn"),
            (
                "Filters.Filter.1.Values.Value.1",
                "arn:aws:rds:us-west-2:000000000000:cluster-pg:default",
            ),
        ],
        expected: Some(&[]),
    },
    FilterCase {
        case: "two filters select the records both name",
        params: &[
            ("Filters.Filter.1.Name", "severity"),
            ("Filters.Filter.1.Values.Value.1", "high"),
            ("Filters.Filter.2.Name", "type-id"),
            ("Filters.Filter.2.Values.Value.1", "publicly-accessible"),
        ],
        expected: Some(&["publicly-accessible"]),
    },
    FilterCase {
        case: "two filters that name disjoint sets select nothing",
        params: &[
            ("Filters.Filter.1.Name", "severity"),
            ("Filters.Filter.1.Values.Value.1", "high"),
            ("Filters.Filter.2.Name", "type-id"),
            ("Filters.Filter.2.Values.Value.1", "single-az-deployment"),
        ],
        expected: Some(&[]),
    },
    FilterCase {
        case: "a window around now reports every recommendation",
        params: &[("LastUpdatedAfter", LONG_AGO), ("LastUpdatedBefore", FAR_AHEAD)],
        expected: None,
    },
    FilterCase {
        case: "a window that starts after now reports none",
        params: &[("LastUpdatedAfter", FAR_AHEAD)],
        expected: Some(&[]),
    },
    FilterCase {
        case: "a window that ends before now reports none",
        params: &[("LastUpdatedBefore", LONG_AGO)],
        expected: Some(&[]),
    },
    FilterCase {
        case: "a window open at its upper end reports every recommendation",
        params: &[("LastUpdatedAfter", LONG_AGO)],
        expected: None,
    },
];

/// Each documented filter narrows the report to the records it names.
#[tokio::test]
async fn filters_narrow_the_report_to_the_records_they_name() {
    let router = emulator();
    create_instance(&router, "emu-rec-filtered", &ALL_ISSUES).await;
    let dbi = dbi_resource_id(&router, "emu-rec-filtered").await;
    let backups_id = text(
        &the_recommendation(&describe(&router).await, "automated-backups-turned-off"),
        "RecommendationId",
    );

    for row in FILTER_CASES {
        let params: Vec<(&str, String)> = row
            .params
            .iter()
            .map(|(name, value)| {
                (
                    *name,
                    value.replace("{dbi}", &dbi).replace("{id}", &backups_id),
                )
            })
            .collect();
        let body = expect_ok(
            row.case,
            call(&router, "DescribeDBRecommendations", &params).await,
        );
        let found = type_ids_for(&body, &resource_arn("db", "emu-rec-filtered"));
        let expected: Vec<String> = match row.expected {
            None => every_type_id(),
            Some(ids) => ids.iter().map(|id| id.to_string()).collect(),
        };
        assert_eq!(
            found, expected,
            "case {:?}: expected {expected:?}, got {found:?} in {body}",
            row.case
        );
    }
}

/// One request this operation refuses, and the refusal it answers with.
struct RefusalCase {
    case: &'static str,
    params: &'static [(&'static str, &'static str)],
    status: u16,
    code: &'static str,
    /// The message the refusal must carry, with `{name}` standing for the filter name the
    /// request carried.
    message: Option<&'static str>,
}

/// The rows span a filter name the operation does not publish, a locale outside the
/// published languages, both `MaxRecords` bounds broken by one, and a marker no page
/// issued.
const REFUSAL_CASES: &[RefusalCase] = &[
    RefusalCase {
        case: "a filter name the operation does not publish",
        params: &[
            ("Filters.Filter.1.Name", "db-instance-id"),
            ("Filters.Filter.1.Values.Value.1", "emu-rec-refused"),
        ],
        status: 400,
        code: "InvalidParameterValue",
        // The sentence Amazon RDS answers an unrecognized filter name with: fixed text,
        // the rejected name, and nothing after it.
        message: Some("Unrecognized filter name: db-instance-id"),
    },
    RefusalCase {
        case: "a locale the recommendations are not published in",
        params: &[("Locale", "en-GB")],
        status: 400,
        code: "InvalidParameterValue",
        message: None,
    },
    RefusalCase {
        case: "a MaxRecords one below the published minimum",
        params: &[("MaxRecords", "19")],
        status: 400,
        code: "InvalidParameterValue",
        message: None,
    },
    RefusalCase {
        case: "a MaxRecords one above the published maximum",
        params: &[("MaxRecords", "101")],
        status: 400,
        code: "InvalidParameterValue",
        message: None,
    },
    RefusalCase {
        case: "a marker this service's codec could not have produced",
        params: &[("Marker", "not~a~token")],
        status: 400,
        code: "InvalidParameterValue",
        message: None,
    },
];

/// A request this operation cannot serve as asked is refused, never served narrowed.
#[tokio::test]
async fn an_unservable_request_is_refused() {
    let router = emulator();
    create_instance(&router, "emu-rec-refused", &ALL_ISSUES).await;
    for row in REFUSAL_CASES {
        let body = expect_error(
            row.case,
            call(&router, "DescribeDBRecommendations", row.params).await,
            row.status,
            row.code,
        );
        if let Some(expected) = row.message {
            let message = text(&body, "Message");
            assert_eq!(
                message, expected,
                "case {:?}: expected message {expected:?}, got {message:?}",
                row.case
            );
        }
    }
    // Every language the member publishes is served, and both published `MaxRecords`
    // bounds are accepted.
    for locale in RECOMMENDATION_LOCALES {
        expect_ok(
            locale,
            call(&router, "DescribeDBRecommendations", &[("Locale", locale)]).await,
        );
    }
    for bound in [*MAX_RECORDS_RANGE.start(), *MAX_RECORDS_RANGE.end()] {
        expect_ok(
            &format!("MaxRecords {bound}"),
            call(
                &router,
                "DescribeDBRecommendations",
                &[("MaxRecords", bound.to_string())],
            )
            .await,
        );
    }
}

/// The report pages at `MaxRecords` and resumes at the marker it handed out, without
/// repeating or dropping a record.
#[tokio::test]
async fn the_report_pages_by_max_records_and_marker() {
    let router = emulator();
    // Three resources holding every check: more records than one page of the smallest
    // published `MaxRecords` can carry.
    for identifier in ["emu-rec-page-a", "emu-rec-page-b", "emu-rec-page-c"] {
        create_instance(&router, identifier, &ALL_ISSUES).await;
    }
    let page_size = *MAX_RECORDS_RANGE.start() as usize;
    let whole = reported(&describe(&router).await);
    assert_eq!(
        whole.len(),
        3 * CONFIGURATION_CHECKS.len(),
        "three resources holding every check have one recommendation each per check, got \
         {whole:?}"
    );

    let first = expect_ok(
        "the first page",
        call(
            &router,
            "DescribeDBRecommendations",
            &[("MaxRecords", page_size.to_string())],
        )
        .await,
    );
    let first_page = reported(&first);
    assert_eq!(
        first_page.len(),
        page_size,
        "the first page carries MaxRecords records, got {first_page:?}"
    );
    let marker = text(&first, "Marker");
    assert!(
        !marker.is_empty(),
        "records remain, so the page hands out a marker, got {first}"
    );

    let second = expect_ok(
        "the page the marker resumes",
        call(
            &router,
            "DescribeDBRecommendations",
            &[
                ("MaxRecords", page_size.to_string()),
                ("Marker", marker.clone()),
            ],
        )
        .await,
    );
    let second_page = reported(&second);
    assert_eq!(
        second_page.len(),
        whole.len() - page_size,
        "the resumed page carries the rest, got {second_page:?}"
    );
    assert_eq!(
        optional_text(&second, "Marker"),
        None,
        "nothing remains, so the last page hands out no marker, got {second}"
    );
    let paged: Vec<Reported> = first_page.into_iter().chain(second_page).collect();
    assert_eq!(
        paged, whole,
        "the two pages together are the whole report, in the same order"
    );
}

/// The report is scoped to the region the request is signed for, and a deleted resource
/// takes its recommendations with it.
#[tokio::test]
async fn the_report_is_scoped_to_its_region_and_to_live_resources() {
    let router = emulator();
    create_instance(&router, "emu-rec-scoped", &ALL_ISSUES).await;
    let elsewhere = expect_ok(
        "describe in another region",
        call_in(
            &router,
            OTHER_REGION,
            "DescribeDBRecommendations",
            &NO_PARAMS,
        )
        .await,
    );
    assert!(
        reported(&elsewhere).is_empty(),
        "a region holding no resource has no recommendations, got {elsewhere}"
    );

    expect_ok(
        "delete the instance",
        call(
            &router,
            "DeleteDBInstance",
            &[
                ("DBInstanceIdentifier", "emu-rec-scoped"),
                ("SkipFinalSnapshot", "true"),
            ],
        )
        .await,
    );
    // The delete is accepted at once and the instance leaves the store a span later; the
    // recommendations about it go with it.
    wait_for_settle();
    wait_for_settle();
    let after = describe(&router).await;
    assert!(
        reported(&after).is_empty(),
        "a recommendation is about a resource, so it goes when the resource does, got {after}"
    );
}
