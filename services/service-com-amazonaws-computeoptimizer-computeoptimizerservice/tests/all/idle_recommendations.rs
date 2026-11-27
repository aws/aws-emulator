// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `GetIdleRecommendations`.
//!
//! Every assertion drives the service the way a client does — an awsJson1_0 `POST /`
//! through the same `MultiServiceRouter` the emulator binary mounts — so a response that is
//! not computed from persisted state fails here.
//!
//! An idle resource recommendation is produced by Compute Optimizer's own analysis of an
//! observed workload, and no operation of THIS API creates one, so the analysis is part of
//! the store's initial value: a FRESHLY BOOTED emulator already holds it, and the suite below
//! reads it back only over the wire. What is asserted is the RULE the read follows — every
//! member computed from the stored analysis, the ARN naming the Region the request addressed,
//! an answer only for the one account this emulator presents, selection by resource, account
//! and filter, sorting on the requested savings dimension, paging at `maxResults`, and a
//! requested resource with no analysis absent from the answer — over a table of inputs, never
//! one recorded call.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_computeoptimizer_computeoptimizerservice::ops::types::{
    IdleDimension, IdleEstimatedMonthlySavings, IdleRecommendation, IdleSavingsOpportunity,
    IdleSavingsOpportunityAfterDiscounts, IdleUtilizationMetric, SAVINGS_ESTIMATION_MODE_VALUES,
};
use service_com_amazonaws_computeoptimizer_computeoptimizerservice::state::{
    idle_savings_value, service_state, IdleResourceAnalysis, ANALYSIS_DELAY,
    DIMENSION_SAVINGS_VALUE, DIMENSION_SAVINGS_VALUE_AFTER_DISCOUNT, IDLE_MAX_PAGE_SIZE,
    RESOURCE_TYPE_IDLE, SAVINGS_ESTIMATION_AFTER_DISCOUNTS, SCOPE_ACCOUNT_ID, SCOPE_ORGANIZATION,
};

use crate::harness::{
    call, call_in_region, emulator_with_engine, error_code, get_preferences, make_analysis_due,
    management_account_emulator, opt_in, put_preference,
};

/// The ONE account this emulator presents, and so the only account any reader of this
/// service can report on — the account the seeded analysis is held for. Taken from the
/// runtime's single source of truth rather than written out, so the expectations below cannot
/// drift from the account the service answers for.
fn own_account() -> &'static str {
    ::engine::emulator_account_id()
}

/// A well-formed account id that is NOT the caller's: an account of an organization this
/// emulator records none of, so every request naming it is refused.
const MEMBER_ACCOUNT: &str = "210987654321";

/// The refusal a request about another account is answered with, as this crate's readers
/// word it once (`crate::state::CROSS_ACCOUNT_MESSAGE`): it states the permission that is
/// missing and how an organization account gets it, naming neither member nor account.
const CROSS_ACCOUNT_MESSAGE: &str = "Invalid permission to access another account. If this \
     is an organization account, request the management account to enable service access.";

/// The region the harness signs for, and so the regional endpoint every call below addresses
/// unless it names another.
const REGION: &str = "us-east-1";

/// A second regional endpoint, for the rule that the ARN a reader reports names the Region of
/// the endpoint the request addressed.
const OTHER_REGION: &str = "eu-west-1";

/// An ARN in the grammar every AWS service shares:
/// `arn:<partition>:<service>:<region>:<account>:<resource>`.
fn arn(service: &str, region: &str, account_id: &str, resource: &str) -> String {
    format!("arn:aws:{service}:{region}:{account_id}:{resource}")
}

/// One resource of the seeded analysis, as a client sees it: the `<service>` and `<resource>`
/// ARN fields the emulator holds it under, and the members a read derives from the record.
struct Seeded {
    arn_service: &'static str,
    arn_resource: &'static str,
    resource_id: &'static str,
    resource_type: &'static str,
    finding: &'static str,
    /// What the resource costs per month, which is what adopting its recommendation saves.
    monthly_cost: f64,
    /// `lookBackPeriodInDays`: the service default of 14 days, capped by how long the
    /// resource has been observed.
    look_back_days: f64,
}

impl Seeded {
    /// This resource's ARN as the endpoint in `region` reports it.
    fn arn_in(&self, region: &str) -> String {
        arn(self.arn_service, region, own_account(), self.arn_resource)
    }
}

/// The idle resource analysis a freshly booted emulator holds, in record order — the resource
/// kinds Compute Optimizer reports only as idle resources, plus an unattached Amazon EBS
/// volume. Each cost is the price the store records for the resource; each look-back is the
/// 14-day default capped by the days observed (the MemoryDB cluster has been observed for 9).
fn seeded() -> Vec<Seeded> {
    vec![
        Seeded {
            arn_service: "ec2",
            arn_resource: "natgateway/nat-0a1b2c3d4e5f60001",
            resource_id: "nat-0a1b2c3d4e5f60001",
            resource_type: "NatGateway",
            finding: "Unused",
            monthly_cost: 32.85,
            look_back_days: 14.0,
        },
        Seeded {
            arn_service: "ec2",
            arn_resource: "volume/vol-0a1b2c3d4e5f60009",
            resource_id: "vol-0a1b2c3d4e5f60009",
            resource_type: "EBSVolume",
            finding: "Unattached",
            monthly_cost: 20.0,
            look_back_days: 14.0,
        },
        Seeded {
            arn_service: "dynamodb",
            arn_resource: "table/legacy-sessions",
            resource_id: "legacy-sessions",
            resource_type: "DynamoDBTable",
            finding: "Idle",
            monthly_cost: 11.39,
            look_back_days: 14.0,
        },
        Seeded {
            arn_service: "elasticache",
            arn_resource: "cluster:sessions-cache",
            resource_id: "sessions-cache",
            resource_type: "ElastiCacheCluster",
            finding: "Idle",
            monthly_cost: 112.42,
            look_back_days: 14.0,
        },
        Seeded {
            arn_service: "memorydb",
            arn_resource: "cluster/leaderboard-archive",
            resource_id: "leaderboard-archive",
            resource_type: "MemoryDBCluster",
            finding: "Unused",
            monthly_cost: 49.64,
            look_back_days: 9.0,
        },
        Seeded {
            arn_service: "rds",
            arn_resource: "cluster:docdb-reporting",
            resource_id: "docdb-reporting",
            resource_type: "DocumentDBCluster",
            finding: "Idle",
            monthly_cost: 56.94,
            look_back_days: 14.0,
        },
        Seeded {
            arn_service: "workspaces",
            arn_resource: "workspace/ws-0a1b2c3d4e5f6",
            resource_id: "ws-0a1b2c3d4e5f6",
            resource_type: "WorkSpaces",
            finding: "Unused",
            monthly_cost: 35.0,
            look_back_days: 14.0,
        },
        Seeded {
            arn_service: "sagemaker",
            arn_resource: "endpoint/scoring-canary",
            resource_id: "scoring-canary",
            resource_type: "SageMakerEndpoint",
            finding: "Idle",
            monthly_cost: 83.95,
            look_back_days: 14.0,
        },
    ]
}

/// The ARN of every seeded resource at the endpoint in `region`, in record order.
fn seeded_arns(region: &str) -> Vec<String> {
    seeded()
        .iter()
        .map(|resource| resource.arn_in(region))
        .collect()
}

