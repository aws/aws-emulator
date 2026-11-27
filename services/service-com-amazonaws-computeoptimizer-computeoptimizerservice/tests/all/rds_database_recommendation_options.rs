// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the recommendation OPTIONS `GetRDSDatabaseRecommendations` reports.
//!
//! Every assertion drives the service the way a client does — an awsJson1_0 `POST /` through
//! the same `MultiServiceRouter` the emulator binary mounts — so a response that is not
//! computed from persisted state fails here.
//!
//! Two rules are pinned, each over a TABLE of inputs rather than one recorded call:
//!
//! * the utilization projected onto a recommended DB instance class, and the performance risk
//!   that projection implies: the same work runs on whatever capacity the class provides, so
//!   a metric measuring a SHARE of the instance's capacity moves with the ratio of the two
//!   classes' capacities (capped at the whole of it) while a metric measuring the workload's
//!   own volume does not move at all, and the option's risk follows its most loaded share;
//! * the savings-estimation-mode preference: the savings members net of discounts are
//!   reported exactly where the preference records in force for the database activate them.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_computeoptimizer_computeoptimizerservice::ops::types::{
    DBStorageConfiguration, RDSDBUtilizationMetric,
};
use service_com_amazonaws_computeoptimizer_computeoptimizerservice::state::{
    service_state, AnalysedResource, RdsDatabaseDetail, ANALYSIS_DELAY,
    SOURCE_TYPE_RDS_DB_INSTANCE,
};

use crate::harness::{
    call, emulator_with_engine, get_preferences, management_account_emulator, opt_in,
    put_preference,
};

/// Opt the account in and wait the analysis span out: a catalogued recommendation is
/// reported only once the analysis the enrollment started is due ([`ANALYSIS_DELAY`]); the
/// extra margin covers scheduler jitter.
async fn opted_in_and_analysed(router: &smithy_server_runtime::MultiServiceRouter) {
    opt_in(router).await;
    tokio::time::sleep(ANALYSIS_DELAY + Duration::from_millis(100)).await;
}

/// The region the harness signs for, and so the regional endpoint every call below
/// addresses: the ARN reported for an analysed database names it.
const REGION: &str = "us-east-1";

/// The ONE account this service answers for, taken from the engine rather than written out
/// here.
fn own_account() -> &'static str {
    ::engine::emulator_account_id()
}

/// A DB instance ARN: "arn:aws:rds:{region}:{accountId}:db:{resourceName}"
/// (`GetRDSDatabaseRecommendations$resourceArns`).
fn db_arn(name: &str) -> String {
    format!("arn:aws:rds:{REGION}:{}:db:{name}", own_account())
}

/// One observed utilization metric, in the only statistic this API reports
/// (`RDSDBUtilizationMetric$statistic`: "return utilization metrics using only the Maximum
/// statistic").
fn metric(name: &str, value: f64) -> RDSDBUtilizationMetric {
    RDSDBUtilizationMetric {
        name: Some(name.to_string()),
        statistic: Some("Maximum".to_string()),
        value,
    }
}

/// The observed metrics as the response reports them back: a database the analysis observed
/// nothing on reports none at all.
fn observed_as_reported(observed: &[RDSDBUtilizationMetric]) -> Value {
    if observed.is_empty() {
        return Value::Null;
    }
    Value::Array(
        observed
            .iter()
            .map(|metric| {
                json!({
                    "name": metric.name,
                    "statistic": metric.statistic,
                    "value": metric.value,
                })
            })
            .collect(),
    )
}

/// One analysed database as the findings catalog records it.
fn analysed(
    arn_resource: &str,
    finding: &str,
    current_monthly_cost: f64,
    recommended_monthly_cost: f64,
    rds_database: RdsDatabaseDetail,
) -> AnalysedResource {
    AnalysedResource {
        account_id: own_account().to_string(),
        resource_type: SOURCE_TYPE_RDS_DB_INSTANCE.to_string(),
        arn_service: "rds".to_string(),
        arn_resource: arn_resource.to_string(),
        finding: finding.to_string(),
        finding_reason_codes: Vec::new(),
        current_performance_risk: "Low".to_string(),
        current_monthly_cost,
        recommended_monthly_cost,
        inferred_workload_types: Vec::new(),
        rds_database: Some(rds_database),
    }
}

