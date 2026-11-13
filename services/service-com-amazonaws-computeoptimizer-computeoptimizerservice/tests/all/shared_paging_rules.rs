// SPDX-License-Identifier: CC0-1.0

//! The two paging rules EVERY paginated reader of this service shares, driven over the wire.
//!
//! Every call goes through the shared wire harness (`tests/all/harness.rs`), i.e. the same
//! `MultiServiceRouter::dispatch` the emulator's front door uses, so what these tests prove
//! is what a client sees.
//!
//! The rules, which `crate::state::page_size` and `crate::state::offset_page` state once for
//! the whole crate:
//!
//! * HOW LARGE a page is — the size the request asks for, the reader's own default when it
//!   asks for none (an absent `maxResults`, or the modelled range's lower bound `0`, which
//!   the service documents no meaning for), and never more than the reader's own ceiling.
//! * WHICH items that page carries and what follows it — the items after the point the
//!   previous page stopped at, with a `nextToken` handed out exactly when the sequence
//!   continues past the page, resuming where it stopped.
//!
//! Both are asserted over a TABLE of every reader of this service whose page the request
//! sizes, each with its own default and ceiling (the eleven rows below), crossed with the
//! page sizes the rule generalises over: none at all, the range's lower bound, one item, one
//! short of the sequence, exactly the sequence, one past it, and the reader's own ceiling.
//! Nothing here asserts a page size the recorded calls happened to use; what is asserted is
//! that no reader answers a page size differently from the others.
//!
//! `GetRecommendationSummaries` is NOT one of them: its answer is one row per
//! `RecommendationSourceType` the service reports on, a set the vocabulary BOUNDS rather than
//! the account's resources growing, so it is answered whole and hands out no page boundary
//! at all (`tests/all/summary_block_sets.rs` states that rule).

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use service_com_amazonaws_computeoptimizer_computeoptimizerservice::state::{
    service_state, AnalysedResource, MemberAccountEnrollment, ANALYSIS_DELAY, DEFAULT_PAGE_SIZE,
    ENROLLMENT_STATUSES_MAX_PAGE_SIZE, IDLE_MAX_PAGE_SIZE, MAX_PAGE_SIZE, SCOPE_ACCOUNT_ID,
    SCOPE_ORGANIZATION, SCOPE_RESOURCE_ARN, SOURCE_TYPE_RDS_DB_INSTANCE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{
    call, call_ecs, create_bucket, emulator_with_analysed_services_and_engine, instance_arn,
    opt_in, put_preference, seed_cloudwatch_series, ACCOUNT,
};

/// The destination bucket the export jobs below are created against. Compute Optimizer does
/// not create it; the request just names it.
const BUCKET: &str = "shared-paging-rules";

/// The cluster the analysed Amazon ECS services below run in.
const CLUSTER: &str = "paging-rules";

/// One paginated reader of this service: how to call it, where its page lives in the
/// response, what identifies an item of that page, and the two numbers it applies the shared
/// page-size rule with.
struct PagedReader {
    /// The operation name, as `X-Amz-Target` carries it.
    operation: &'static str,
    /// The request members other than `maxResults` and `nextToken`.
    request: Value,
    /// The output member the page is reported in.
    items: &'static str,
    /// The path to the member of an item that identifies it, so two reads can be compared
    /// item by item without depending on anything a clock derives.
    identity: &'static [&'static str],
    /// The page size this reader uses when the request asks for none.
    default_page: usize,
    /// The largest page this reader will build, whatever the request asks for.
    cap: usize,
}