/// One more idle resource of the caller's own account, recorded the way Compute Optimizer's
/// analysis of an observed workload would be — so the suite reads a record it wrote as well as
/// the seeded ones, and a metric `dimensions` breakdown has somewhere to be reported from.
///
/// It is an EC2 instance of 60.00 a month: dearer than every seeded resource but the
/// ElastiCache cluster, so it lands inside a savings ordering rather than at an end.
fn recorded_instance() -> IdleResourceAnalysis {
    IdleResourceAnalysis {
        account_id: own_account().to_string(),
        arn_service: "ec2".to_string(),
        arn_resource: "instance/i-0123456789abcdef0".to_string(),
        finding: "Idle".to_string(),
        utilization_metrics: vec![IdleUtilizationMetric {
            // `IdleMetricName` / `MetricStatistic` values.
            name: Some("CPU".to_string()),
            statistic: Some("Maximum".to_string()),
            value: 1.5,
            dimensions: Some(vec![IdleDimension {
                key: Some("InstanceId".to_string()),
                values: Some(vec!["i-0123456789abcdef0".to_string()]),
            }]),
        }],
        days_observed: 30.0,
        monthly_cost: 60.0,
        tags: vec![("Owner".to_string(), "TeamA".to_string())],
    }
}

/// The ARN of [`recorded_instance`] at the endpoint in `region`.
fn recorded_instance_arn(region: &str) -> String {
    arn(
        "ec2",
        region,
        own_account(),
        "instance/i-0123456789abcdef0",
    )
}

/// Analysis recorded against ANOTHER account — an organization member account this standalone
/// emulator records none of. Nothing a caller of this service may be told about.
fn foreign_analyses() -> Vec<IdleResourceAnalysis> {
    vec![
        IdleResourceAnalysis {
            account_id: MEMBER_ACCOUNT.to_string(),
            arn_service: "dynamodb".to_string(),
            arn_resource: "table/member-orders".to_string(),
            finding: "Idle".to_string(),
            utilization_metrics: Vec::new(),
            days_observed: 14.0,
            monthly_cost: 42.0,
            tags: Vec::new(),
        },
        IdleResourceAnalysis {
            account_id: MEMBER_ACCOUNT.to_string(),
            arn_service: "sagemaker".to_string(),
            arn_resource: "endpoint/member-scoring".to_string(),
            finding: "Unused".to_string(),
            utilization_metrics: Vec::new(),
            days_observed: 14.0,
            monthly_cost: 3.5,
            tags: Vec::new(),
        },
    ]
}

/// Record `analyses` into the store `GetIdleRecommendations` reports from, the way Compute
/// Optimizer's analysis of an observed workload would.
fn record(engine: &Arc<engine::InMemoryEngine>, analyses: Vec<IdleResourceAnalysis>) {
    let state = service_state(&**engine).expect("the engine bears this service's state");
    let mut guard = state.lock();
    for analysis in analyses {
        guard.idle_recommendations.insert(analysis);
    }
}

/// How many resources the store holds an idle analysis for.
fn stored_count(engine: &Arc<engine::InMemoryEngine>) -> usize {
    service_state(&**engine)
        .expect("the engine bears this service's state")
        .lock()
        .idle_recommendations
        .len()
}