/// Record `resources` into the findings catalog this reader reports from, the way Compute
/// Optimizer's own analysis of an observed workload would — no operation of this API creates
/// a recommendation.
fn record(engine: &Arc<engine::InMemoryEngine>, resources: &[AnalysedResource]) {
    let state = service_state(&**engine).expect("the engine bears this service's state");
    let mut guard = state.lock();
    for resource in resources {
        guard.analysed_resources.push(resource.clone());
    }
}

/// The one recommendation reported for `resource_arn`.
async fn reported_for(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    resource_arn: &str,
) -> Value {
    let (status, body) = call(
        router,
        "GetRDSDatabaseRecommendations",
        json!({"resourceArns": [resource_arn]}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 for {resource_arn}, got {status}: {body}"
    );
    let reported = body["rdsDBRecommendations"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a list, got {body}"));
    assert_eq!(
        reported.len(),
        1,
        "case {case:?}: expected exactly the recommendation of {resource_arn}, got {body}"
    );
    reported[0].clone()
}

/// One row of the projection table: a database whose analysis would move it from one DB
/// instance class to another, and the projection and risk that move implies.
struct Projection {
    /// What this row is about, named in every assertion it drives.
    case: &'static str,
    /// The database's identity within its ARN.
    arn_resource: &'static str,
    /// Its `Finding`: the recommendation of an over-provisioned database is smaller, that of
    /// an under-provisioned one larger.
    finding: &'static str,
    /// What it costs today and what the recommended configuration costs, which is what the
    /// saving is computed from — and, being different, what gives the analysis an option to
    /// report at all.
    costs: (f64, f64),
    /// The DB instance class it runs today and the one the analysis would move it to.
    classes: (&'static str, Option<&'static str>),
    /// The utilization the analysis observed on the current class.
    observed: Vec<RDSDBUtilizationMetric>,
    /// `projectedUtilizationMetrics` as the option must report it — `Value::Null` where
    /// there is no ratio to project along, so none is reported rather than invented.
    projected: Value,
    /// `performanceRisk` as the option must report it, on the model's 0..4 scale.
    risk: f64,
}

/// The utilization an option is reported with is the observed workload projected onto the
/// recommended class, and its `performanceRisk` is how close that projection comes to the
/// whole of the class's capacity.
///
/// The classes below are read off the instance size catalog this crate holds (`nano` through
/// `<N>xlarge`, a doubling sequence), so the rows span a recommendation that halves the
/// capacity, one that doubles it, a step on the sub-`large` part of the sequence, a
/// projection that would run past the whole of the capacity, a class whose size the catalog
/// records no capacity for, and a database nothing was observed on.
#[tokio::test]
async fn the_recommended_class_is_reported_with_the_utilization_projected_onto_it() {
    let rows = vec![
        Projection {
            case: "half the capacity carries the same work at twice the share",
            arn_resource: "db:orders-writer",
            finding: "Overprovisioned",
            costs: (400.0, 200.0),
            classes: ("db.r6g.4xlarge", Some("db.r6g.2xlarge")),
            observed: vec![
                metric("CPU", 12.0),
                metric("Memory", 30.0),
                // A connection count is the workload's own volume, not a share of the
                // instance: resizing the database does not change it.
                metric("DatabaseConnections", 40.0),
            ],
            projected: json!([
                {"name": "CPU", "statistic": "Maximum", "value": 24.0},
                {"name": "Memory", "statistic": "Maximum", "value": 60.0},
                {"name": "DatabaseConnections", "statistic": "Maximum", "value": 40.0},
            ]),
            // The most loaded share takes 60% of the recommended class.
            risk: 1.4,
        },
        Projection {
            case: "twice the capacity carries it at half the share",
            arn_resource: "db:reports-primary",
            finding: "Underprovisioned",
            costs: (100.0, 180.0),
            classes: ("db.m5.large", Some("db.m5.xlarge")),
            observed: vec![metric("CPU", 88.0), metric("Memory", 95.0)],
            projected: json!([
                {"name": "CPU", "statistic": "Maximum", "value": 44.0},
                {"name": "Memory", "statistic": "Maximum", "value": 47.5},
            ]),
            risk: 0.9,
        },
        Projection {
            case: "a step on the sub-large part of the size sequence",
            arn_resource: "db:sandbox",
            finding: "Overprovisioned",
            costs: (30.0, 15.0),
            classes: ("db.t3.small", Some("db.t3.micro")),
            observed: vec![metric("CPU", 20.0)],
            projected: json!([{"name": "CPU", "statistic": "Maximum", "value": 40.0}]),
            risk: 0.6,
        },
        Projection {
            case: "a projection past the whole of the capacity is capped at it",
            arn_resource: "db:ledger",
            finding: "Overprovisioned",
            costs: (300.0, 90.0),
            classes: ("db.r6g.2xlarge", Some("db.r6g.large")),
            observed: vec![metric("CPU", 30.0), metric("Memory", 10.0)],
            projected: json!([
                // 30% of four times the capacity is 120% of a quarter of it, and a
                // percentage of a capacity cannot pass the whole of it.
                {"name": "CPU", "statistic": "Maximum", "value": 100.0},
                {"name": "Memory", "statistic": "Maximum", "value": 40.0},
            ]),
            risk: 3.0,
        },
        Projection {
            case: "a class whose size the catalog records no capacity for leaves the metric \
                   where it was observed",
            arn_resource: "db:aurora-serverless",
            finding: "Underprovisioned",
            costs: (50.0, 70.0),
            classes: ("db.serverless", Some("db.m5.large")),
            observed: vec![metric("CPU", 50.0)],
            // No ratio to project by, so the projection is the observation rather than a
            // guess — and the risk follows that projection like any other.
            projected: json!([{"name": "CPU", "statistic": "Maximum", "value": 50.0}]),
            risk: 1.0,
        },
        Projection {
            case: "a database nothing was observed on is projected onto nothing",
            arn_resource: "db:billing-primary",
            finding: "Overprovisioned",
            costs: (80.0, 40.0),
            classes: ("db.m6g.xlarge", Some("db.m6g.large")),
            observed: Vec::new(),
            projected: Value::Null,
            risk: 0.0,
        },
    ];

    let (router, engine) = emulator_with_engine();
    opted_in_and_analysed(&router).await;
    let catalog: Vec<AnalysedResource> = rows
        .iter()
        .map(|row| {
            analysed(
                row.arn_resource,
                row.finding,
                row.costs.0,
                row.costs.1,
                RdsDatabaseDetail {
                    current_db_instance_class: Some(row.classes.0.to_string()),
                    recommended_db_instance_class: row.classes.1.map(ToString::to_string),
                    utilization_metrics: row.observed.clone(),
                    ..RdsDatabaseDetail::default()
                },
            )
        })
        .collect();
    record(&engine, &catalog);

    for row in &rows {
        let name = row
            .arn_resource
            .strip_prefix("db:")
            .expect("the row names a DB instance");
        let reported = reported_for(&router, row.case, &db_arn(name)).await;
        let option = &reported["instanceRecommendationOptions"][0];
        assert_eq!(
            option["dbInstanceClass"],
            json!(row.classes.1),
            "case {:?}: the option names the recommended class, got {reported}",
            row.case
        );
        assert_eq!(
            option["projectedUtilizationMetrics"], row.projected,
            "case {:?}: expected projectedUtilizationMetrics {}, got {reported}",
            row.case, row.projected
        );
        assert_eq!(
            option["performanceRisk"],
            json!(row.risk),
            "case {:?}: expected performanceRisk {}, got {reported}",
            row.case,
            row.risk
        );
        // `PerformanceRisk` is modelled with range 0..4, so no projection may report a risk
        // outside it.
        let risk = option["performanceRisk"]
            .as_f64()
            .unwrap_or_else(|| panic!("case {:?}: a numeric performanceRisk: {reported}", row.case));
        assert!(
            (0.0..=4.0).contains(&risk),
            "case {:?}: performanceRisk {risk} is outside the modelled 0..4 range, got {reported}",
            row.case
        );
        // The observed utilization is reported as observed: projecting it onto the option
        // must not restate the observation.
        assert_eq!(
            reported["utilizationMetrics"],
            observed_as_reported(&row.observed),
            "case {:?}: the observed metrics are reported unchanged beside the projection, \
             got {reported}",
            row.case
        );
    }
}

/// One row of the preference table: what an account stored for the savings-estimation mode,
/// and whether the savings members net of discounts are then in force for its databases.
struct Preference {
    /// What this row is about.
    case: &'static str,
    /// The `PutRecommendationPreferences` request to store first, or `None` for an account
    /// that stored nothing at all.
    request: Option<Value>,
    /// Whether `savingsOpportunityAfterDiscounts` is then reported on each option.
    after_discounts: bool,
}

/// The savings-estimation-mode preference in force for a database is applied to its
/// recommendation: the savings members net of discounts are reported on the instance option
/// and on the storage option exactly when a record that applies to the database activates the
/// preference, and on neither when none does.
///
/// The rows span the scopes a database preference can be stored at (the account default, the
/// account, the organization), the value that DEACTIVATES the preference, a scope naming
/// another account, and a record of another resource type — the last two applying to nothing
/// this request reports.
#[tokio::test]
async fn the_savings_estimation_mode_preference_is_applied_to_rds_recommendations() {
    let rows = vec![
        Preference {
            case: "an account that stored no preference",
            request: None,
            after_discounts: false,
        },
        Preference {
            case: "BeforeDiscounts deactivates the preference",
            request: Some(json!({
                "resourceType": "RdsDBInstance",
                "savingsEstimationMode": "BeforeDiscounts",
            })),
            after_discounts: false,
        },
        Preference {
            case: "AfterDiscounts as the account default",
            request: Some(json!({
                "resourceType": "RdsDBInstance",
                "savingsEstimationMode": "AfterDiscounts",
            })),
            after_discounts: true,
        },
        Preference {
            case: "AfterDiscounts at the account scope",
            request: Some(json!({
                "resourceType": "RdsDBInstance",
                "scope": {"name": "AccountId", "value": own_account()},
                "savingsEstimationMode": "AfterDiscounts",
            })),
            after_discounts: true,
        },
        Preference {
            case: "AfterDiscounts at the organization scope",
            request: Some(json!({
                "resourceType": "RdsDBInstance",
                "scope": {"name": "Organization", "value": "ALL_ACCOUNTS"},
                "savingsEstimationMode": "AfterDiscounts",
            })),
            after_discounts: true,
        },
        Preference {
            case: "AfterDiscounts scoped to another account applies to no database here",
            request: Some(json!({
                "resourceType": "RdsDBInstance",
                "scope": {"name": "AccountId", "value": "210987654321"},
                "savingsEstimationMode": "AfterDiscounts",
            })),
            after_discounts: false,
        },
        Preference {
            case: "AfterDiscounts for another resource type applies to no database",
            request: Some(json!({
                "resourceType": "EbsVolume",
                "savingsEstimationMode": "AfterDiscounts",
            })),
            after_discounts: false,
        },
    ];

    // An over-provisioned database whose INSTANCE the analysis would change, and one whose
    // STORAGE it would: the two option kinds the preference activates a savings member on.
    let instance_db = analysed(
        "db:discounts-instance",
        "Overprovisioned",
        400.0,
        300.0,
        RdsDatabaseDetail {
            current_db_instance_class: Some("db.r6g.2xlarge".to_string()),
            recommended_db_instance_class: Some("db.r6g.xlarge".to_string()),
            utilization_metrics: vec![metric("CPU", 15.0)],
            ..RdsDatabaseDetail::default()
        },
    );
    let storage_db = analysed(
        "db:discounts-storage",
        // The one `Finding` only `RDSStorageFinding` declares, so the option goes on the
        // storage side.
        "NotOptimized",
        60.0,
        40.0,
        RdsDatabaseDetail {
            current_db_instance_class: Some("db.m6g.large".to_string()),
            current_storage_configuration: Some(DBStorageConfiguration {
                storage_type: Some("io1".to_string()),
                allocated_storage: 200,
                iops: Some(5_000),
                max_allocated_storage: Some(400),
                storage_throughput: Some(125),
            }),
            recommended_storage_configuration: Some(DBStorageConfiguration {
                storage_type: Some("gp3".to_string()),
                allocated_storage: 200,
                iops: Some(3_000),
                max_allocated_storage: Some(400),
                storage_throughput: Some(125),
            }),
            ..RdsDatabaseDetail::default()
        },
    );

    for row in &rows {
        // A fresh store per row: a preference one row stored must not decide another's. One row
        // writes at the organization-level scope, which the model admits "for management
        // accounts of an organization only" (`PutRecommendationPreferences$scope`), so every
        // row's caller manages an organization.
        let (router, engine) = management_account_emulator();
        opted_in_and_analysed(&router).await;
        record(&engine, &[instance_db.clone(), storage_db.clone()]);
        if let Some(request) = &row.request {
            put_preference(&router, request).await;
            // The preference round-trips: what was written is what the preference reader
            // reports, whatever the recommendation reader then makes of it.
            let resource_type = request["resourceType"]
                .as_str()
                .expect("the request names a resource type");
            let stored = get_preferences(&router, resource_type, request.get("scope")).await;
            assert_eq!(
                stored["recommendationPreferencesDetails"][0]["savingsEstimationMode"],
                request["savingsEstimationMode"],
                "case {:?}: the stored preference must report the mode that was written, \
                 got {stored}",
                row.case
            );
        }

        for (member, resource, percentage, saved) in [
            (
                "instanceRecommendationOptions",
                &instance_db,
                json!(25.0),
                json!(100.0),
            ),
            (
                "storageRecommendationOptions",
                &storage_db,
                json!(33.33),
                json!(20.0),
            ),
        ] {
            let case = format!("{} / {member}", row.case);
            let reported = reported_for(&router, &case, &resource.arn_in(REGION)).await;
            let option = &reported[member][0];
            assert_eq!(
                option["savingsOpportunity"],
                json!({
                    "savingsOpportunityPercentage": percentage,
                    "estimatedMonthlySavings": {"currency": "USD", "value": saved},
                }),
                "case {case:?}: the saving the stored costs imply is reported whatever the \
                 preference says, got {reported}"
            );
            let expected_after_discounts = if row.after_discounts {
                // No discount is recorded against the database's spend, so the saving net
                // of discounts is the saving before them — in the same currency and on the
                // same percentage.
                json!({
                    "savingsOpportunityPercentage": percentage,
                    "estimatedMonthlySavings": {"currency": "USD", "value": saved},
                })
            } else {
                Value::Null
            };
            assert_eq!(
                option["savingsOpportunityAfterDiscounts"], expected_after_discounts,
                "case {case:?}: expected savingsOpportunityAfterDiscounts \
                 {expected_after_discounts}, got {reported}"
            );
            // The savings estimation mode names where the money figures CAME FROM
            // (`RDSSavingsEstimationModeSource`), and every one of them is computed from the
            // prices stored with this emulator's own analysis either way.
            assert_eq!(
                reported["effectiveRecommendationPreferences"]["savingsEstimationMode"],
                json!({"source": "PublicPricing"}),
                "case {case:?}: the reported source is the one the figures came from, \
                 got {reported}"
            );
        }
    }
}