/// Every reader of this service whose page a request sizes, each with the default and the
/// ceiling it applies the shared page-size rule with: nine of them page at the service's own
/// default under the model's 0..1000 ceiling, while `GetIdleRecommendations` and
/// `GetEnrollmentStatusesForOrganization` are bounded lower by what their own members
/// document.
fn readers() -> Vec<PagedReader> {
    vec![
        PagedReader {
            operation: "DescribeRecommendationExportJobs",
            request: json!({}),
            items: "recommendationExportJobs",
            identity: &["jobId"],
            default_page: DEFAULT_PAGE_SIZE,
            cap: MAX_PAGE_SIZE,
        },
        PagedReader {
            operation: "GetAutoScalingGroupRecommendations",
            request: json!({}),
            items: "autoScalingGroupRecommendations",
            identity: &["autoScalingGroupArn"],
            default_page: DEFAULT_PAGE_SIZE,
            cap: MAX_PAGE_SIZE,
        },
        PagedReader {
            operation: "GetEBSVolumeRecommendations",
            request: json!({}),
            items: "volumeRecommendations",
            identity: &["volumeArn"],
            default_page: DEFAULT_PAGE_SIZE,
            cap: MAX_PAGE_SIZE,
        },
        PagedReader {
            operation: "GetEC2InstanceRecommendations",
            request: json!({}),
            items: "instanceRecommendations",
            identity: &["instanceArn"],
            default_page: DEFAULT_PAGE_SIZE,
            cap: MAX_PAGE_SIZE,
        },
        PagedReader {
            operation: "GetECSServiceRecommendations",
            request: json!({}),
            items: "ecsServiceRecommendations",
            identity: &["serviceArn"],
            default_page: DEFAULT_PAGE_SIZE,
            cap: MAX_PAGE_SIZE,
        },
        PagedReader {
            operation: "GetEnrollmentStatusesForOrganization",
            request: json!({}),
            items: "accountEnrollmentStatuses",
            identity: &["accountId"],
            default_page: ENROLLMENT_STATUSES_MAX_PAGE_SIZE,
            cap: ENROLLMENT_STATUSES_MAX_PAGE_SIZE,
        },
        PagedReader {
            operation: "GetIdleRecommendations",
            request: json!({}),
            items: "idleRecommendations",
            identity: &["resourceArn"],
            default_page: IDLE_MAX_PAGE_SIZE,
            cap: IDLE_MAX_PAGE_SIZE,
        },
        PagedReader {
            operation: "GetLambdaFunctionRecommendations",
            request: json!({}),
            items: "lambdaFunctionRecommendations",
            identity: &["functionArn"],
            default_page: DEFAULT_PAGE_SIZE,
            cap: MAX_PAGE_SIZE,
        },
        PagedReader {
            operation: "GetLicenseRecommendations",
            request: json!({}),
            items: "licenseRecommendations",
            identity: &["resourceArn"],
            default_page: DEFAULT_PAGE_SIZE,
            cap: MAX_PAGE_SIZE,
        },
        PagedReader {
            operation: "GetRDSDatabaseRecommendations",
            request: json!({}),
            items: "rdsDBRecommendations",
            identity: &["resourceArn"],
            default_page: DEFAULT_PAGE_SIZE,
            cap: MAX_PAGE_SIZE,
        },
        PagedReader {
            operation: "GetRecommendationPreferences",
            request: json!({"resourceType": "Ec2Instance"}),
            items: "recommendationPreferencesDetails",
            identity: &["scope", "value"],
            default_page: DEFAULT_PAGE_SIZE,
            cap: MAX_PAGE_SIZE,
        },
    ]
}

/// A page size the request asks for, and the page it must be answered with.
#[derive(Debug)]
struct PageSizeCase {
    /// What the row is, for the assertion message.
    name: String,
    /// The `maxResults` the request carries, or `None` for a request naming none.
    max_results: Option<usize>,
    /// The items the first page must carry, by identity.
    expected: Vec<String>,
    /// Whether a `nextToken` must come with that page.
    expect_token: bool,
}

