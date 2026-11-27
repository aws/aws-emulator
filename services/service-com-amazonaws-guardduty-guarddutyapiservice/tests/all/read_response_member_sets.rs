// SPDX-License-Identifier: CC0-1.0

//! The MEMBER SET a successful read answers with, end to end over the wire.
//!
//! Every request goes through [`smithy_server_runtime::MultiServiceRouter::dispatch`]
//! (the harness lives in the `detector_lifecycle` suite), so routing, restJson1
//! (de)serialization, modelled-constraint validation and the handlers are exercised
//! together.
//!
//! The ONE rule under test, in the five places this service applies it, each asserted
//! over a SET of inputs rather than one recorded call: a 200 answer carries the member
//! set GuardDuty carries — every member the service always emits is present, with the
//! value the stored state gives it or the EMPTY form (an empty map, the empty token, a
//! `null`) where the state holds nothing — and it invents nothing for state that does
//! not exist.
//!
//! 1. An entity-set read reports the account that owns the S3 bucket its `location`
//!    names: the `expectedBucketOwner` a create or update supplied, and the account the
//!    request was signed for when neither supplied one. (`GetIPSet` and
//!    `GetThreatIntelSet` document the opposite for the same member — "this field
//!    appears in the response only if it was provided during … creation or update" —
//!    and the entity-set operations document no such qualification, so they always
//!    report it.)
//! 2. `GetMalwareScanSettings.scanResourceCriteria` carries BOTH `include` and
//!    `exclude` on every read, each an empty map where the detector carries no
//!    criterion of that kind, so a read round-trips into
//!    `UpdateMalwareScanSettings`.
//! 3. `GetAdministratorAccount.administrator` and `GetMasterAccount.master` are always
//!    on the wire, carrying `null` where no administrator relationship exists, never an
//!    empty object a client would read as "there is an administrator".
//! 4. Every `GetDetector` `features` entry carries the member set
//!    `{name, status, updatedAt, additionalConfiguration}`, and the data-source view of
//!    a feature reports the enablement the detector was created with — RDS login events
//!    included, which a create that names no features enables ("with an exception to
//!    RUNTIME_MONITORING, all the optional features are enabled by default").
//! 5. `GetUsageStatistics` reports `nextToken` on every page, as the position of the
//!    next page while one remains and as the EMPTY string on the terminal page.
//!
//! The same rule over `GetCoverageStatistics` (one entry per group of the statistic's
//! dimension, counting zero where nothing fell) and over `ListCoverage` (no `resources`
//! member at all on a page that matches no row) is asserted in the `coverage_statistics`
//! and `coverage_listing` suites, beside the rest of those operations' behaviour.

use serde_json::{json, Value};

use super::detector_lifecycle::{call, create_detector, enabled_detector, new_router, Router};

/// The account this emulator serves — the one every request below is signed for, and so
/// the account that owns the S3 locations those requests name.
fn owning_account() -> String {
    engine::emulator_account_id().to_string()
}

/// The two entity-set families, each with the URI segment it is addressed under, the id
/// member its create answers with
/// (`smithy_query(operation="CreateThreatEntitySet")` /
/// `…="CreateTrustedEntitySet"`) and one `Format` member of its own enum, a different
/// one per family (`smithy_query(shape="ThreatEntitySetFormat")` /
/// `…shape="TrustedEntitySetFormat"`, which carry the same six values).
const ENTITY_SET_FAMILIES: [(&str, &str, &str, &str); 2] = [
    (
        "threat entity set",
        "threatentityset",
        "threatEntitySetId",
        "STIX",
    ),
    (
        "trusted entity set",
        "trustedentityset",
        "trustedEntitySetId",
        "TXT",
    ),
];

/// A 12-digit account id of the modelled `ExpectedBucketOwner` form (`@length` 12..12,
/// `@pattern ^[0-9]+$`) that is NOT the account this emulator serves, so a row that
/// supplies one is distinguishable from a row that supplies none.
const ANOTHER_ACCOUNT_ID: &str = "210987654321";

/// Another such id, for the update half of the round trip.
const A_THIRD_ACCOUNT_ID: &str = "399999999999";