/// The `resourceArn` of every recommendation in a response, in the order reported.
fn reported_arns(body: &Value) -> Vec<String> {
    body["idleRecommendations"]
        .as_array()
        .unwrap_or_else(|| panic!("expected a list of idleRecommendations, got {body}"))
        .iter()
        .map(|recommendation| {
            recommendation["resourceArn"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// An emulator whose account is opted in and whose analysis is due, so what it reports is the
/// analysis its enrollment started — the service owes that transition one [`ANALYSIS_DELAY`]
/// after the enrollment is written; the extra margin covers scheduler jitter.
async fn analysed_emulator() -> (
    smithy_server_runtime::MultiServiceRouter,
    Arc<engine::InMemoryEngine>,
) {
    let (router, engine) = emulator_with_engine();
    opt_in(&router).await;
    tokio::time::sleep(ANALYSIS_DELAY + Duration::from_millis(100)).await;
    (router, engine)
}

/// The same emulator, with [`recorded_instance`] recorded beside the seeded analysis, and the
/// ARN of every idle resource of the caller's own account in record order.
async fn emulator_with_recorded_resource() -> (
    smithy_server_runtime::MultiServiceRouter,
    Arc<engine::InMemoryEngine>,
    Vec<String>,
) {
    let (router, engine) = analysed_emulator().await;
    record(&engine, vec![recorded_instance()]);
    let own_arns: Vec<String> = seeded_arns(REGION)
        .into_iter()
        .chain([recorded_instance_arn(REGION)])
        .collect();
    (router, engine, own_arns)
}

/// Read, asserting the call was accepted.
async fn get_idle(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    request: Value,
) -> Value {
    let (status, body) = call(router, "GetIdleRecommendations", request.clone()).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from {request}, got {status}: {body}"
    );
    body
}

/// A freshly booted, enrolled emulator reports the analysis it holds, every member of every
/// recommendation computed from the stored record — and reporting it twice changes nothing a
/// later caller sees.
#[tokio::test]
async fn the_seeded_analysis_is_reported_member_for_member() {
    let (router, engine) = analysed_emulator().await;
    let before = stored_count(&engine);

    let body = get_idle(&router, "the whole analysis", json!({})).await;
    let reported = body["idleRecommendations"]
        .as_array()
        .unwrap_or_else(|| panic!("expected a list of idleRecommendations, got {body}"));
    assert_eq!(
        reported.len(),
        seeded().len(),
        "every idle resource the analysis found is reported, got {body}"
    );

    for (resource, got) in seeded().iter().zip(reported) {
        let case = resource.arn_resource;
        // The whole of what an idle resource costs is saved by dropping it, so the
        // opportunity is 100% of its monthly cost — before and after discounts alike, because
        // this service prices from public pricing and records no negotiated discount.
        let savings = json!({
            "savingsOpportunityPercentage": 100.0,
            "estimatedMonthlySavings": {"currency": "USD", "value": resource.monthly_cost},
        });
        let expected = json!({
            "resourceArn": resource.arn_in(REGION),
            "resourceId": resource.resource_id,
            "resourceType": resource.resource_type,
            "accountId": own_account(),
            "finding": resource.finding,
            "savingsOpportunity": savings,
            // This account stored no savings-estimation-mode preference, and that
            // preference is what activates the after-discount member, so it is reported
            // on no recommendation here.
            "savingsOpportunityAfterDiscounts": Value::Null,
            "lookBackPeriodInDays": resource.look_back_days,
        });
        for (member, wanted) in expected.as_object().expect("an object of members") {
            assert_eq!(
                &got[member], wanted,
                "case {case:?}: member {member:?}: expected {wanted}, got {got}"
            );
        }
        // The finding description summarises the finding the analysis reached, and the
        // metrics are the evidence for it.
        let description = got["findingDescription"].as_str().unwrap_or_default();
        assert!(
            description.contains(resource.resource_type)
                && description.contains(&format!("{} days", resource.look_back_days)),
            "case {case:?}: expected a description of the {} finding over {} days, got {got}",
            resource.finding,
            resource.look_back_days
        );
        let metrics = got["utilizationMetrics"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected utilizationMetrics, got {got}"));
        assert!(
            !metrics.is_empty(),
            "case {case:?}: a finding is reported with the utilization behind it, got {got}"
        );
        for metric in metrics {
            assert_eq!(
                metric["statistic"],
                json!("Maximum"),
                "case {case:?}: this API reports only the Maximum statistic, got {metric}"
            );
            assert!(
                metric["name"].is_string() && metric["value"].is_number(),
                "case {case:?}: a metric names itself and carries a value, got {metric}"
            );
        }
        // The recommendation was generated when the analysis the enrollment started came
        // due, so every row carries the same instant rather than the time of the read.
        assert!(
            got["lastRefreshTimestamp"].is_number(),
            "case {case:?}: expected a refresh timestamp, got {got}"
        );
    }

    assert_eq!(
        body["errors"],
        json!([]),
        "no resource was named, so nothing is missing a recommendation, got {body}"
    );
    assert!(
        body["nextToken"].is_null(),
        "the whole analysis fits on one default page, got {body}"
    );

    // A read reports; it must not change what a later caller sees.
    let again = get_idle(&router, "the same read again", json!({})).await;
    assert_eq!(
        again, body,
        "a second read must report the same analysis, got {again}"
    );
    assert_eq!(
        stored_count(&engine),
        before,
        "a read must not add or drop a stored analysis"
    );
}

/// An analysis recorded against the caller's own account is reported exactly as the seeded
/// ones are: every member derived from the record, including the metric dimensions it holds.
#[tokio::test]
async fn a_recorded_analysis_is_reported_from_its_own_record() {
    let (router, _engine, own_arns) = emulator_with_recorded_resource().await;
    let recorded = recorded_instance_arn(REGION);

    let body = get_idle(
        &router,
        "the recorded resource",
        json!({"resourceArns": [recorded.clone()]}),
    )
    .await;
    assert_eq!(
        reported_arns(&body),
        vec![recorded.clone()],
        "expected exactly the recorded resource, got {body}"
    );
    assert_eq!(
        own_arns.last().map(String::as_str),
        Some(recorded.as_str()),
        "the recorded resource is the last one in record order"
    );

    let got = &body["idleRecommendations"][0];
    let savings = json!({
        "savingsOpportunityPercentage": 100.0,
        "estimatedMonthlySavings": {"currency": "USD", "value": 60.0},
    });
    let expected = json!({
        "resourceArn": recorded,
        "resourceId": "i-0123456789abcdef0",
        "resourceType": "EC2Instance",
        "accountId": own_account(),
        "finding": "Idle",
        "savingsOpportunity": savings,
        // No savings-estimation-mode preference is in force, so the member that
        // preference activates is not reported.
        "savingsOpportunityAfterDiscounts": Value::Null,
        // Observed for 30 days, so the 14-day default is what was analysed.
        "lookBackPeriodInDays": 14.0,
        "utilizationMetrics": [{
            "name": "CPU",
            "statistic": "Maximum",
            "value": 1.5,
            "dimensions": [{"key": "InstanceId", "values": ["i-0123456789abcdef0"]}],
        }],
        "tags": [{"key": "Owner", "value": "TeamA"}],
    });
    for (member, wanted) in expected.as_object().expect("an object of members") {
        assert_eq!(
            &got[member], wanted,
            "member {member:?}: expected {wanted}, got {got}"
        );
    }
}

/// Compute Optimizer is a REGIONAL service: the ARN a recommendation reports names the Region
/// of the endpoint the request addressed, and nothing else about the record changes with it.
#[tokio::test]
async fn a_reported_arn_names_the_region_the_request_addressed() {
    let (router, engine, own_arns) = emulator_with_recorded_resource().await;

    for region in [REGION, OTHER_REGION] {
        let (status, body) =
            call_in_region(&router, region, "GetIdleRecommendations", json!({})).await;
        assert_eq!(
            status, 200,
            "region {region}: expected 200, got {status}: {body}"
        );
        let wanted: Vec<String> = seeded_arns(region)
            .into_iter()
            .chain([recorded_instance_arn(region)])
            .collect();
        assert_eq!(
            reported_arns(&body),
            wanted,
            "region {region}: every ARN must name that Region's endpoint, got {body}"
        );
    }

    // A resource named by the ARN of ANOTHER Region's endpoint is not a resource this
    // endpoint reports, so it is simply absent from the answer here.
    let elsewhere = recorded_instance_arn(OTHER_REGION);
    let body = get_idle(
        &router,
        "the other Region's ARN",
        json!({"resourceArns": [elsewhere.clone()]}),
    )
    .await;
    assert_eq!(
        reported_arns(&body),
        Vec::<String>::new(),
        "the {OTHER_REGION} resource is not reported by the {REGION} endpoint, got {body}"
    );
    assert_eq!(
        body["errors"],
        json!([]),
        "an ARN this endpoint holds no recommendation for is no error of the request, got {body}"
    );
    assert_eq!(
        stored_count(&engine),
        own_arns.len(),
        "reading two Regions must not duplicate a stored analysis"
    );
}

/// The selection rule: a request reports the stored analysis of the resources it names, of
/// the account it names, that satisfies every filter it carries — and a member it omits
/// selects nothing away. Each row differs along one of those dimensions.
#[tokio::test]
async fn the_read_selects_by_resource_account_and_filter() {
    let (router, _engine, own_arns) = emulator_with_recorded_resource().await;
    let [nat_gateway, volume, table, cache, memorydb, docdb, workspace, endpoint, instance] =
        <[String; 9]>::try_from(own_arns.clone()).expect("nine idle resources");

    // (case, request, the ARNs expected in the order expected)
    let rows: Vec<(&str, Value, Vec<String>)> = vec![
        ("no selection", json!({}), own_arns.clone()),
        (
            "one resource ARN",
            json!({"resourceArns": [volume.clone()]}),
            vec![volume.clone()],
        ),
        (
            "three resource ARNs, reported in record order",
            json!({"resourceArns": [endpoint.clone(), nat_gateway.clone(), table.clone()]}),
            vec![nat_gateway.clone(), table.clone(), endpoint.clone()],
        ),
        (
            "the account the analysis is held for",
            json!({"accountIds": [own_account()]}),
            own_arns.clone(),
        ),
        (
            "one finding",
            json!({"filters": [{"name": "Finding", "values": ["Unattached"]}]}),
            vec![volume.clone()],
        ),
        (
            "two findings",
            json!({"filters": [{"name": "Finding", "values": ["Unused", "Unattached"]}]}),
            vec![
                nat_gateway.clone(),
                volume.clone(),
                memorydb.clone(),
                workspace.clone(),
            ],
        ),
        (
            "one resource type",
            json!({"filters": [{"name": "ResourceType", "values": ["DynamoDBTable"]}]}),
            vec![table.clone()],
        ),
        (
            "two resource types",
            json!({"filters": [
                {"name": "ResourceType", "values": ["DocumentDBCluster", "EBSVolume"]},
            ]}),
            vec![volume.clone(), docdb.clone()],
        ),
        (
            "a finding and a resource type are ANDed",
            json!({"filters": [
                {"name": "Finding", "values": ["Idle"]},
                {"name": "ResourceType", "values": ["ElastiCacheCluster", "DynamoDBTable"]},
            ]}),
            vec![table.clone(), cache.clone()],
        ),
        (
            "a finding no analysed resource of that type carries",
            json!({"filters": [
                {"name": "Finding", "values": ["Unattached"]},
                {"name": "ResourceType", "values": ["DynamoDBTable"]},
            ]}),
            Vec::new(),
        ),
        (
            "a resource type no analysed resource is",
            json!({"filters": [{"name": "ResourceType", "values": ["AutoScalingGroup"]}]}),
            Vec::new(),
        ),
        (
            "a filter carrying no values constrains nothing",
            json!({"filters": [{"name": "Finding", "values": []}]}),
            own_arns.clone(),
        ),
        (
            "an account and a resource ARN are ANDed",
            json!({
                "accountIds": [own_account()],
                "resourceArns": [instance.clone(), table.clone()],
            }),
            vec![table.clone(), instance.clone()],
        ),
        (
            // `accountIds: []` is a shape clients send: an empty list names no account, so
            // it selects nothing away.
            "an empty accountIds list names no account",
            json!({"accountIds": []}),
            own_arns.clone(),
        ),
        (
            "an account, a resource ARN and a filter are ANDed",
            json!({
                "accountIds": [own_account()],
                "resourceArns": [instance.clone(), table.clone(), volume.clone()],
                "filters": [{"name": "Finding", "values": ["Idle"]}],
            }),
            vec![table.clone(), instance.clone()],
        ),
    ];

    for (case, request, wanted) in rows {
        let body = get_idle(&router, case, request).await;
        assert_eq!(
            reported_arns(&body),
            wanted,
            "case {case:?}: expected {wanted:?}, got {body}"
        );
    }
}

/// A request that asks about an account other than the one this emulator presents is REFUSED
/// (HTTP 400) rather than used as a selection key, and a recommendation recorded against
/// another account is never reported.
///
/// The rows span the class of foreign ids: the documentation's own example accounts, an id
/// one digit from the caller's own, and both ends of the 12-digit range — each sent through
/// both members that can name an account, alone and together with the caller's own id.
#[tokio::test]
async fn a_request_about_another_account_is_refused() {
    let (router, engine, own_arns) = emulator_with_recorded_resource().await;
    // Analysis recorded for another account, which no answer of this service may carry.
    let foreign = foreign_analyses();
    record(&engine, foreign.clone());

    let foreign_accounts = [
        MEMBER_ACCOUNT,
        "111122223333",
        "123456789012",
        "000000000001",
        "999999999999",
    ];

    for account_id in foreign_accounts {
        let foreign_arn = arn("ec2", REGION, account_id, "instance/i-0123456789abcdef0");
        let requests = [
            ("accountIds alone", json!({"accountIds": [account_id]})),
            (
                "accountIds with a resource of the caller's own account",
                json!({"accountIds": [account_id], "resourceArns": [own_arns[0].clone()]}),
            ),
            (
                "a resource ARN of another account",
                json!({"resourceArns": [foreign_arn.clone()]}),
            ),
            (
                "the caller's own account with another account's resource ARN",
                json!({
                    "accountIds": [own_account()],
                    "resourceArns": [own_arns[0].clone(), foreign_arn.clone()],
                }),
            ),
        ];
        for (member, request) in requests {
            let case = format!("{account_id} via {member}");
            let (status, body) = call(&router, "GetIdleRecommendations", request.clone()).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected {request} to be refused, got {status}: {body}"
            );
            assert_eq!(
                error_code(&body),
                "InvalidParameterValueException",
                "case {case:?}: expected InvalidParameterValueException, got {body}"
            );
            assert_eq!(
                body["message"].as_str(),
                Some(CROSS_ACCOUNT_MESSAGE),
                "case {case:?}: expected the cross-account refusal, got {body}"
            );
            assert!(
                body["idleRecommendations"].is_null(),
                "case {case:?}: a refused request reports nothing, got {body}"
            );
        }
    }

    // What the store holds for another account is reported to nobody: not to a request that
    // names no account, not to one naming the caller's own, and not through its own ARN.
    for (case, request) in [
        ("no account named", json!({})),
        (
            "the caller's own account",
            json!({"accountIds": [own_account()]}),
        ),
        (
            "every resource of the caller's own account named",
            json!({"resourceArns": own_arns.clone()}),
        ),
    ] {
        let body = get_idle(&router, case, request).await;
        assert_eq!(
            reported_arns(&body),
            own_arns,
            "case {case:?}: expected only the caller's own analysis, got {body}"
        );
    }

    // The foreign records are still in the store — the read reported around them, it did not
    // delete them.
    assert_eq!(
        stored_count(&engine),
        own_arns.len() + foreign.len(),
        "a read must not add or drop a stored analysis"
    );
}

/// The ordering rule: `orderBy` sorts the selection on the estimated monthly savings of the
/// dimension it names, ascending unless `Desc` asks otherwise; with no dimension to sort on,
/// the recommendations keep the order the analysis recorded them in.
#[tokio::test]
async fn order_by_sorts_the_selection_on_the_requested_savings_dimension() {
    let (router, _engine, own_arns) = emulator_with_recorded_resource().await;
    let [nat_gateway, volume, table, cache, memorydb, docdb, workspace, endpoint, instance] =
        <[String; 9]>::try_from(own_arns.clone()).expect("nine idle resources");

    // Monthly cost, which is what dropping each resource saves: table 11.39 < volume 20.00 <
    // nat 32.85 < workspace 35.00 < memorydb 49.64 < docdb 56.94 < instance 60.00 <
    // endpoint 83.95 < cache 112.42.
    let ascending = vec![
        table.clone(),
        volume.clone(),
        nat_gateway.clone(),
        workspace.clone(),
        memorydb.clone(),
        docdb.clone(),
        instance.clone(),
        endpoint.clone(),
        cache.clone(),
    ];
    let descending: Vec<String> = ascending.iter().rev().cloned().collect();

    // (case, orderBy, the ARNs expected in the order expected)
    let rows: Vec<(&str, Value, Vec<String>)> = vec![
        ("no orderBy", json!({}), own_arns.clone()),
        (
            "SavingsValue ascending",
            json!({"orderBy": {"dimension": "SavingsValue", "order": "Asc"}}),
            ascending.clone(),
        ),
        (
            "SavingsValue descending",
            json!({"orderBy": {"dimension": "SavingsValue", "order": "Desc"}}),
            descending.clone(),
        ),
        (
            // `SavingsValueAfterDiscount` reads the savings opportunity net of discounts,
            // which the savings-estimation-mode preference activates. This account stored
            // none, so no recommendation carries that member and every one sorts as its
            // modelled default — leaving the record order the sort started from.
            "SavingsValueAfterDiscount with the preference unset",
            json!({"orderBy": {"dimension": "SavingsValueAfterDiscount", "order": "Asc"}}),
            own_arns.clone(),
        ),
        (
            "SavingsValueAfterDiscount descending with the preference unset",
            json!({"orderBy": {"dimension": "SavingsValueAfterDiscount", "order": "Desc"}}),
            own_arns.clone(),
        ),
        (
            "a dimension with no order sorts ascending",
            json!({"orderBy": {"dimension": "SavingsValue"}}),
            ascending.clone(),
        ),
        (
            "an order with no dimension keeps the record order",
            json!({"orderBy": {"order": "Desc"}}),
            own_arns.clone(),
        ),
        (
            "ordering applies to the selection, not to the store",
            json!({
                "filters": [{"name": "Finding", "values": ["Unused"]}],
                "orderBy": {"dimension": "SavingsValue", "order": "Desc"},
            }),
            vec![memorydb.clone(), workspace.clone(), nat_gateway.clone()],
        ),
    ];

    for (case, request, wanted) in rows {
        let body = get_idle(&router, case, request).await;
        assert_eq!(
            reported_arns(&body),
            wanted,
            "case {case:?}: expected {wanted:?}, got {body}"
        );
    }

    // Once the account activates the savings-estimation-mode preference for its idle
    // resources, every recommendation carries the member `SavingsValueAfterDiscount` reads,
    // and that dimension sorts on it — in both directions.
    put_preference(
        &router,
        &json!({
            "resourceType": RESOURCE_TYPE_IDLE,
            "savingsEstimationMode": SAVINGS_ESTIMATION_AFTER_DISCOUNTS,
        }),
    )
    .await;
    for (case, order, wanted) in [
        (
            "SavingsValueAfterDiscount ascending with the preference active",
            "Asc",
            &ascending,
        ),
        (
            "SavingsValueAfterDiscount descending with the preference active",
            "Desc",
            &descending,
        ),
    ] {
        let body = get_idle(
            &router,
            case,
            json!({"orderBy": {"dimension": "SavingsValueAfterDiscount", "order": order}}),
        )
        .await;
        assert_eq!(
            &reported_arns(&body),
            wanted,
            "case {case:?}: expected {wanted:?}, got {body}"
        );
    }
}

/// The sort KEY each `Dimension` reads: `SavingsValue` reads the savings opportunity BEFORE
/// discounts and `SavingsValueAfterDiscount` the one AFTER them, and a recommendation that
/// carries no estimate for the dimension asked for sorts as the modelled default of the member
/// it lacks (`IdleEstimatedMonthlySavings$value` defaults to `0`).
///
/// The analysis this service holds carries equal before- and after-discount figures — it
/// prices from public pricing and records no negotiated discount — so an ordering over it
/// cannot tell the two dimensions apart. This is where the dimension is pinned to the member
/// it reads, over recommendations whose two figures differ.
#[test]
fn each_sort_dimension_reads_its_own_savings_opportunity() {
    let estimate = |value: f64| IdleEstimatedMonthlySavings {
        currency: Some("USD".to_string()),
        value,
    };
    let candidate = |before: Option<f64>, after: Option<f64>| IdleRecommendation {
        savings_opportunity: before.map(|value| IdleSavingsOpportunity {
            savings_opportunity_percentage: 100.0,
            estimated_monthly_savings: Some(estimate(value)),
        }),
        savings_opportunity_after_discounts: after.map(|value| {
            IdleSavingsOpportunityAfterDiscounts {
                savings_opportunity_percentage: 100.0,
                estimated_monthly_savings: Some(estimate(value)),
            }
        }),
        ..IdleRecommendation::default()
    };

    // (case, the recommendation, the key SavingsValue reads, the key
    //  SavingsValueAfterDiscount reads)
    let rows: Vec<(&str, IdleRecommendation, f64, f64)> = vec![
        (
            "a discount that lowers the saving",
            candidate(Some(30.0), Some(2.0)),
            30.0,
            2.0,
        ),
        (
            "a discount that raises it",
            candidate(Some(1.0), Some(9.5)),
            1.0,
            9.5,
        ),
        (
            "no after-discount estimate",
            candidate(Some(7.5), None),
            7.5,
            0.0,
        ),
        ("no estimate at all", candidate(None, None), 0.0, 0.0),
    ];

    for (case, recommendation, wanted_before, wanted_after) in rows {
        for (dimension, wanted) in [
            (DIMENSION_SAVINGS_VALUE, wanted_before),
            (DIMENSION_SAVINGS_VALUE_AFTER_DISCOUNT, wanted_after),
        ] {
            let got = idle_savings_value(&recommendation, dimension);
            assert_eq!(
                got, wanted,
                "case {case:?}: {dimension} must sort on {wanted}, got {got}"
            );
        }
    }
}

/// The paging rule: a page carries at most `maxResults` recommendations and the `nextToken`
/// resumes exactly where it stopped, so walking the pages reports the selection once, in
/// order, whatever page size the walk uses — and no `maxResults` can exceed the ceiling the
/// model puts on it.
#[tokio::test]
async fn paging_walks_the_selection_exactly_once() {
    let (router, _engine, own_arns) = emulator_with_recorded_resource().await;
    let unpaged = get_idle(&router, "unpaged", json!({})).await;
    let every_arn = reported_arns(&unpaged);
    assert_eq!(
        every_arn, own_arns,
        "the whole analysis fits in one default page, got {unpaged}"
    );
    assert!(
        unpaged["nextToken"].is_null(),
        "a complete page carries no nextToken, got {unpaged}"
    );

    // `maxResults` is modelled 0..100: both published bounds, every size that splits the
    // analysis, and one size larger than it.
    for page_size in [0_u64, 1, 2, 3, 4, 5, 8, 9, 10, 100] {
        let case = format!("maxResults={page_size}");
        let mut walked: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request = json!({"maxResults": page_size});
            if let Some(token) = &next_token {
                request["nextToken"] = json!(token);
            }
            let body = get_idle(&router, &case, request).await;
            let page = reported_arns(&body);
            // `0` is the published lower bound and the service documents no meaning for
            // it, so it is read as "no size asked for" — one page of the model's ceiling.
            let cap = usize::try_from(page_size)
                .expect("a page size fits a usize")
                .clamp(1, IDLE_MAX_PAGE_SIZE);
            let cap = if page_size == 0 { IDLE_MAX_PAGE_SIZE } else { cap };
            assert!(
                page.len() <= cap,
                "case {case:?}: a page must carry at most {cap} entries, got {}",
                page.len()
            );
            walked.extend(page);
            pages += 1;
            assert!(
                pages <= every_arn.len() + 1,
                "case {case:?}: the walk must terminate, got {pages} pages"
            );
            match body["nextToken"].as_str() {
                Some(token) => next_token = Some(token.to_string()),
                None => break,
            }
        }
        assert_eq!(
            walked, every_arn,
            "case {case:?}: the walk must report the selection exactly once, in order"
        );
    }

    // Paging a SORTED selection walks exactly that selection, in the order it was sorted in.
    let selection = json!({
        "filters": [{"name": "Finding", "values": ["Idle", "Unused"]}],
        "orderBy": {"dimension": "SavingsValue", "order": "Desc"},
    });
    let sorted = get_idle(&router, "a sorted selection, unpaged", selection.clone()).await;
    let wanted = reported_arns(&sorted);
    let mut walked: Vec<String> = Vec::new();
    let mut next_token: Option<String> = None;
    loop {
        let mut request = selection.clone();
        request["maxResults"] = json!(3);
        if let Some(token) = &next_token {
            request["nextToken"] = json!(token);
        }
        let body = get_idle(&router, "a sorted selection, paged", request).await;
        walked.extend(reported_arns(&body));
        match body["nextToken"].as_str() {
            Some(token) => next_token = Some(token.to_string()),
            None => break,
        }
    }
    assert_eq!(
        walked, wanted,
        "paging a sorted selection must walk it once, in that order"
    );

    // One past the published top bound is refused by the modelled range, before the
    // handler runs.
    let (status, body) = call(
        &router,
        "GetIdleRecommendations",
        json!({"maxResults": IDLE_MAX_PAGE_SIZE + 1}),
    )
    .await;
    assert_eq!(
        status, 400,
        "maxResults={}: outside the modelled range 0..{IDLE_MAX_PAGE_SIZE}, got {status}: {body}",
        IDLE_MAX_PAGE_SIZE + 1
    );
}

/// Regional endpoints other than the one the calls above address: one that reports the same
/// region-free analysis under its OWN ARNs, and one in another partition area again. A token
/// minted at [`REGION`] names a position in neither of their walks.
const OTHER_REGIONS: &[&str] = &[OTHER_REGION, "ap-southeast-2"];

/// Every idle resource of the caller's own account as the endpoint in `region` names it, in
/// record order: the seeded analysis plus [`recorded_instance`].
fn own_arns_in(region: &str) -> Vec<String> {
    seeded_arns(region)
        .into_iter()
        .chain([recorded_instance_arn(region)])
        .collect()
}

/// An opted-in emulator holding [`recorded_instance`] beside the seeded analysis, with that
/// analysis already DUE.
///
/// The due instant is reached by moving the stored enrollment record back a span
/// (`crate::harness::make_analysis_due`) rather than by waiting for the clock to pass it, so
/// the condition the read needs holds however fast or loaded the host is and nothing below
/// depends on elapsed time.
async fn emulator_with_due_analysis() -> smithy_server_runtime::MultiServiceRouter {
    let (router, engine) = emulator_with_engine();
    opt_in(&router).await;
    make_analysis_due(&engine);
    record(&engine, vec![recorded_instance()]);
    router
}

/// A `nextToken` names a position in the walk of the ENDPOINT and the SELECTION it was minted
/// over, and in no other.
///
/// Compute Optimizer is regional while the analysis it holds is region-free: a recommendation
/// names its resource by the ARN of the endpoint reporting it, and that ARN is what a
/// `resourceArns` selection is matched against, so the sequence a token names a position in
/// belongs to the Region the request was signed for. Replayed at another endpoint — or with
/// another selection at its own — the token names no position in the sequence being walked, so
/// it is refused with HTTP 400 `InvalidParameterValueException` rather than answered with a
/// page of a different result set, which would leave a client reading an empty page as "the
/// walk is finished".
///
/// The rows span the selections a token binds: none at all, a pair of the endpoint's own ARNs,
/// a wider subset of them, and every one of them filtered and sorted. Each token is replayed
/// at the endpoint that minted it (where it resumes exactly where its page stopped), at two
/// other endpoints with the selection unchanged and with the same selection restated in those
/// endpoints' OWN ARNs, and at its own endpoint with the selection ordered the other way.
#[tokio::test]
async fn a_page_token_resumes_only_the_endpoint_and_selection_that_minted_it() {
    let router = emulator_with_due_analysis().await;

    // (case, which of the endpoint's own resources the selection names — by index into record
    // order — and the rest of the request)
    let rows: Vec<(&str, &[usize], Value)> = vec![
        ("no resourceArns", &[], json!({})),
        ("two resource ARNs", &[0, 1], json!({})),
        ("four resource ARNs", &[1, 3, 5, 8], json!({})),
        (
            "every resource ARN, filtered and sorted",
            &[0, 1, 2, 3, 4, 5, 6, 7, 8],
            json!({
                "filters": [{"name": "Finding", "values": ["Idle", "Unused"]}],
                "orderBy": {"dimension": "SavingsValue", "order": "Desc"},
            }),
        ),
    ];

    for (case, named, rest) in rows {
        // The selection as the endpoint in `region` writes it: the ARNs it names are the ones
        // that endpoint reports its resources under.
        let selection = |region: &str| -> Value {
            let arns = own_arns_in(region);
            let mut request = rest.clone();
            if !named.is_empty() {
                request["resourceArns"] = json!(named
                    .iter()
                    .map(|index| arns[*index].clone())
                    .collect::<Vec<String>>());
            }
            request
        };

        let whole = reported_arns(&get_idle(&router, case, selection(REGION)).await);
        assert!(
            whole.len() >= 2,
            "case {case:?}: this selection must hold at least two recommendations for a token to \
             resume at one, got {whole:?}"
        );

        let mut first_page = selection(REGION);
        first_page["maxResults"] = json!(1);
        let body = get_idle(&router, case, first_page.clone()).await;
        assert_eq!(
            reported_arns(&body),
            whole[..1].to_vec(),
            "case {case:?}: the first page carries the first recommendation of the selection, \
             got {body}"
        );
        let token = body["nextToken"]
            .as_str()
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: a one-item page of a longer selection hands out a nextToken, \
                     got {body}"
                )
            })
            .to_string();

        // At the endpoint that minted it, for the selection it was minted over, the token
        // resumes exactly where that page stopped.
        let mut resumed = first_page.clone();
        resumed["nextToken"] = json!(token);
        let body = get_idle(&router, case, resumed).await;
        assert_eq!(
            reported_arns(&body),
            whole[1..2].to_vec(),
            "case {case:?}: the token must resume where its page stopped, got {body}"
        );

        for other_region in OTHER_REGIONS {
            // That endpoint has its own page of the same analysis to report, so what the
            // refusals below answer is the TOKEN and not an empty result set.
            let (status, body) = call_in_region(
                &router,
                other_region,
                "GetIdleRecommendations",
                selection(other_region),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?} at {other_region}: expected 200, got {status}: {body}"
            );
            assert!(
                reported_arns(&body).len() >= 2,
                "case {case:?} at {other_region}: this endpoint must have a walk of its own for \
                 a token to be refused rather than answered empty, got {body}"
            );

            let mut replays: Vec<(&str, Value)> =
                vec![("the selection it was minted for", selection(REGION))];
            if !named.is_empty() {
                replays.push((
                    "that selection restated in this endpoint's own ARNs",
                    selection(other_region),
                ));
            }
            for (how, mut request) in replays {
                request["maxResults"] = json!(1);
                request["nextToken"] = json!(token);
                let label = format!("{case}: a {REGION} token replayed at {other_region} with {how}");
                let (status, body) =
                    call_in_region(&router, other_region, "GetIdleRecommendations", request.clone())
                        .await;
                assert_eq!(
                    status, 400,
                    "case {label:?}: expected {request} to be refused with 400, got {status}: \
                     {body}"
                );
                assert_eq!(
                    error_code(&body),
                    "InvalidParameterValueException",
                    "case {label:?}: expected InvalidParameterValueException, got {body}"
                );
                assert!(
                    body["idleRecommendations"].is_null(),
                    "case {label:?}: a refused read reports no page at all, got {body}"
                );
            }
        }

        // A different ORDER over the same resources is a different sequence at the same
        // endpoint, so the token names no position in it either.
        let mut reordered = selection(REGION);
        reordered["orderBy"] = json!({"dimension": DIMENSION_SAVINGS_VALUE, "order": "Asc"});
        reordered["maxResults"] = json!(1);
        reordered["nextToken"] = json!(token);
        let label = format!("{case}: a token replayed over a differently ordered selection");
        let (status, body) =
            call_in_region(&router, REGION, "GetIdleRecommendations", reordered.clone()).await;
        assert_eq!(
            status, 400,
            "case {label:?}: expected {reordered} to be refused with 400, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValueException",
            "case {label:?}: expected InvalidParameterValueException, got {body}"
        );
    }
}