/// Whatever page size a request asks for, every reader answers it by the same rule: the size
/// asked for when it asks for one, the reader's own default when it asks for none, and never
/// more than the reader's own ceiling.
#[tokio::test]
async fn the_page_size_rule_is_the_same_for_every_paginated_reader() {
    let (router, _engine) = paged_emulator().await;

    for reader in readers() {
        let whole = page_of(&read(&router, &reader, "the unpaged read", None, None).await, &reader);
        let count = whole.len();
        assert!(
            count >= 2,
            "case {:?}: this fixture must hold at least two items for a page size to be \
             observable, got {whole:?}",
            reader.operation
        );
        assert!(
            count <= reader.default_page,
            "case {:?}: this fixture must fit one default-sized page of {}, got {count}",
            reader.operation,
            reader.default_page
        );

        // The rule generalises over the size asked for, so the rows span the forms that size
        // can take: none at all, the modelled range's lower bound (no size asked for), one
        // item, one short of the sequence, exactly it, one past it, and the ceiling.
        let cases = vec![
            PageSizeCase {
                name: "no maxResults at all".to_string(),
                max_results: None,
                expected: whole.clone(),
                expect_token: false,
            },
            PageSizeCase {
                name: "the modelled range's lower bound 0".to_string(),
                max_results: Some(0),
                expected: whole.clone(),
                expect_token: false,
            },
            PageSizeCase {
                name: "one item per page".to_string(),
                max_results: Some(1),
                expected: whole[..1].to_vec(),
                expect_token: true,
            },
            PageSizeCase {
                name: format!("one short of the sequence ({})", count - 1),
                max_results: Some(count - 1),
                expected: whole[..count - 1].to_vec(),
                expect_token: true,
            },
            PageSizeCase {
                name: format!("exactly the sequence ({count})"),
                max_results: Some(count),
                expected: whole.clone(),
                expect_token: false,
            },
            PageSizeCase {
                name: format!("one past the sequence ({})", count + 1),
                max_results: Some(count + 1),
                expected: whole.clone(),
                expect_token: false,
            },
            PageSizeCase {
                name: format!("the reader's own ceiling ({})", reader.cap),
                max_results: Some(reader.cap),
                expected: whole.clone(),
                expect_token: false,
            },
        ];

        for case in cases {
            let label = format!("{} / {}", reader.operation, case.name);
            let body = read(&router, &reader, &label, case.max_results, None).await;
            let page = page_of(&body, &reader);
            assert_eq!(
                page, case.expected,
                "case {label:?}: expected the page {:?}, got {page:?}",
                case.expected
            );
            assert!(
                page.len() <= reader.cap,
                "case {label:?}: a page may hold at most {} items, got {}",
                reader.cap,
                page.len()
            );
            assert_eq!(
                body["nextToken"].as_str().is_some(),
                case.expect_token,
                "case {label:?}: expected a nextToken: {}, got {body}",
                case.expect_token
            );
        }
    }
}

/// Whatever page size walks a reader's sequence, the walk reports that reader's unpaged
/// sequence exactly once, and only its final page comes without a token.
#[tokio::test]
async fn a_paged_walk_reports_the_unpaged_sequence_exactly_once() {
    let (router, _engine) = paged_emulator().await;

    for reader in readers() {
        let whole = page_of(&read(&router, &reader, "the unpaged read", None, None).await, &reader);
        let count = whole.len();

        // Rows: every page size that splits the sequence differently — one item at a time,
        // two at a time, and the whole sequence in one page.
        for page_size in [1, 2, count] {
            let label = format!("{} / maxResults={page_size}", reader.operation);
            let mut walked: Vec<String> = Vec::new();
            let mut pages: Vec<usize> = Vec::new();
            let mut token: Option<String> = None;
            loop {
                let body = read(&router, &reader, &label, Some(page_size), token.as_deref()).await;
                let page = page_of(&body, &reader);
                assert!(
                    page.len() <= page_size,
                    "case {label:?}: a page may hold at most {page_size} items, got {}",
                    page.len()
                );
                pages.push(page.len());
                walked.extend(page);
                token = body["nextToken"].as_str().map(ToString::to_string);
                let remaining = count - walked.len();
                assert_eq!(
                    token.is_some(),
                    remaining > 0,
                    "case {label:?}: with {remaining} items left, expected a nextToken: {}, got \
                     {body}",
                    remaining > 0
                );
                if token.is_none() {
                    break;
                }
                assert!(
                    pages.len() <= count,
                    "case {label:?}: the walk does not terminate, pages so far {pages:?}"
                );
            }
            assert_eq!(
                walked, whole,
                "case {label:?}: a paged walk must report the unpaged sequence, got {walked:?}"
            );
        }
    }
}