// ---------------------------------------------------------------------------
// Rule 1: an entity-set read always reports the owner of its S3 location.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_entity_set_read_reports_the_account_that_owns_its_location() {
    // The rows differ along what the rule generalises over: WHICH family the set
    // belongs to, whether the create supplied an owner, whether the set was activated,
    // and whether the update that follows supplies an owner of its own.
    struct Row {
        case: String,
        /// The URI segment the family is addressed under.
        segment: &'static str,
        /// The id member the family's create answers with.
        id_member: &'static str,
        format: &'static str,
        owner: Option<&'static str>,
        activate: bool,
        update_owner: Option<&'static str>,
    }

    let mut rows: Vec<Row> = Vec::new();
    for (family, segment, id_member, format) in ENTITY_SET_FAMILIES {
        for (owner_case, owner) in [
            ("no owner on the create", None),
            ("an owner on the create", Some(ANOTHER_ACCOUNT_ID)),
        ] {
            for activate in [false, true] {
                for (update_case, update_owner) in [
                    ("an update naming no owner", None),
                    ("an update naming another owner", Some(A_THIRD_ACCOUNT_ID)),
                ] {
                    rows.push(Row {
                        case: format!(
                            "{family}: {owner_case}, activate {activate}, {update_case}"
                        ),
                        segment,
                        id_member,
                        format,
                        owner,
                        activate,
                        update_owner,
                    });
                }
            }
        }
    }

    for row in rows {
        let Row {
            case,
            segment,
            id_member,
            format,
            owner,
            activate,
            update_owner,
        } = row;

        let router = new_router();
        let detector_id = enabled_detector(&router).await;

        let mut create = json!({
            "name": "entity-set",
            "format": format,
            "location": "s3://my-bucket/lists/entities.txt",
            "activate": activate,
        });
        if let Some(owner) = owner {
            create["expectedBucketOwner"] = json!(owner);
        }
        let (status, created) = call(
            &router,
            "POST",
            &format!("/detector/{detector_id}/{segment}"),
            Some(create),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the create expected 200, got {status} {created:?}"
        );
        let set_id = created[id_member]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected a {id_member}, got {created:?}"))
            .to_string();

        // The owner a read reports: the one the create supplied, or the account that
        // owns the location when it supplied none.
        let expected_owner = owner.map(str::to_string).unwrap_or_else(owning_account);
        let read = read_entity_set(&router, &detector_id, segment, &set_id, &case).await;
        assert_eq!(
            read["expectedBucketOwner"],
            json!(expected_owner),
            "case {case:?}: expected expectedBucketOwner {expected_owner:?}, got {read:?}"
        );

        // And it round-trips through the update: a member the update carries replaces
        // the stored owner, one it omits leaves it where it was.
        let mut update = json!({"name": "entity-set-renamed"});
        if let Some(update_owner) = update_owner {
            update["expectedBucketOwner"] = json!(update_owner);
        }
        let (status, updated) = call(
            &router,
            "POST",
            &format!("/detector/{detector_id}/{segment}/{set_id}"),
            Some(update),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the update expected 200, got {status} {updated:?}"
        );
        let expected_owner = update_owner
            .map(str::to_string)
            .unwrap_or_else(|| expected_owner.clone());
        let read = read_entity_set(&router, &detector_id, segment, &set_id, &case).await;
        assert_eq!(
            read["expectedBucketOwner"],
            json!(expected_owner),
            "case {case:?}: expected expectedBucketOwner {expected_owner:?} after the update, got \
             {read:?}"
        );
    }
}