/// A resource the request names that the analysis holds nothing for is simply ABSENT from the
/// answer: the call succeeds, the recommendation list carries only the analysed resources, and
/// `errors` stays empty — it lists resources an idle analysis rejected, not the ones it has
/// not reached.
///
/// The rows span the ARN forms of the model's own `IdleRecommendationResourceType` value set,
/// including resource types the analysis covers no resource of, plus an ARN whose namespace
/// names no idle-recommendable resource.
#[tokio::test]
async fn a_requested_resource_with_no_recommendation_is_absent_rather_than_an_error() {
    let (router, _engine) = analysed_emulator().await;
    let analysed = seeded()[0].arn_in(REGION);

    // Every ARN below names the caller's own account: an ARN of another account is refused
    // outright rather than answered with an empty result.
    let own = |service: &str, resource: &str| arn(service, REGION, own_account(), resource);

    // (case, the requested ARN)
    let rows: Vec<(&str, String)> = vec![
        (
            "an Auto Scaling group",
            own(
                "autoscaling",
                "autoScalingGroup:12345678-1234-1234-1234-123456789012:\
                 autoScalingGroupName/web-tier",
            ),
        ),
        ("an RDS DB instance", own("rds", "db:orders")),
        (
            "a DocumentDB cluster the analysis never looked at, which carries the rds namespace",
            own("rds", "cluster:docdb-analytics"),
        ),
        (
            "an ElastiCache cluster",
            own("elasticache", "cluster:sessions"),
        ),
        ("a MemoryDB cluster", own("memorydb", "cluster/leaderboard")),
        ("a WorkSpace", own("workspaces", "workspace/ws-0123456789")),
        ("an ECS service", own("ecs", "service/payments/checkout")),
        (
            "an EC2 instance the analysis never looked at",
            own("ec2", "instance/i-00000000000000000"),
        ),
        (
            // An ARN whose account field is empty names no other account, so it is not
            // refused — it is simply a resource no idle recommendation covers.
            "an ARN whose namespace names no idle-recommendable resource",
            "arn:aws:s3:::reports-bucket".to_string(),
        ),
    ];

    for (case, requested) in rows {
        let body = get_idle(
            &router,
            case,
            json!({"resourceArns": [requested.clone(), analysed.clone()]}),
        )
        .await;
        // The analysed resource is reported as a recommendation; the one with none is
        // reported nowhere.
        assert_eq!(
            reported_arns(&body),
            vec![analysed.clone()],
            "case {case:?}: expected only the analysed recommendation, got {body}"
        );
        assert_eq!(
            body["errors"],
            json!([]),
            "case {case:?}: a resource with no idle recommendation is no error of the \
             request, got {body}"
        );
    }

    // A resource the analysis covers puts nothing in `errors` either, whatever else the
    // request names, and a filter that selects it away still leaves `errors` empty.
    for (case, request) in [
        (
            "only analysed resources",
            json!({"resourceArns": [analysed.clone()]}),
        ),
        (
            "an analysed resource a filter selects away",
            json!({
                "resourceArns": [analysed.clone()],
                "filters": [{"name": "Finding", "values": ["Idle"]}],
            }),
        ),
    ] {
        let body = get_idle(&router, case, request).await;
        assert_eq!(
            body["errors"],
            json!([]),
            "case {case:?}: a request naming only analysed resources reports no errors, got {body}"
        );
    }
}