/// One page of `reader`'s response, as the identities of the items it carries.
fn page_of(body: &Value, reader: &PagedReader) -> Vec<String> {
    body[reader.items]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case {:?}: expected a list of {}, got {body}",
                reader.operation, reader.items
            )
        })
        .iter()
        .map(|item| {
            let mut found = item;
            for member in reader.identity {
                found = &found[*member];
            }
            found
                .as_str()
                .unwrap_or_else(|| {
                    panic!(
                        "case {:?}: expected every item to carry {:?}, got {item}",
                        reader.operation, reader.identity
                    )
                })
                .to_string()
        })
        .collect()
}

/// Read one page from `reader`, asserting the call was accepted.
async fn read(
    router: &MultiServiceRouter,
    reader: &PagedReader,
    case: &str,
    max_results: Option<usize>,
    token: Option<&str>,
) -> Value {
    let mut request = reader.request.clone();
    if let Some(max_results) = max_results {
        request["maxResults"] = json!(max_results);
    }
    if let Some(token) = token {
        request["nextToken"] = json!(token);
    }
    let (status, body) = call(router, reader.operation, request.clone()).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from {request}, got {status}: {body}"
    );
    body
}

/// An emulator every reader in [`readers`] has at least two items to report from: the
/// account is enrolled, the organization holds member accounts, preference records are
/// written at four distinct scopes, export jobs exist for three resource types, two analysed
/// databases are recorded, and two Amazon ECS services have the analysis inputs a
/// recommendation is computed from.
async fn paged_emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let (router, engine) = emulator_with_analysed_services_and_engine();
    opt_in(&router).await;
    // The analysis an enrollment starts is the gate every recommendation reader of this
    // service applies, so the fixture lets it come due; the span is the service's own
    // ([`ANALYSIS_DELAY`]) and the margin covers scheduler jitter.
    tokio::time::sleep(ANALYSIS_DELAY + Duration::from_millis(100)).await;
    record_organization(&engine);
    record_analysed_databases(&engine);
    write_preference_records(&router).await;
    create_bucket(&router, BUCKET).await;
    create_export_jobs(&router).await;
    analyse_ecs_services(&router, &engine).await;
    (router, engine)
}

/// Record member accounts for the organization, the way AWS Organizations would: no operation
/// of THIS API creates one, and `GetEnrollmentStatusesForOrganization` reports one row per
/// member account the store holds.
fn record_organization(engine: &Arc<engine::InMemoryEngine>) {
    let state = service_state(&**engine).expect("the engine bears this service's state");
    let mut guard = state.lock();
    for account_id in ["200000000001", "200000000002", "200000000003"] {
        guard
            .organization_members
            .insert(MemberAccountEnrollment::new(account_id));
    }
}

/// Record two analysed Amazon Aurora or RDS databases into the findings catalog, the way
/// Compute Optimizer's own analysis of an observed workload would: a page size is only
/// observable over a sequence of more than one.
fn record_analysed_databases(engine: &Arc<engine::InMemoryEngine>) {
    let state = service_state(&**engine).expect("the engine bears this service's state");
    let mut guard = state.lock();
    for identifier in ["db:ledger-primary", "db:ledger-replica"] {
        guard.analysed_resources.push(AnalysedResource {
            account_id: ::engine::emulator_account_id().to_string(),
            resource_type: SOURCE_TYPE_RDS_DB_INSTANCE.to_string(),
            // The catalog records a resource's REGION-FREE identity; the ARN reported for it
            // names the region of the endpoint that reports it.
            arn_service: "rds".to_string(),
            arn_resource: identifier.to_string(),
            // An `RDSInstanceFinding` value with nothing to change, so the row needs no
            // recommendation option of its own.
            finding: "Optimized".to_string(),
            finding_reason_codes: Vec::new(),
            current_performance_risk: "VeryLow".to_string(),
            current_monthly_cost: 120.0,
            recommended_monthly_cost: 120.0,
            inferred_workload_types: Vec::new(),
            rds_database: None,
        });
    }
}