/// One entity-set read, failing the test unless it succeeded.
async fn read_entity_set(
    router: &Router,
    detector_id: &str,
    segment: &str,
    set_id: &str,
    case: &str,
) -> Value {
    let (status, body) = call(
        router,
        "GET",
        &format!("/detector/{detector_id}/{segment}/{set_id}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the read expected 200, got {status} {body:?}"
    );
    body
}

// ---------------------------------------------------------------------------
// Rule 2: both scan criteria are always reported.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_scan_settings_report_both_criteria_whatever_is_configured() {
    // The only `ScanCriterionKey` member (`smithy_query(shape="ScanCriterionKey")`).
    let key = "EC2_INSTANCE_TAG";
    let condition = json!({key: {"mapEquals": [{"key": "NoMalwareScan", "value": "true"}]}});

    // The rows differ along WHICH criterion kinds the detector carries: none at all,
    // each kind alone, both, and both cleared again.
    let rows: Vec<(&str, Option<Value>)> = vec![
        ("no update at all", None),
        ("an update carrying neither criterion", Some(json!({}))),
        (
            "an include criterion alone",
            Some(json!({"include": condition})),
        ),
        (
            "an exclude criterion alone",
            Some(json!({"exclude": condition})),
        ),
        (
            "both criteria",
            Some(json!({"include": condition, "exclude": condition})),
        ),
    ];

    for (case, criteria) in rows {
        let router = new_router();
        let detector_id = enabled_detector(&router).await;
        if let Some(criteria) = criteria.as_ref() {
            let (status, body) = call(
                &router,
                "POST",
                &format!("/detector/{detector_id}/malware-scan-settings"),
                Some(json!({"scanResourceCriteria": criteria})),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: the update expected 200, got {status} {body:?}"
            );
        }

        let (status, body) = call(
            &router,
            "GET",
            &format!("/detector/{detector_id}/malware-scan-settings"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: get-malware-scan-settings expected 200, got {status} {body:?}"
        );
        for kind in ["include", "exclude"] {
            let reported = &body["scanResourceCriteria"][kind];
            let expected = criteria
                .as_ref()
                .and_then(|criteria| criteria.get(kind))
                .cloned()
                .unwrap_or_else(|| json!({}));
            assert_eq!(
                reported, &expected,
                "case {case:?}: expected scanResourceCriteria.{kind} {expected:?}, got {body:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 3: the administrator relationship is reported as null, not as {}.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_account_with_no_administrator_reports_the_member_as_null() {
    // Both surfaces of the one stored relationship, each addressed under its own URI:
    // `GetAdministratorAccount` and the deprecated `GetMasterAccount`
    // ("this operation is deprecated, use GetAdministratorAccount instead").
    const SURFACES: [(&str, &str); 2] = [("administrator", "administrator"), ("master", "master")];

    // The rows differ along what the detector holds BESIDES an administrator: nothing,
    // member accounts of its own (which make it an administrator, not a member), and a
    // feature configuration. None of them is an administrator relationship.
    let rows: Vec<(&str, Value, bool)> = vec![
        ("a freshly created detector", json!({"enable": true}), false),
        (
            "a detector with a feature configuration",
            json!({"enable": true, "features": [{"name": "S3_DATA_EVENTS", "status": "DISABLED"}]}),
            false,
        ),
        (
            "a detector that administers member accounts of its own",
            json!({"enable": true}),
            true,
        ),
    ];

    for (case, create, with_members) in rows {
        let router = new_router();
        let (status, created) = create_detector(&router, create).await;
        assert_eq!(
            status, 200,
            "case {case:?}: create-detector expected 200, got {status} {created:?}"
        );
        let detector_id = created["detectorId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected a detectorId, got {created:?}"))
            .to_string();
        if with_members {
            let (status, body) = call(
                &router,
                "POST",
                &format!("/detector/{detector_id}/member"),
                Some(json!({"accountDetails": [
                    {"accountId": ANOTHER_ACCOUNT_ID, "email": "member@example.com"},
                ]})),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: create-members expected 200, got {status} {body:?}"
            );
        }

        for (member, segment) in SURFACES {
            let (status, body) = call(
                &router,
                "GET",
                &format!("/detector/{detector_id}/{segment}"),
                None,
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: the {member} read expected 200, got {status} {body:?}"
            );
            let object = body
                .as_object()
                .unwrap_or_else(|| panic!("case {case:?}: expected an object, got {body:?}"));
            assert!(
                object.contains_key(member),
                "case {case:?}: expected the {member} member on the wire, got {body:?}"
            );
            assert_eq!(
                body[member],
                Value::Null,
                "case {case:?}: no administrator relationship exists, so {member} is null, got \
                 {body:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 4: every reported feature carries the whole member set.
// ---------------------------------------------------------------------------

/// The member set every `DetectorFeatureConfigurationResult` entry carries
/// (`smithy_query(shape="DetectorFeatureConfigurationResult")`, less `managedBy`, which
/// names what manages a feature and is reported only where a GuardDuty policy does).
const FEATURE_MEMBERS: [&str; 4] = ["name", "status", "updatedAt", "additionalConfiguration"];

#[tokio::test]
async fn every_reported_detector_feature_carries_the_whole_member_set() {
    // The rows differ along the feature configuration the create carries: none at all,
    // one that disables an optional feature, one that enables the feature a bare create
    // leaves off, and one that configures a feature's additional configuration — plus,
    // for each, the status the RDS data-source view must report.
    let rows: Vec<(&str, Value, &str)> = vec![
        ("no features named at all", json!({"enable": true}), "ENABLED"),
        (
            "a create that disables RDS login events",
            json!({"enable": true, "features": [{"name": "RDS_LOGIN_EVENTS", "status": "DISABLED"}]}),
            "DISABLED",
        ),
        (
            "a create that enables RDS login events explicitly",
            json!({"enable": true, "features": [{"name": "RDS_LOGIN_EVENTS", "status": "ENABLED"}]}),
            "ENABLED",
        ),
        (
            "a create that configures an additional configuration",
            json!({"enable": true, "features": [{
                "name": "RUNTIME_MONITORING",
                "status": "ENABLED",
                "additionalConfiguration": [{"name": "EKS_ADDON_MANAGEMENT", "status": "ENABLED"}],
            }]}),
            "ENABLED",
        ),
    ];

    for (case, create, rds_status) in rows {
        let router = new_router();
        let (status, created) = create_detector(&router, create).await;
        assert_eq!(
            status, 200,
            "case {case:?}: create-detector expected 200, got {status} {created:?}"
        );
        let detector_id = created["detectorId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected a detectorId, got {created:?}"))
            .to_string();

        let (status, body) = call(&router, "GET", &format!("/detector/{detector_id}"), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: get-detector expected 200, got {status} {body:?}"
        );

        let features = body["features"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected a features list, got {body:?}"))
            .clone();
        assert!(
            !features.is_empty(),
            "case {case:?}: expected the detector's features to be reported, got {body:?}"
        );
        for entry in &features {
            let object = entry
                .as_object()
                .unwrap_or_else(|| panic!("case {case:?}: expected a feature object, got {entry:?}"));
            for member in FEATURE_MEMBERS {
                assert!(
                    object.contains_key(member),
                    "case {case:?}: expected every feature entry to carry {member}, got {entry:?}"
                );
            }
            assert!(
                entry["additionalConfiguration"].is_array(),
                "case {case:?}: expected additionalConfiguration to be a list, got {entry:?}"
            );
        }

        // The RDS login-events switch is reported under `Features`: the deprecated
        // data-source view declares no node for it
        // (`smithy_query(shape="DataSourceConfigurationsResult")`), so the view carries
        // nothing for RDS at all rather than a node the model does not model.
        assert!(
            body["dataSources"]["database"].is_null(),
            "case {case:?}: the data-source view models no RDS node, got {body:?}"
        );
        let rds_feature = features
            .iter()
            .find(|entry| entry["name"] == json!("RDS_LOGIN_EVENTS"))
            .unwrap_or_else(|| panic!("case {case:?}: expected an RDS_LOGIN_EVENTS entry"));
        assert_eq!(
            rds_feature["status"],
            json!(rds_status),
            "case {case:?}: expected the RDS switch {rds_status} the create asked for, got \
             {rds_feature:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 5: the usage statistics report a token on every page.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_usage_statistics_report_the_terminal_page_token_as_the_empty_string() {
    let router = new_router();
    let detector_id = enabled_detector(&router).await;
    // One sample finding per published finding type, so the per-feature statistic has
    // several rows to page over.
    let (status, body) = call(
        &router,
        "POST",
        &format!("/detector/{detector_id}/findings/create"),
        Some(json!({})),
    )
    .await;
    assert_eq!(
        status, 200,
        "create-sample-findings expected 200, got {status} {body:?}"
    );

    // The rows differ along the statistic asked for and the page size: one row at a
    // time, a few at a time, and more than the statistic holds (one page).
    for statistic in ["SUM_BY_FEATURES", "SUM_BY_ACCOUNT", "SUM_BY_DATA_SOURCE"] {
        for page_size in [1_i64, 2, 50] {
            let case = format!("{statistic} with maxResults {page_size}");
            let mut token: Option<String> = None;
            let mut pages = 0;
            loop {
                let mut request = json!({
                    "usageStatisticsType": statistic,
                    "usageCriteria": {},
                    "maxResults": page_size,
                });
                if let Some(token) = token.as_deref() {
                    request["nextToken"] = json!(token);
                }
                let (status, response) = call(
                    &router,
                    "POST",
                    &format!("/detector/{detector_id}/usage/statistics"),
                    Some(request),
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: get-usage-statistics expected 200, got {status} {response:?}"
                );
                // The member is on the wire on EVERY page, terminal or not.
                let reported = response["nextToken"].as_str().unwrap_or_else(|| {
                    panic!("case {case:?}: expected a nextToken on every page, got {response:?}")
                });
                pages += 1;
                assert!(
                    pages <= 50,
                    "case {case:?}: paging did not terminate after {pages} pages"
                );
                if reported.is_empty() {
                    // The terminal page: an empty token names no page, so a caller that
                    // hands it back is asking for the first page again rather than being
                    // refused.
                    let (status, replayed) = call(
                        &router,
                        "POST",
                        &format!("/detector/{detector_id}/usage/statistics"),
                        Some(json!({
                            "usageStatisticsType": statistic,
                            "usageCriteria": {},
                            "maxResults": page_size,
                            "nextToken": "",
                        })),
                    )
                    .await;
                    assert_eq!(
                        status, 200,
                        "case {case:?}: the empty token must read as the first page, got {status} \
                         {replayed:?}"
                    );
                    break;
                }
                token = Some(reported.to_string());
            }
            assert!(
                pages >= 1,
                "case {case:?}: expected at least one page, got {pages}"
            );
        }
    }
}