/// A request outside the service's documented input rules is refused with HTTP 400 and
/// nothing is reported — one row per rule, each on a value the rule rejects.
#[tokio::test]
async fn an_invalid_request_is_refused() {
    let (router, _engine, own_arns) = emulator_with_recorded_resource().await;
    let analysed = own_arns[0].clone();

    // A token this service really issued, taken over a different selection: resuming it
    // for another selection would report a page of a different result set.
    let other_selection = get_idle(
        &router,
        "a token over another selection",
        json!({"accountIds": [own_account()], "maxResults": 1}),
    )
    .await;
    let foreign_token = other_selection["nextToken"]
        .as_str()
        .expect("a bounded page over a larger selection carries a token")
        .to_string();

    // (case, request, the wire error code expected)
    let rows: Vec<(&str, Value, &str)> = vec![
        (
            // "You can only specify one account ID per request."
            "two account ids",
            json!({"accountIds": [own_account(), own_account()]}),
            "InvalidParameterValueException",
        ),
        (
            "an account id that is not 12 digits",
            json!({"accountIds": ["12345"]}),
            "InvalidParameterValueException",
        ),
        (
            "a resource id that is not an ARN",
            json!({"resourceArns": ["i-0123456789abcdef0"]}),
            "InvalidParameterValueException",
        ),
        (
            "an ARN missing its resource field",
            json!({"resourceArns": [format!("arn:aws:ec2:{REGION}:{}:", own_account())]}),
            "InvalidParameterValueException",
        ),
        (
            "a finding value outside the IdleFinding value set",
            json!({"filters": [{"name": "Finding", "values": ["Oversized"]}]}),
            "InvalidParameterValueException",
        ),
        (
            "a resource type outside the IdleRecommendationResourceType value set",
            json!({"filters": [{"name": "ResourceType", "values": ["LambdaFunction"]}]}),
            "InvalidParameterValueException",
        ),
        (
            "a filter carrying no name",
            json!({"filters": [{"values": ["Idle"]}]}),
            "InvalidParameterValueException",
        ),
        (
            // The service's prose also describes `tag:key` / `tag-key` filter names, but
            // the model's `IdleRecommendationFilterName` declares only Finding and
            // ResourceType, so the request boundary refuses anything else — the model is
            // what this emulator follows, and the boundary refuses it with the same
            // modelled error class the handler's own checks answer with.
            "a filter name the model does not declare",
            json!({"filters": [{"name": "tag:Owner", "values": ["TeamA"]}]}),
            "InvalidParameterValueException",
        ),
        (
            // Likewise `OrderBy$dimension` and `$order` are modelled enums, so a value
            // outside them never reaches the sort.
            "a sort dimension the model does not declare",
            json!({"orderBy": {"dimension": "SavingsPercentage", "order": "Asc"}}),
            "InvalidParameterValueException",
        ),
        (
            "a sort order the model does not declare",
            json!({"orderBy": {"dimension": "SavingsValue", "order": "Descending"}}),
            "InvalidParameterValueException",
        ),
        (
            "a forged nextToken",
            json!({"nextToken": "not-a-token"}),
            "InvalidParameterValueException",
        ),
        (
            "a nextToken issued for another selection",
            json!({"nextToken": foreign_token, "resourceArns": [analysed]}),
            "InvalidParameterValueException",
        ),
    ];

    for (case, request, wanted_code) in rows {
        let (status, body) = call(&router, "GetIdleRecommendations", request.clone()).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected {request} to be refused, got {status}: {body}"
        );
        assert!(
            error_code(&body).contains(wanted_code),
            "case {case:?}: expected {wanted_code}, got {body}"
        );
        assert!(
            body["idleRecommendations"].is_null(),
            "case {case:?}: a refused request reports nothing, got {body}"
        );
    }
}