/// Write one preference record at each of four distinct scopes, which is what
/// `GetRecommendationPreferences` reports one row per. The value written is the one the
/// store already presents, so no other reader's recommendation changes.
async fn write_preference_records(router: &MultiServiceRouter) {
    let scopes = [
        json!({"name": SCOPE_ACCOUNT_ID, "value": ACCOUNT}),
        json!({"name": SCOPE_ORGANIZATION, "value": "ALL_ACCOUNTS"}),
        json!({"name": SCOPE_RESOURCE_ARN, "value": instance_arn("i-0ab100000000000a1")}),
        json!({"name": SCOPE_RESOURCE_ARN, "value": instance_arn("i-0ab100000000000a2")}),
    ];
    for scope in scopes {
        put_preference(
            router,
            &json!({
                "resourceType": "Ec2Instance",
                "scope": scope,
                "enhancedInfrastructureMetrics": "Inactive",
            }),
        )
        .await;
    }
}

/// Create one export job per resource type, which is what `DescribeRecommendationExportJobs`
/// reports. The resource types differ because only one job per resource type may be in
/// progress in a Region at a time.
async fn create_export_jobs(router: &MultiServiceRouter) {
    for operation in [
        "ExportIdleRecommendations",
        "ExportEC2InstanceRecommendations",
        "ExportEBSVolumeRecommendations",
    ] {
        let (status, body) = call(
            router,
            operation,
            json!({"s3DestinationConfig": {"bucket": BUCKET}}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {operation:?}: expected the export to be accepted, got {status}: {body}"
        );
    }
}

/// Give two Amazon ECS services the inputs a recommendation is computed from: the service the
/// ECS store holds, and the `AWS/ECS` utilization published for it.
async fn analyse_ecs_services(router: &MultiServiceRouter, engine: &Arc<engine::InMemoryEngine>) {
    let observed_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_secs_f64()
        - 3_600.0;
    ecs_ok(router, "CreateCluster", json!({"clusterName": CLUSTER})).await;
    for service in ["alpha", "bravo"] {
        let family = format!("{service}-family");
        ecs_ok(
            router,
            "RegisterTaskDefinition",
            json!({
                "family": family,
                "networkMode": "awsvpc",
                "requiresCompatibilities": ["FARGATE"],
                "cpu": "1024",
                "memory": "2048",
                "containerDefinitions": [{
                    "name": "web",
                    "image": "public.ecr.aws/nginx/nginx:latest",
                    "cpu": 128,
                    "memory": 256,
                    "memoryReservation": 192,
                }],
            }),
        )
        .await;
        ecs_ok(
            router,
            "CreateService",
            json!({
                "cluster": CLUSTER,
                "serviceName": service,
                "taskDefinition": family,
                "desiredCount": 1,
                "launchType": "FARGATE",
                // The revision above is in the `awsvpc` network mode, whose tasks each get
                // their own elastic network interface, so the launch says which subnets and
                // security groups that interface takes.
                "networkConfiguration": {
                    "awsvpcConfiguration": {
                        "subnets": ["subnet-0123456789abcdef0"],
                        "securityGroups": ["sg-0123456789abcdef0"],
                    }
                },
            }),
        )
        .await;
        // `AWS/ECS` is a namespace AWS reserves for the service that owns the metric, so the
        // utilization analysed here is seeded the way ECS writes it.
        for metric_name in ["CPUUtilization", "MemoryUtilization"] {
            seed_cloudwatch_series(
                engine,
                "AWS/ECS",
                metric_name,
                &[("ClusterName", CLUSTER), ("ServiceName", service)],
                Some("Percent"),
                observed_at,
                &[20.0, 30.0],
            );
        }
    }
}

/// One Amazon ECS call that must be accepted.
async fn ecs_ok(router: &MultiServiceRouter, operation: &str, body: Value) {
    let (status, parsed) = call_ecs(router, operation, body).await;
    assert_eq!(
        status, 200,
        "case {operation:?}: expected 200, got {status}: {parsed}"
    );
}