/// The enrollment gate: "The account is not opted in to Compute Optimizer" is what a read gets
/// while the account's enrollment is not `Active`, whatever the analysis holds — and the same
/// read reports the analysis once it is, but only after the analysis that enrollment starts is
/// due.
#[tokio::test]
async fn a_read_is_refused_while_the_account_is_not_opted_in() {
    let (router, engine) = emulator_with_engine();
    record(&engine, vec![recorded_instance()]);
    let held = seeded().len() + 1;

    // (case, the enrollment to put the account in first, whether the read is accepted)
    let rows: Vec<(&str, Option<&str>, bool)> = vec![
        ("never enrolled", None, false),
        ("opted in", Some("Active"), true),
        ("opted back out", Some("Inactive"), false),
        ("opted in again", Some("Active"), true),
    ];

    for (case, status_to_set, accepted) in rows {
        if let Some(wanted) = status_to_set {
            let (code, body) = call(
                &router,
                "UpdateEnrollmentStatus",
                json!({"status": wanted, "includeMemberAccounts": false}),
            )
            .await;
            assert_eq!(
                code, 200,
                "case {case:?}: expected the enrollment update to be accepted, got {code}: {body}"
            );
        }

        let (status, body) = call(&router, "GetIdleRecommendations", json!({})).await;
        if accepted {
            // The analysis an enrollment starts is not due the instant it is written, so
            // this read reports nothing yet.
            assert_eq!(status, 200, "case {case:?}: expected 200, got {status}: {body}");
            assert_eq!(
                reported_arns(&body),
                Vec::<String>::new(),
                "case {case:?}: the analysis is not due yet, got {body}"
            );

            tokio::time::sleep(ANALYSIS_DELAY + Duration::from_millis(100)).await;
            let due = get_idle(&router, case, json!({})).await;
            assert_eq!(
                reported_arns(&due).len(),
                held,
                "case {case:?}: expected every idle resource once the analysis is due, got {due}"
            );
        } else {
            assert_eq!(
                status, 403,
                "case {case:?}: expected the opt-in refusal, got {status}: {body}"
            );
            assert!(
                error_code(&body).contains("OptInRequiredException"),
                "case {case:?}: expected OptInRequiredException, got {body}"
            );
        }
    }

    assert_eq!(
        stored_count(&engine),
        held,
        "neither a refused nor an accepted read may change the store"
    );
}

/// One row of the savings-estimation-mode table: the preference an account stored, and whether
/// the savings member net of discounts is then reported for its idle resources.
struct SavingsEstimation {
    /// What this row is about.
    case: &'static str,
    /// The `PutRecommendationPreferences` request to store first, or `None` for an account that
    /// stored nothing at all.
    request: Option<Value>,
    /// Whether `savingsOpportunityAfterDiscounts` is then reported on each recommendation.
    after_discounts: bool,
}

/// The `SavingsEstimationMode` value that DEACTIVATES the preference: the one the model
/// declares beside the value that activates it
/// (`crate::ops::types::SAVINGS_ESTIMATION_MODE_VALUES`), rather than a spelling written into
/// this suite.
fn deactivating_mode() -> &'static str {
    SAVINGS_ESTIMATION_MODE_VALUES
        .iter()
        .copied()
        .find(|value| *value != SAVINGS_ESTIMATION_AFTER_DISCOUNTS)
        .expect("the model declares a value that deactivates the preference")
}

/// The savings-estimation-mode preference in force for an idle resource is applied to its
/// recommendation: the savings member net of discounts is reported exactly when a record that
/// applies to the resource activates the preference, and is reported on no recommendation when
/// none does — "specify the `AfterDiscounts` status to activate the preference, or specify
/// `BeforeDiscounts` to deactivate the preference".
///
/// The rows span the scopes an idle-resource preference can be stored at (the account default,
/// the account, the organization), the value that deactivates the preference, a scope naming
/// another account and a record of another resource type — the last two applying to nothing
/// this request reports.
#[tokio::test]
async fn the_savings_estimation_mode_preference_is_applied_to_idle_recommendations() {
    let rows: Vec<SavingsEstimation> = vec![
        SavingsEstimation {
            case: "an account that stored no preference",
            request: None,
            after_discounts: false,
        },
        SavingsEstimation {
            case: "the value that deactivates the preference",
            request: Some(json!({
                "resourceType": RESOURCE_TYPE_IDLE,
                "savingsEstimationMode": deactivating_mode(),
            })),
            after_discounts: false,
        },
        SavingsEstimation {
            case: "activated as the account default",
            request: Some(json!({
                "resourceType": RESOURCE_TYPE_IDLE,
                "savingsEstimationMode": SAVINGS_ESTIMATION_AFTER_DISCOUNTS,
            })),
            after_discounts: true,
        },
        SavingsEstimation {
            case: "activated at the account scope",
            request: Some(json!({
                "resourceType": RESOURCE_TYPE_IDLE,
                "scope": {"name": SCOPE_ACCOUNT_ID, "value": own_account()},
                "savingsEstimationMode": SAVINGS_ESTIMATION_AFTER_DISCOUNTS,
            })),
            after_discounts: true,
        },
        SavingsEstimation {
            case: "activated at the organization scope",
            request: Some(json!({
                "resourceType": RESOURCE_TYPE_IDLE,
                "scope": {"name": SCOPE_ORGANIZATION, "value": "ALL_ACCOUNTS"},
                "savingsEstimationMode": SAVINGS_ESTIMATION_AFTER_DISCOUNTS,
            })),
            after_discounts: true,
        },
        SavingsEstimation {
            case: "activated for another account, which applies to no resource here",
            request: Some(json!({
                "resourceType": RESOURCE_TYPE_IDLE,
                "scope": {"name": SCOPE_ACCOUNT_ID, "value": MEMBER_ACCOUNT},
                "savingsEstimationMode": SAVINGS_ESTIMATION_AFTER_DISCOUNTS,
            })),
            after_discounts: false,
        },
        SavingsEstimation {
            case: "activated for another resource type, which applies to no idle resource",
            request: Some(json!({
                "resourceType": "EbsVolume",
                "savingsEstimationMode": SAVINGS_ESTIMATION_AFTER_DISCOUNTS,
            })),
            after_discounts: false,
        },
    ];

    for row in &rows {
        // A fresh store per row: a preference one row stored must not decide another's. The
        // analysis is made due by moving the enrollment instant back rather than by waiting,
        // so nothing here depends on how fast the host runs. One row writes at the
        // organization-level scope, which the model admits "for management accounts of an
        // organization only" (`PutRecommendationPreferences$scope`), so every row's caller
        // manages an organization.
        let (router, engine) = management_account_emulator();
        opt_in(&router).await;
        make_analysis_due(&engine);
        record(&engine, vec![recorded_instance()]);
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

        let body = get_idle(&router, row.case, json!({"maxResults": IDLE_MAX_PAGE_SIZE})).await;
        let reported = body["idleRecommendations"]
            .as_array()
            .unwrap_or_else(|| panic!("case {:?}: a list of recommendations: {body}", row.case))
            .clone();
        assert_eq!(
            reported.len(),
            seeded().len() + 1,
            "case {:?}: every idle resource of the account is reported, got {body}",
            row.case
        );
        for got in &reported {
            let case = format!("{} / {}", row.case, got["resourceArn"]);
            // The saving the analysis implies is reported whatever the preference says: the
            // whole of what an idle resource costs.
            let before = &got["savingsOpportunity"];
            let saved = before["estimatedMonthlySavings"]["value"]
                .as_f64()
                .unwrap_or_default();
            assert!(
                saved > 0.0,
                "case {case:?}: dropping an idle resource saves what it costs, got {got}"
            );
            // Nothing in this service's analysis records a negotiated discount, so where the
            // preference activates the after-discount member it restates those same figures —
            // and where no record activates it, the member is not reported at all.
            let expected = if row.after_discounts {
                before.clone()
            } else {
                Value::Null
            };
            assert_eq!(
                got["savingsOpportunityAfterDiscounts"], expected,
                "case {case:?}: expected savingsOpportunityAfterDiscounts {expected}, got {got}"
            );
        }
    }
}
