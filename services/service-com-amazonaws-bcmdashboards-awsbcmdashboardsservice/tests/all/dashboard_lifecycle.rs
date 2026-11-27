// SPDX-License-Identifier: CC0-1.0

//! The dashboard lifecycle, end to end over the wire: `CreateDashboard`,
//! `GetDashboard`, `ListDashboards`, `UpdateDashboard`, `DeleteDashboard`.
//!
//! Every call goes through the real router (`POST /` with
//! `X-Amz-Target: AWSBCMDashboardsService.<Op>`, awsJson1_0), so what these tests
//! assert is what an SDK client sees: the response is built from the persisted
//! record, not from the request.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_bcmdashboards_awsbcmdashboardsservice::state::MAX_DASHBOARDS;
use service_com_amazonaws_bcmdashboards_awsbcmdashboardsservice::EmulatedService;

/// A router serving this one service over a fresh engine, as `build_router` does
/// in production. Shared with the sibling audit suite.
pub(crate) fn router() -> MultiServiceRouter {
    emulator().0
}

/// The same router, together with the engine it was built over.
///
/// EventBridge is mounted BESIDE this service on that one engine, because an execution of
/// a scheduled report delivers one notification onto the account's default event bus
/// (`state::deliver_execution_notification`), reaching EventBridge through the in-process
/// callback: with no EventBridge mounted there is nothing for that delivery to reach, so
/// every execution would report the report as unable to deliver it. EventBridge claims
/// only requests scoped to its own signing name (`events`), so no bcm-dashboards request
/// below is routed any differently for its presence.
///
/// The engine is handed back so a suite can read the EventBridge store the delivery
/// reaches — the event a client observes — exactly as the handlers reach their own.
pub(crate) fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    router.register_service(
        service_com_amazonaws_eventbridge_awsevents::EmulatedService::new(Arc::clone(&engine)),
    );
    (router, engine)
}

/// A router serving ONLY this service, with no EventBridge for an execution's
/// notification to reach: the emulator a client gets when the event bus the model grounds
/// a scheduled report's health in is not there at all.
pub(crate) fn router_without_event_bus() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// Call one operation and return `(http status, parsed body)`.
pub(crate) async fn call(
    router: &MultiServiceRouter,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    call_signed_in_region(router, operation, body, "us-east-1").await
}

/// [`call`], with the Region field of the request's SigV4 credential scope chosen by the
/// caller: that scope is the only thing naming the Region a request addressed, so a suite
/// whose subject is what the service does with it states it per call.
pub(crate) async fn call_signed_in_region(
    router: &MultiServiceRouter,
    operation: &str,
    body: &Value,
    scope_region: &str,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        // The routing signal an SDK request carries: the SigV4 credential scope names
        // this service's signing name (`bcm-dashboards`). Request authentication is off
        // for this router, so the signature itself is never checked.
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 \
                 Credential=AKIDEXAMPLE/20250818/{scope_region}/bcm-dashboards/aws4_request, \
                 SignedHeaders=host;x-amz-date;x-amz-target, Signature=0000"
            ),
        )
        .header("host", "bcm-dashboards.us-east-1.amazonaws.com")
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("AWSBCMDashboardsService.{operation}"))
        .body(Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|e| {
            panic!(
                "{operation} answered a body that is not JSON ({e}): {}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    (status, parsed)
}

/// Succeed-or-explain: the call must answer 200.
pub(crate) async fn ok_call(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let (status, parsed) = call(router, operation, body).await;
    assert_eq!(status, 200, "{operation} expected 200, got {status}: {parsed}");
    parsed
}

/// The access key id the requests [`call`] builds present, which an audit record must
/// attribute the call to.
pub(crate) const PRESENTED_AKID: &str = "AKIDEXAMPLE";

/// The `Host` the requests [`call`] builds address, which an audit record reports as the
/// request's source.
pub(crate) const REQUEST_SOURCE: &str = "bcm-dashboards.us-east-1.amazonaws.com";

/// The wire error code (`__type`) of an error body.
pub(crate) fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .to_string()
}

/// The AWS-managed dashboard catalog a fresh store already holds, as `(arn, name)` pairs
/// in ARN order.
///
/// These dashboards are the service's own: an account that has created nothing already
/// lists them, so a test that states what a listing reports states its own dashboards
/// AND these. Read from the one place that defines the catalog
/// (`state::managed_dashboard_catalog`), so no count or ARN is written out here.
pub(crate) fn managed_catalog() -> Vec<(String, String)> {
    service_com_amazonaws_bcmdashboards_awsbcmdashboardsservice::state::managed_dashboard_catalog()
        .into_values()
        .map(|dashboard| (dashboard.arn, dashboard.name))
        .collect()
}

/// Every ARN of the AWS-managed catalog, in ARN order.
pub(crate) fn managed_dashboard_arns() -> Vec<String> {
    managed_catalog().into_iter().map(|(arn, _)| arn).collect()
}

/// How many dashboards the AWS-managed catalog holds.
pub(crate) fn managed_dashboard_count() -> usize {
    managed_catalog().len()
}

/// `entries` together with the AWS-managed catalog, sorted: what a listing of the whole
/// account reports once a test's own dashboards are stored beside the seeded ones.
pub(crate) fn with_managed_catalog(
    entries: impl IntoIterator<Item = (String, String)>,
) -> Vec<(String, String)> {
    let mut all: Vec<(String, String)> = entries.into_iter().chain(managed_catalog()).collect();
    all.sort();
    all
}

/// [`with_managed_catalog`] for a test that compares ARNs alone.
pub(crate) fn arns_with_managed_catalog(
    arns: impl IntoIterator<Item = String>,
) -> Vec<String> {
    let mut all: Vec<String> = arns.into_iter().chain(managed_dashboard_arns()).collect();
    all.sort();
    all
}

/// `DashboardArn`'s modelled `@pattern`, checked by hand because this crate has no
/// regex dependency. Source: the `DashboardArn` member pattern reported by
/// `smithy_query(query="operation",operation="CreateDashboard")` and OPERATIONS.md —
/// `^arn:aws[-a-z0-9]*:bcm-dashboards::[0-9]{12}:dashboard/(\*|[-a-z0-9]+)$`, length
/// 20..2048.
fn is_dashboard_arn(arn: &str) -> bool {
    fn lower_or_digit_or_dash(s: &str) -> bool {
        s.chars()
            .all(|c| c == '-' || c.is_ascii_lowercase() || c.is_ascii_digit())
    }
    let Some(rest) = arn.strip_prefix("arn:aws") else {
        return false;
    };
    let Some((partition_suffix, rest)) = rest.split_once(':') else {
        return false;
    };
    if !lower_or_digit_or_dash(partition_suffix) {
        return false;
    }
    let Some(rest) = rest.strip_prefix("bcm-dashboards::") else {
        return false;
    };
    let Some((account, resource)) = rest.split_once(':') else {
        return false;
    };
    if account.len() != 12 || !account.chars().all(|c| c.is_ascii_digit()) {
        return false;
    }
    let Some(id) = resource.strip_prefix("dashboard/") else {
        return false;
    };
    let id_ok = id == "*" || (!id.is_empty() && lower_or_digit_or_dash(id));
    id_ok && (20..=2048).contains(&arn.len())
}

/// `WidgetId`'s modelled `@pattern` `^[0-9a-f]{32}$` (source: `smithy_query
/// shape=Widget`).
fn is_widget_id(id: &str) -> bool {
    id.len() == 32 && id.chars().all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
}

/// A widget config whose data comes from a cost-and-usage query and is displayed as a
/// graph. `metric` is a `MetricName` value VALID FOR A COST-AND-USAGE QUERY — the model's
/// note on `CostAndUsageQuery$metrics` admits AmortizedCost, BlendedCost, NetAmortizedCost,
/// NetUnblendedCost, NormalizedUsageAmount, UnblendedCost and UsageQuantity, and a
/// dashboard write refuses any other — and `visual` a `VisualType` value (sources: the
/// `CostAndUsageQuery` / `VisualType` / `Granularity` shapes in the model).
pub(crate) fn graph_config(metric: &str, visual: &str, granularity: &str) -> Value {
    json!({
        "queryParameters": {
            "costAndUsage": {
                "metrics": [metric],
                "timeRange": {
                    "startTime": {"type": "RELATIVE", "value": "-P3M"},
                    "endTime": {"type": "RELATIVE", "value": "P0D"}
                },
                "granularity": granularity
            }
        },
        "displayConfig": {"graph": {metric: {"visualType": visual}}}
    })
}

/// A widget config displayed as a table, over an absolute time range. `metric` is a
/// `MetricName` valid for the cost-and-usage query this builds, as [`graph_config`] states.
fn table_config(metric: &str) -> Value {
    json!({
        "queryParameters": {
            "costAndUsage": {
                "metrics": [metric],
                "timeRange": {
                    "startTime": {"type": "ABSOLUTE", "value": "2025-01-01"},
                    "endTime": {"type": "ABSOLUTE", "value": "2025-07-01"}
                },
                "granularity": "DAILY"
            }
        },
        "displayConfig": {"table": {}}
    })
}

/// What a widget must look like once stored, for one request widget.
struct WidgetExpectation {
    /// `Some(id)` when the request supplied one (it must survive), `None` when the
    /// service must mint one.
    id: Option<&'static str>,
    width: i64,
    height: i64,
    horizontal_offset: i64,
}

struct CreateCase {
    label: &'static str,
    name: &'static str,
    description: Option<&'static str>,
    widgets: Vec<Value>,
    expected: Vec<WidgetExpectation>,
    tags: Option<Value>,
}

fn create_cases() -> Vec<CreateCase> {
    vec![
        CreateCase {
            label: "no widgets, no description, no tags",
            name: "Em",
            description: None,
            widgets: vec![],
            expected: vec![],
            tags: None,
        },
        CreateCase {
            label: "one widget taking every size default",
            name: "Monthly Spend Overview",
            description: Some("Spend, by service."),
            widgets: vec![json!({
                "title": "Spend",
                "configs": [graph_config("UnblendedCost", "LINE", "MONTHLY")]
            })],
            // Widget defaults from the model: width 4, height 7, horizontalOffset 0.
            expected: vec![WidgetExpectation {
                id: None,
                width: 4,
                height: 7,
                horizontal_offset: 0,
            }],
            tags: Some(json!([{"key": "team", "value": "finops"}])),
        },
        CreateCase {
            label: "client-supplied widget id, explicit sizes, two configs",
            name: "Coverage-and-Utilization",
            description: Some("Two configs on one widget."),
            widgets: vec![json!({
                "id": "0123456789abcdef0123456789abcdef",
                "title": "Coverage",
                "description": "Savings plans coverage",
                "width": 6,
                "height": 10,
                "horizontalOffset": 3,
                "configs": [
                    graph_config("NormalizedUsageAmount", "STACK", "DAILY"),
                    table_config("AmortizedCost")
                ]
            })],
            expected: vec![WidgetExpectation {
                id: Some("0123456789abcdef0123456789abcdef"),
                width: 6,
                height: 10,
                horizontal_offset: 3,
            }],
            tags: Some(json!([
                {"key": "team", "value": "finops"},
                {"key": "empty-value-is-legal", "value": ""}
            ])),
        },
        CreateCase {
            label: "three widgets: supplied id, minted id, minimum sizes",
            name: "A_b-C 1",
            description: None,
            widgets: vec![
                json!({
                    "id": "ffffffffffffffffffffffffffffffff",
                    "title": "Hourly",
                    "width": 2,
                    "height": 4,
                    "configs": [graph_config("BlendedCost", "BAR", "HOURLY")]
                }),
                json!({
                    "title": "Usage-Quantity",
                    "configs": [table_config("UsageQuantity")]
                }),
                json!({
                    "title": "Net_Amortized",
                    "horizontalOffset": 5,
                    "configs": [graph_config("NetAmortizedCost", "LINE", "MONTHLY")]
                }),
            ],
            expected: vec![
                WidgetExpectation {
                    id: Some("ffffffffffffffffffffffffffffffff"),
                    width: 2,
                    height: 4,
                    horizontal_offset: 0,
                },
                WidgetExpectation {
                    id: None,
                    width: 4,
                    height: 7,
                    horizontal_offset: 0,
                },
                WidgetExpectation {
                    id: None,
                    width: 4,
                    height: 7,
                    horizontal_offset: 5,
                },
            ],
            tags: None,
        },
    ]
}

fn create_request(case: &CreateCase) -> Value {
    let mut request = json!({"name": case.name, "widgets": case.widgets});
    if let Some(description) = case.description {
        request["description"] = json!(description);
    }
    if let Some(tags) = &case.tags {
        request["resourceTags"] = tags.clone();
    }
    request
}

/// Assert `widgets` (as read back) matches `case`, and return the ids it carries.
fn assert_widgets(case: &CreateCase, widgets: &Value, request_widgets: &[Value]) -> Vec<String> {
    let widgets = widgets
        .as_array()
        .unwrap_or_else(|| panic!("case {}: widgets is not an array: {widgets}", case.label));
    assert_eq!(
        widgets.len(),
        request_widgets.len(),
        "case {}: expected {} widgets, got {widgets:?}",
        case.label,
        request_widgets.len()
    );

    let mut ids = Vec::new();
    for (index, (stored, expectation)) in widgets.iter().zip(case.expected.iter()).enumerate() {
        let requested = &request_widgets[index];
        let label = format!("case {} widget {index}", case.label);

        let id = stored
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("{label}: no id was stored: {stored}"));
        assert!(
            is_widget_id(id),
            "{label}: expected a 32-hex-character widget id, got {id:?}"
        );
        if let Some(supplied) = expectation.id {
            assert_eq!(
                id, supplied,
                "{label}: the supplied widget id must survive, got {id:?}"
            );
        }
        ids.push(id.to_string());

        assert_eq!(
            stored.get("title"),
            requested.get("title"),
            "{label}: title must be the stored one, got {stored}"
        );
        assert_eq!(
            stored.get("description"),
            requested.get("description"),
            "{label}: description must be the stored one, got {stored}"
        );
        assert_eq!(
            stored.get("width").and_then(Value::as_i64),
            Some(expectation.width),
            "{label}: expected width {}, got {stored}",
            expectation.width
        );
        assert_eq!(
            stored.get("height").and_then(Value::as_i64),
            Some(expectation.height),
            "{label}: expected height {}, got {stored}",
            expectation.height
        );
        assert_eq!(
            stored.get("horizontalOffset").and_then(Value::as_i64),
            Some(expectation.horizontal_offset),
            "{label}: expected horizontalOffset {}, got {stored}",
            expectation.horizontal_offset
        );
        assert_eq!(
            stored.get("configs"),
            requested.get("configs"),
            "{label}: configs must be stored verbatim, got {stored}"
        );
    }
    ids
}

pub(crate) fn timestamp(body: &Value, member: &str, label: &str) -> f64 {
    body.get(member)
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("{label}: {member} is not an epoch-seconds number: {body}"))
}

/// CreateDashboard mints and stores a dashboard, and GetDashboard reads that record
/// back — across a table of dashboards differing in widget count, widget ids,
/// widget sizes, config count, display type, description and tags.
#[tokio::test]
async fn create_then_get_reads_back_the_stored_dashboard() {
    let router = router();
    let mut seen_arns: BTreeSet<String> = BTreeSet::new();
    let mut seen_widget_ids: BTreeSet<String> = BTreeSet::new();

    for case in create_cases() {
        let request = create_request(&case);
        let created = ok_call(&router, "CreateDashboard", &request).await;
        let arn = created
            .get("arn")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {}: CreateDashboard returned no arn: {created}", case.label))
            .to_string();
        assert!(
            is_dashboard_arn(&arn),
            "case {}: {arn:?} is not a DashboardArn",
            case.label
        );
        assert!(
            seen_arns.insert(arn.clone()),
            "case {}: CreateDashboard reused the arn {arn}",
            case.label
        );

        let fetched = ok_call(&router, "GetDashboard", &json!({"arn": arn})).await;
        assert_eq!(
            fetched.get("arn").and_then(Value::as_str),
            Some(arn.as_str()),
            "case {}: GetDashboard must report the stored arn: {fetched}",
            case.label
        );
        assert_eq!(
            fetched.get("name").and_then(Value::as_str),
            Some(case.name),
            "case {}: GetDashboard must report the stored name: {fetched}",
            case.label
        );
        assert_eq!(
            fetched.get("description").and_then(Value::as_str),
            case.description,
            "case {}: GetDashboard must report the stored description: {fetched}",
            case.label
        );
        assert_eq!(
            fetched.get("type").and_then(Value::as_str),
            Some("CUSTOM"),
            "case {}: a dashboard created through this API is CUSTOM: {fetched}",
            case.label
        );

        let created_at = timestamp(&fetched, "createdAt", case.label);
        let updated_at = timestamp(&fetched, "updatedAt", case.label);
        assert_eq!(
            created_at, updated_at,
            "case {}: a dashboard that was never updated has createdAt == updatedAt",
            case.label
        );
        assert!(
            created_at > 1_700_000_000.0,
            "case {}: createdAt {created_at} is not a plausible epoch-seconds instant",
            case.label
        );

        let widgets = fetched
            .get("widgets")
            .unwrap_or_else(|| panic!("case {}: GetDashboard returned no widgets: {fetched}", case.label));
        let ids = assert_widgets(&case, widgets, &case.widgets);
        for id in ids {
            assert!(
                seen_widget_ids.insert(id.clone()),
                "case {}: widget id {id} was minted twice",
                case.label
            );
        }

        // A read must not change what the next caller sees.
        let again = ok_call(&router, "GetDashboard", &json!({"arn": arn})).await;
        assert_eq!(
            again, fetched,
            "case {}: GetDashboard is a read: two calls must answer identically",
            case.label
        );
    }
}

/// A dashboard's name identifies it within the account: a create naming a dashboard the
/// account already holds is refused and stores nothing. See
/// `tests/all/dashboard_name_uniqueness.rs` for the rule over a table of inputs.
#[tokio::test]
async fn a_second_create_under_a_stored_name_is_refused() {
    let router = router();
    let request = json!({
        "name": "Duplicate Name",
        "widgets": [{"title": "Spend", "configs": [table_config("BlendedCost")]}]
    });

    let first = ok_call(&router, "CreateDashboard", &request).await;
    let first_arn = first["arn"].as_str().expect("an arn").to_string();

    let (status, body) = call(&router, "CreateDashboard", &request).await;
    assert_eq!(
        status, 400,
        "a second create under a stored name must be refused, got {status}: {body}"
    );
    assert!(
        error_code(&body).contains("ValidationException"),
        "expected ValidationException, got {}",
        error_code(&body)
    );

    let listed = ok_call(&router, "ListDashboards", &json!({})).await;
    let arns: Vec<String> = listed["dashboards"]
        .as_array()
        .expect("dashboards")
        .iter()
        .map(|item| item["arn"].as_str().unwrap_or("<none>").to_string())
        .collect();
    assert_eq!(
        arns,
        arns_with_managed_catalog([first_arn.clone()]),
        "the refused create must have stored nothing: {listed}"
    );
}

/// Widget `width` and `height` are refused outside their modelled ranges and accepted
/// on each published bound. Source: `smithy_query shape=Widget` — width range 2..6,
/// height range 4..10.
#[tokio::test]
async fn widget_sizes_are_held_to_their_modelled_range() {
    let router = router();
    // (label, width, height, accepted)
    let cases: &[(&str, i64, i64, bool)] = &[
        ("width at min", 2, 7, true),
        ("width at max", 6, 7, true),
        ("width below min", 1, 7, false),
        ("width above max", 7, 7, false),
        ("height at min", 4, 4, true),
        ("height at max", 4, 10, true),
        ("height below min", 4, 3, false),
        ("height above max", 4, 11, false),
    ];

    for (index, (label, width, height, accepted)) in cases.iter().enumerate() {
        // A distinct name per row: a dashboard's name is unique within the account
        // (pinned by `dashboard_name_uniqueness`), and this row is about widget sizes.
        let request = json!({
            "name": format!("Sized Widgets {index}"),
            "widgets": [{
                "title": "Spend",
                "width": width,
                "height": height,
                "configs": [table_config("BlendedCost")]
            }]
        });
        let (status, body) = call(&router, "CreateDashboard", &request).await;
        if *accepted {
            assert_eq!(
                status, 200,
                "case {label}: width {width}/height {height} is inside the modelled range, got {status}: {body}"
            );
            let arn = body["arn"].as_str().expect("an arn");
            let fetched = ok_call(&router, "GetDashboard", &json!({"arn": arn})).await;
            assert_eq!(
                fetched["widgets"][0]["width"].as_i64(),
                Some(*width),
                "case {label}: the stored width must be the supplied one: {fetched}"
            );
            assert_eq!(
                fetched["widgets"][0]["height"].as_i64(),
                Some(*height),
                "case {label}: the stored height must be the supplied one: {fetched}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {label}: width {width}/height {height} is outside the modelled range, got {status}: {body}"
            );
            assert!(
                error_code(&body).contains("ValidationException"),
                "case {label}: expected ValidationException, got {}",
                error_code(&body)
            );
        }
    }
}

/// ListDashboards pages for real: every page holds at most `maxResults` references
/// projected from the stored records, `nextToken` appears exactly when dashboards
/// remain, and resuming from it walks the whole set exactly once — for every page
/// size, and for the default page size when `maxResults` is absent.
#[tokio::test]
async fn list_dashboards_pages_over_the_stored_dashboards() {
    let router = router();
    let names = [
        "Alpha Costs",
        "Bravo Costs",
        "Charlie Costs",
        "Delta Costs",
        "Echo Costs",
    ];
    let mut expected: Vec<String> = Vec::new();
    for name in names {
        let created = ok_call(
            &router,
            "CreateDashboard",
            &json!({
                "name": name,
                "widgets": [{"title": "Spend", "configs": [table_config("BlendedCost")]}]
            }),
        )
        .await;
        expected.push(created["arn"].as_str().expect("an arn").to_string());
    }
    // The store is keyed by ARN, so the listing order is ARN order — over the account's
    // own dashboards AND the AWS-managed catalog a fresh store already holds, which pages
    // by exactly the same rule.
    let expected = arns_with_managed_catalog(expected);
    let stored = expected.len();

    for page_size in [1_usize, 2, 3, 5, 100] {
        let mut collected: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0_usize;
        loop {
            let mut request = json!({"maxResults": page_size});
            if let Some(token) = &token {
                request["nextToken"] = json!(token);
            }
            let page = ok_call(&router, "ListDashboards", &request).await;
            pages += 1;
            let items = page["dashboards"]
                .as_array()
                .unwrap_or_else(|| panic!("case page size {page_size}: no dashboards: {page}"))
                .clone();
            assert!(
                items.len() <= page_size,
                "case page size {page_size}: a page returned {} items",
                items.len()
            );
            for item in &items {
                let arn = item["arn"].as_str().expect("an arn").to_string();
                // Each reference is the projection of the stored record.
                let fetched = ok_call(&router, "GetDashboard", &json!({"arn": arn})).await;
                for member in ["arn", "name", "type", "createdAt", "updatedAt"] {
                    assert_eq!(
                        item.get(member),
                        fetched.get(member),
                        "case page size {page_size}: reference member {member} must match the stored record: {item} vs {fetched}"
                    );
                }
                collected.push(arn);
            }
            token = page
                .get("nextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            if token.is_none() {
                break;
            }
            assert!(
                pages <= stored,
                "case page size {page_size}: pagination did not terminate"
            );
        }

        assert_eq!(
            collected, expected,
            "case page size {page_size}: paging must walk every dashboard exactly once, in order"
        );
        let want_pages = stored.div_ceil(page_size);
        assert_eq!(
            pages, want_pages,
            "case page size {page_size}: expected {want_pages} pages, walked {pages}"
        );
    }

    // `maxResults` absent: the modelled default (20) covers every stored dashboard, so no
    // cursor.
    let defaulted = ok_call(&router, "ListDashboards", &json!({})).await;
    let items = defaulted["dashboards"].as_array().expect("dashboards");
    assert_eq!(
        items.len(),
        stored,
        "case default page size: expected every dashboard on one page: {defaulted}"
    );
    assert!(
        defaulted.get("nextToken").is_none(),
        "case default page size: the last page carries no nextToken: {defaulted}"
    );
}

/// `maxResults` outside its modelled 1..100 range, and a `nextToken` this service
/// never minted, are both refused with `ValidationException`.
#[tokio::test]
async fn list_dashboards_refuses_a_bad_page_request() {
    let router = router();
    ok_call(
        &router,
        "CreateDashboard",
        &json!({"name": "Only One", "widgets": []}),
    )
    .await;

    // Source: `maxResults` range 1..100 — each published bound, and one past it.
    let max_results_cases: &[(&str, i64, bool)] = &[
        ("at min", 1, true),
        ("at max", 100, true),
        ("below min", 0, false),
        ("above max", 101, false),
        ("negative", -1, false),
    ];
    for (label, max_results, accepted) in max_results_cases {
        let (status, body) = call(
            &router,
            "ListDashboards",
            &json!({"maxResults": max_results}),
        )
        .await;
        if *accepted {
            assert_eq!(
                status, 200,
                "case maxResults {label}: {max_results} is inside 1..100, got {status}: {body}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case maxResults {label}: {max_results} is outside 1..100, got {status}: {body}"
            );
            assert!(
                error_code(&body).contains("ValidationException"),
                "case maxResults {label}: expected ValidationException, got {}",
                error_code(&body)
            );
        }
    }

    for token in [
        "not-a-token",
        "v1:",
        "v1:zzzz",
        "v1:6e6f",
        "deadbeef",
        "{\"page\": 2}",
    ] {
        let (status, body) = call(&router, "ListDashboards", &json!({"nextToken": token})).await;
        assert_eq!(
            status, 400,
            "case token {token:?}: an uninterpretable nextToken must be refused, got {status}: {body}"
        );
        assert!(
            error_code(&body).contains("ValidationException"),
            "case token {token:?}: expected ValidationException, got {}",
            error_code(&body)
        );
    }
}

/// UpdateDashboard mutates the stored record in place: the members it carries are
/// replaced, an absent optional member (`description`, `widgets`) leaves what was
/// stored alone, `updatedAt` moves forward and `createdAt` does not.
#[tokio::test]
async fn update_dashboard_mutates_the_stored_record() {
    let original_widgets = json!([
        {"title": "Spend", "configs": [table_config("BlendedCost")]},
        {"title": "Coverage", "configs": [graph_config("UnblendedCost", "BAR", "DAILY")]}
    ]);

    // (label, the update's extra members beyond `arn`+`name`, expected widget titles)
    let cases: Vec<(&str, Value, Vec<&str>)> = vec![
        (
            "name only, widgets absent",
            json!({}),
            vec!["Spend", "Coverage"],
        ),
        (
            "name and description",
            json!({"description": "Now with a description"}),
            vec!["Spend", "Coverage"],
        ),
        (
            "widgets replaced wholesale by one",
            json!({"widgets": [{"title": "Only-One", "configs": [table_config("AmortizedCost")]}]}),
            vec!["Only-One"],
        ),
        (
            "widgets replaced by none",
            json!({"widgets": []}),
            vec![],
        ),
        (
            "widgets replaced by three",
            json!({"widgets": [
                {"title": "One", "width": 2, "configs": [table_config("BlendedCost")]},
                {"title": "Two", "height": 10, "configs": [table_config("NetAmortizedCost")]},
                {"title": "Three", "configs": [graph_config("NormalizedUsageAmount", "STACK", "HOURLY")]}
            ]}),
            vec!["One", "Two", "Three"],
        ),
    ];

    for (label, extra, expected_titles) in cases {
        let router = router();
        let created = ok_call(
            &router,
            "CreateDashboard",
            &json!({
                "name": "Before Update",
                "description": "The original description",
                "widgets": original_widgets
            }),
        )
        .await;
        let arn = created["arn"].as_str().expect("an arn").to_string();
        let before = ok_call(&router, "GetDashboard", &json!({"arn": arn})).await;

        // The clock must have moved on, so `updatedAt` is comparable.
        tokio::time::sleep(Duration::from_millis(20)).await;

        let mut request = json!({"arn": arn, "name": "After Update"});
        if let Value::Object(members) = &extra {
            for (member, value) in members {
                request[member] = value.clone();
            }
        }
        let updated = ok_call(&router, "UpdateDashboard", &request).await;
        assert_eq!(
            updated["arn"].as_str(),
            Some(arn.as_str()),
            "case {label}: UpdateDashboard must answer the arn it updated: {updated}"
        );

        let after = ok_call(&router, "GetDashboard", &json!({"arn": arn})).await;
        assert_eq!(
            after["name"].as_str(),
            Some("After Update"),
            "case {label}: the new name must be stored: {after}"
        );
        // An update that omits `description` leaves the stored one in place: this
        // service unsets an optional member only through an explicit clear flag, and
        // UpdateDashboard models none.
        let expected_description = extra
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or("The original description");
        assert_eq!(
            after.get("description").and_then(Value::as_str),
            Some(expected_description),
            "case {label}: expected description {expected_description:?}, got {after}"
        );
        let titles: Vec<&str> = after["widgets"]
            .as_array()
            .unwrap_or_else(|| panic!("case {label}: no widgets: {after}"))
            .iter()
            .map(|widget| widget["title"].as_str().unwrap_or("<none>"))
            .collect();
        assert_eq!(
            titles, expected_titles,
            "case {label}: expected widgets {expected_titles:?}, got {after}"
        );
        for widget in after["widgets"].as_array().expect("widgets") {
            let id = widget["id"].as_str().unwrap_or("<none>");
            assert!(
                is_widget_id(id),
                "case {label}: every stored widget carries a 32-hex id, got {id:?}"
            );
        }

        assert_eq!(
            timestamp(&after, "createdAt", label),
            timestamp(&before, "createdAt", label),
            "case {label}: createdAt must not move on update"
        );
        assert!(
            timestamp(&after, "updatedAt", label) > timestamp(&before, "updatedAt", label),
            "case {label}: updatedAt must move forward on update: {before} -> {after}"
        );

        // And the listing sees the update too.
        let listed = ok_call(&router, "ListDashboards", &json!({})).await;
        assert_eq!(
            listed["dashboards"][0]["name"].as_str(),
            Some("After Update"),
            "case {label}: ListDashboards projects the updated record: {listed}"
        );
    }
}

/// DeleteDashboard removes the record: the ARN stops resolving and stops being listed.
#[tokio::test]
async fn delete_dashboard_removes_the_record() {
    let router = router();
    let mut arns = Vec::new();
    for name in ["Keep Me", "Delete Me", "Keep Me Too"] {
        let created = ok_call(
            &router,
            "CreateDashboard",
            &json!({
                "name": name,
                "widgets": [{"title": "Spend", "configs": [table_config("BlendedCost")]}],
                "resourceTags": [{"key": "team", "value": "finops"}]
            }),
        )
        .await;
        arns.push(created["arn"].as_str().expect("an arn").to_string());
    }
    let doomed = arns[1].clone();

    let deleted = ok_call(&router, "DeleteDashboard", &json!({"arn": doomed})).await;
    assert_eq!(
        deleted["arn"].as_str(),
        Some(doomed.as_str()),
        "DeleteDashboard must answer the arn it removed: {deleted}"
    );

    let (status, body) = call(&router, "GetDashboard", &json!({"arn": doomed})).await;
    assert_eq!(
        status, 400,
        "a deleted dashboard must not resolve, got {status}: {body}"
    );
    assert!(
        error_code(&body).contains("ResourceNotFoundException"),
        "expected ResourceNotFoundException, got {}",
        error_code(&body)
    );
    assert_eq!(
        body["resourceId"].as_str(),
        Some(doomed.as_str()),
        "the not-found answer must echo the requested arn in resourceId: {body}"
    );

    let listed = ok_call(&router, "ListDashboards", &json!({})).await;
    let remaining: Vec<String> = listed["dashboards"]
        .as_array()
        .expect("dashboards")
        .iter()
        .map(|item| item["arn"].as_str().unwrap_or("<none>").to_string())
        .collect();
    assert_eq!(
        remaining,
        arns_with_managed_catalog([arns[0].clone(), arns[2].clone()]),
        "ListDashboards must list exactly the surviving dashboards: {listed}"
    );

    // The record is gone, and this operation models no not-found error: deleting it
    // again reports the same post-condition, a 200 echoing the requested arn (the whole
    // class is covered by `tests/all/delete_dashboard_idempotency.rs`).
    let repeated = ok_call(&router, "DeleteDashboard", &json!({"arn": doomed})).await;
    assert_eq!(
        repeated["arn"].as_str(),
        Some(doomed.as_str()),
        "a repeated delete must answer the requested arn: {repeated}"
    );

    // Updating it is a not-found, which that operation does model.
    let (status, body) = call(
        &router,
        "UpdateDashboard",
        &json!({"arn": doomed, "name": "Back From The Dead"}),
    )
    .await;
    assert_eq!(
        status, 400,
        "updating an absent dashboard is a not-found, got {status}: {body}"
    );
    assert!(
        error_code(&body).contains("ResourceNotFoundException"),
        "expected ResourceNotFoundException, got {}",
        error_code(&body)
    );
}

/// An ARN that names no dashboard is a not-found for every operation that has to resolve
/// one (`DeleteDashboard`, which models no not-found error, is the exception — see
/// `tests/all/delete_dashboard_idempotency.rs`); an ARN that is not a `DashboardArn`
/// at all is a validation failure. Both answer HTTP 400 (see
/// `tests/all/resource_not_found.rs` for why a not-found is 400 here), so the two are
/// told apart by the wire error code, which is what a client branches on.
#[tokio::test]
async fn unknown_and_malformed_arns_are_told_apart() {
    let router = router();

    // Well-formed (it satisfies the modelled pattern) but never created.
    let never_created = "arn:aws:bcm-dashboards::000000000000:dashboard/does-not-exist";
    let (status, body) = call(&router, "GetDashboard", &json!({"arn": never_created})).await;
    assert_eq!(
        status, 400,
        "case well-formed but unknown: expected 400, got {status}: {body}"
    );
    assert!(
        error_code(&body).contains("ResourceNotFoundException"),
        "case well-formed but unknown: expected ResourceNotFoundException, got {}",
        error_code(&body)
    );

    // Each row breaks the `DashboardArn` pattern or its length in a different way.
    let malformed: &[(&str, &str)] = &[
        ("not an arn at all", "not-an-arn"),
        (
            "another service",
            "arn:aws:bcm-dataexports::000000000000:dashboard/abc",
        ),
        (
            "a region where the model allows none",
            "arn:aws:bcm-dashboards:us-east-1:000000000000:dashboard/abc",
        ),
        (
            "an account id that is not 12 digits",
            "arn:aws:bcm-dashboards::12345:dashboard/abc",
        ),
        (
            "an uppercase resource id",
            "arn:aws:bcm-dashboards::000000000000:dashboard/ABC",
        ),
        (
            "the wrong resource type",
            "arn:aws:bcm-dashboards::000000000000:report/abc",
        ),
        ("an empty arn", ""),
    ];
    for (label, arn) in malformed {
        assert!(
            !is_dashboard_arn(arn),
            "case {label}: the test's own ARN check disagrees that {arn:?} is malformed"
        );
        let (status, body) = call(&router, "GetDashboard", &json!({"arn": arn})).await;
        assert_eq!(
            status, 400,
            "case {label}: {arn:?} must be refused as invalid, got {status}: {body}"
        );
        assert!(
            error_code(&body).contains("ValidationException"),
            "case {label}: expected ValidationException, got {}",
            error_code(&body)
        );
    }
}

/// The store a caller can grow is bounded: at the dashboard quota CreateDashboard
/// answers the modelled `ServiceQuotaExceededException` (402), the dashboards already
/// stored are untouched, and deleting one makes room for exactly one more.
#[tokio::test]
async fn create_dashboard_stops_at_the_dashboard_quota() {
    let router = router();
    // A distinct name per create: a dashboard's name is unique within the account, so
    // the quota — not a name collision — has to be what refuses the create past the
    // ceiling. `DashboardName` is 2..50 characters matching
    // `^(?!.* {2})[a-zA-Z][a-zA-Z0-9 _-]{0,48}[a-zA-Z0-9_-]$`, which every row below is.
    let filler = |index: usize| json!({"name": format!("Quota Filler {index:04}"), "widgets": []});

    let mut first_arn = None;
    // The account's own creates fill the store to the quota ALONGSIDE the AWS-managed
    // catalog it already holds, which counts against the same ceiling.
    let room = MAX_DASHBOARDS - managed_dashboard_count();
    for index in 0..room {
        let created = ok_call(&router, "CreateDashboard", &filler(index)).await;
        let arn = created["arn"].as_str().expect("an arn").to_string();
        if index == 0 {
            first_arn = Some(arn);
        }
    }
    let first_arn = first_arn.expect("the first dashboard's arn");

    let (status, body) = call(&router, "CreateDashboard", &filler(room)).await;
    assert_eq!(
        status, 402,
        "case at the quota: CreateDashboard must refuse past {MAX_DASHBOARDS} dashboards, got {status}: {body}"
    );
    assert!(
        error_code(&body).contains("ServiceQuotaExceededException"),
        "case at the quota: expected ServiceQuotaExceededException, got {}",
        error_code(&body)
    );

    // The refusal stored nothing and removed nothing.
    let page = ok_call(&router, "ListDashboards", &json!({"maxResults": 100})).await;
    assert_eq!(
        page["dashboards"].as_array().expect("dashboards").len(),
        100,
        "case after the refusal: the stored dashboards must be untouched"
    );
    ok_call(&router, "GetDashboard", &json!({"arn": first_arn})).await;

    // Freeing one record makes room for exactly one more.
    ok_call(&router, "DeleteDashboard", &json!({"arn": first_arn})).await;
    ok_call(&router, "CreateDashboard", &filler(room + 1)).await;
    let (status, body) = call(&router, "CreateDashboard", &filler(room + 2)).await;
    assert_eq!(
        status, 402,
        "case back at the quota: CreateDashboard must refuse again, got {status}: {body}"
    );
}

// ---------------------------------------------------------------------------
// The QUERY a widget runs: every member of the `QueryParameters` union.
//
// RULE: a widget configuration carries exactly one `QueryParameters` member, and a write
// stores the member and the query the request submitted — so a read answers that member
// with the values it was given and no other member set. The union's members are the five
// the model snapshot declares plus the two AWS answers the published catalog's panels with
// (`budget`, `costEfficiency`), which this crate declares from the overlay model beside the
// snapshot (`smithy-model/LOCAL-ADDITIONS.md`).
// ---------------------------------------------------------------------------

/// The published model snapshot this crate is generated from, embedded at compile time so
/// the local declaration is checked against the model rather than against a copy of it.
const SNAPSHOT: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/smithy-model/com-amazonaws-bcmdashboards-awsbcmdashboardsservice.json"
));

/// The overlay model the local query declarations are generated from, embedded for the same
/// reason: what it restates of the snapshot must still match it.
const OVERLAY: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/smithy-model/local-additions.json"
));

/// The union whose members a widget configuration chooses between.
const UNION_SHAPE: &str = "com.amazonaws.bcmdashboards#QueryParameters";

/// The union members this crate declares locally, which the snapshot does not.
const LOCAL_MEMBERS: [&str; 2] = ["budget", "costEfficiency"];

/// The shapes the overlay adds for those members, which the snapshot declares nowhere.
const LOCAL_SHAPES: [&str; 2] = [
    "com.amazonaws.bcmdashboards#BudgetQuery",
    "com.amazonaws.bcmdashboards#CostEfficiencyQuery",
];

/// Every member of the `QueryParameters` union, so a stored configuration can be held to
/// carrying exactly one of them.
const QUERY_MEMBERS: [&str; 7] = [
    "costAndUsage",
    "savingsPlansCoverage",
    "savingsPlansUtilization",
    "reservationCoverage",
    "reservationUtilization",
    "budget",
    "costEfficiency",
];

/// One row of the query table: the member the request submits, the query it carries and the
/// display configuration beside it.
struct QueryCase {
    label: &'static str,
    member: &'static str,
    query: Value,
    display: Value,
}

/// A relative `DateTimeRange`, both bounds ISO 8601 durations.
fn relative_range(start: &str, end: &str) -> Value {
    json!({
        "startTime": {"type": "RELATIVE", "value": start},
        "endTime": {"type": "RELATIVE", "value": end}
    })
}

/// A grouping by one `Dimension`, with the `GroupDefinitionType` the member defaults to
/// stated explicitly so a row submits exactly what it reads back.
fn group_by(dimension: &str) -> Value {
    json!([{"key": dimension, "type": "DIMENSION"}])
}

/// A filter narrowing a query to the values of one `Dimension`.
fn dimension_filter(dimension: &str, values: &[&str]) -> Value {
    json!({"dimensions": {"key": dimension, "values": values}})
}

/// The rows every case below runs: each member of the union at least once, and the two
/// locally declared ones twice — once carrying every member of the query they model, once
/// carrying the fewest the model requires.
fn query_cases() -> Vec<QueryCase> {
    vec![
        QueryCase {
            label: "a cost-and-usage query",
            member: "costAndUsage",
            query: json!({
                "metrics": ["UnblendedCost"],
                "timeRange": relative_range("-P12M", "P0M"),
                "granularity": "MONTHLY",
                "groupBy": group_by("SERVICE")
            }),
            display: json!({"graph": {"UnblendedCost": {"visualType": "STACK"}}}),
        },
        QueryCase {
            label: "a Savings Plans coverage query",
            member: "savingsPlansCoverage",
            query: json!({
                "timeRange": relative_range("-P6M", "P0M"),
                "metrics": ["SpendCoveredBySavingsPlans"],
                "granularity": "DAILY"
            }),
            display: json!({"table": {}}),
        },
        QueryCase {
            label: "a Savings Plans utilization query",
            member: "savingsPlansUtilization",
            query: json!({
                "timeRange": relative_range("-P3M", "P0M"),
                "granularity": "MONTHLY"
            }),
            display: json!({"table": {}}),
        },
        QueryCase {
            label: "a reservation coverage query",
            member: "reservationCoverage",
            query: json!({
                "timeRange": relative_range("-P12M", "P0M"),
                "granularity": "MONTHLY",
                "groupBy": group_by("INSTANCE_TYPE"),
                "metrics": ["Hour", "Cost"]
            }),
            display: json!({"table": {}}),
        },
        QueryCase {
            label: "a reservation utilization query",
            member: "reservationUtilization",
            query: json!({
                "timeRange": relative_range("-P12M", "P0M"),
                "granularity": "MONTHLY",
                "groupBy": group_by("REGION")
            }),
            display: json!({"table": {}}),
        },
        QueryCase {
            // The locally declared member the published budget-health panel carries: it
            // models no time period, no granularity and no metrics, so a query over every
            // budget the account holds carries NO member at all.
            label: "a budget query over every budget the account holds",
            member: "budget",
            query: json!({}),
            display: json!({"table": {}}),
        },
        QueryCase {
            label: "a budget query narrowed to one dimension",
            member: "budget",
            query: json!({"filter": dimension_filter("SERVICE", ["Amazon Simple Storage Service"].as_slice())}),
            display: json!({"table": {}}),
        },
        QueryCase {
            // The other locally declared member: the series it draws is a score rather
            // than a `MetricName`, which is why it models no `metrics` member.
            label: "a cost-efficiency query, grouped and at a granularity",
            member: "costEfficiency",
            query: json!({
                "timeRange": relative_range("-P3M", "P0M"),
                "granularity": "MONTHLY",
                "groupBy": group_by("LINKED_ACCOUNT")
            }),
            display: json!({"graph": {"EfficiencyScore": {"visualType": "LINE"}}}),
        },
        QueryCase {
            label: "a cost-efficiency query carrying only the time period it requires",
            member: "costEfficiency",
            query: json!({"timeRange": relative_range("-P1M", "P0M")}),
            display: json!({"table": {}}),
        },
    ]
}

/// The one stored configuration of the one stored widget of `dashboard`.
fn only_config(label: &str, dashboard: &Value) -> Value {
    let widgets = dashboard["widgets"]
        .as_array()
        .unwrap_or_else(|| panic!("case {label}: widgets is @required, got {dashboard}"));
    assert_eq!(
        widgets.len(),
        1,
        "case {label}: expected the one submitted widget, got {dashboard}"
    );
    let configs = widgets[0]["configs"]
        .as_array()
        .unwrap_or_else(|| panic!("case {label}: configs is @required, got {dashboard}"));
    assert_eq!(
        configs.len(),
        1,
        "case {label}: expected the one submitted configuration, got {dashboard}"
    );
    configs[0].clone()
}

/// Assert that `config` carries `case`'s query under `case`'s member and under no other.
fn assert_carries_only(case: &QueryCase, site: &str, config: &Value) {
    let label = case.label;
    let parameters = &config["queryParameters"];
    assert_eq!(
        &parameters[case.member], &case.query,
        "case {label}: {site} must carry the submitted {} query, got {config}",
        case.member
    );
    for member in QUERY_MEMBERS {
        if member == case.member {
            continue;
        }
        assert!(
            parameters[member].is_null(),
            "case {label}: {site} carries one union member, but {member} is set too: {config}"
        );
    }
    assert_eq!(
        &config["displayConfig"], &case.display,
        "case {label}: {site} must carry the submitted display configuration, got {config}"
    );
}

/// Every member of the union round-trips: a create stores the submitted query, a read
/// answers it with no other member set, and an update replacing the widget list stores the
/// query it submits in turn.
#[tokio::test]
async fn every_query_parameters_member_round_trips() {
    let router = router();

    for (index, case) in query_cases().into_iter().enumerate() {
        let name = format!("Query Member {index:02}");
        let member = case.member;
        let query = case.query.clone();
        let display = case.display.clone();
        let widgets = json!([{
            "title": "Spend",
            "configs": [{"queryParameters": {member: query}, "displayConfig": display}]
        }]);
        let created = ok_call(
            &router,
            "CreateDashboard",
            &json!({"name": name, "widgets": widgets}),
        )
        .await;
        let arn = created["arn"]
            .as_str()
            .unwrap_or_else(|| {
                panic!(
                    "case {}: CreateDashboard answered no arn: {created}",
                    case.label
                )
            })
            .to_string();

        let fetched = ok_call(&router, "GetDashboard", &json!({"arn": arn})).await;
        assert_carries_only(&case, "the stored dashboard", &only_config(case.label, &fetched));

        // The same query submitted on the update path, and read back again: a second read
        // of the same record must answer the same query.
        ok_call(
            &router,
            "UpdateDashboard",
            &json!({"arn": arn, "name": name, "widgets": widgets}),
        )
        .await;
        let updated = ok_call(&router, "GetDashboard", &json!({"arn": arn})).await;
        assert_carries_only(
            &case,
            "the updated dashboard",
            &only_config(case.label, &updated),
        );
    }
}

/// A caller may replace the query of a stored widget with ANY other member of the union,
/// including the two declared locally: the stored configuration is the one last submitted,
/// never a merge of the two.
#[tokio::test]
async fn an_update_replaces_the_query_member_it_was_given() {
    let router = router();
    let created = ok_call(
        &router,
        "CreateDashboard",
        &json!({
            "name": "Query Member Replacement",
            "widgets": [{"title": "Spend", "configs": [graph_config("UnblendedCost", "LINE", "MONTHLY")]}]
        }),
    )
    .await;
    let arn = created["arn"].as_str().expect("an arn").to_string();

    for case in query_cases() {
        let member = case.member;
        let query = case.query.clone();
        let display = case.display.clone();
        ok_call(
            &router,
            "UpdateDashboard",
            &json!({
                "arn": arn,
                "name": "Query Member Replacement",
                "widgets": [{
                    "title": "Spend",
                    "configs": [{
                        "queryParameters": {member: query},
                        "displayConfig": display
                    }]
                }]
            }),
        )
        .await;
        let fetched = ok_call(&router, "GetDashboard", &json!({"arn": arn})).await;
        assert_carries_only(
            &case,
            "the replaced configuration",
            &only_config(case.label, &fetched),
        );
    }
}

/// The `shapes` object of one model document.
fn model_shapes(model: &str) -> Value {
    let parsed: Value = serde_json::from_str(model).expect("a model document must be JSON");
    Value::Object(
        parsed["shapes"]
            .as_object()
            .expect("a model document must carry a shapes object")
            .clone(),
    )
}

/// `declaration` with every documentation trait dropped, recursively — and with a traits
/// object left empty by that dropped too, so a member the snapshot documents and nothing
/// else compares equal to the same member restated with no traits at all.
///
/// Documentation is prose: the overlay restates what codegen READS — targets, types, enum
/// values and constraint traits — so comparing declarations has to ignore the one trait that
/// carries no behaviour.
fn without_docs(declaration: &Value) -> Value {
    match declaration {
        Value::Object(members) => Value::Object(
            members
                .iter()
                .filter(|(key, _)| key.as_str() != "smithy.api#documentation")
                .map(|(key, value)| (key.clone(), without_docs(value)))
                .filter(|(key, value)| {
                    key.as_str() != "traits"
                        || !value
                            .as_object()
                            .is_some_and(serde_json::Map::<String, Value>::is_empty)
                })
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(without_docs).collect()),
        other => other.clone(),
    }
}

/// The wire values of an `enum` shape, read the way codegen reads them: each member's
/// `@enumValue`, falling back to the member name.
fn enum_values(shape: &Value) -> Vec<String> {
    shape["members"]
        .as_object()
        .map(|members| {
            members
                .iter()
                .map(|(name, member)| {
                    member["traits"]["smithy.api#enumValue"]
                        .as_str()
                        .unwrap_or(name)
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The overlay the local query declarations are generated from restates the snapshot: every
/// shape it names is declared there with the same type, every member and enum value it
/// restates is the snapshot's own, and the only thing it adds is the two members AWS
/// carries. A refreshed snapshot that declares either of them — or that changes a target or
/// a value set the overlay restates — fails here, and the local declaration should then
/// give way to the modelled shape.
#[test]
fn the_overlay_restates_the_snapshot_but_for_the_members_it_adds() {
    let snapshot = model_shapes(SNAPSHOT);
    let overlay = model_shapes(OVERLAY);
    let restated = overlay
        .as_object()
        .expect("the overlay carries a shapes object");

    assert!(
        restated.contains_key(UNION_SHAPE),
        "the overlay must restate {UNION_SHAPE}, the shape the local declaration is of"
    );
    for shape_id in LOCAL_SHAPES {
        assert!(
            restated.contains_key(shape_id),
            "the overlay must declare {shape_id}, a local member's target"
        );
    }

    for (shape_id, declared) in restated {
        if LOCAL_SHAPES.contains(&shape_id.as_str()) {
            assert!(
                snapshot[shape_id].is_null(),
                "the snapshot now declares {shape_id}, so the local declaration should give \
                 way to the modelled shape"
            );
            continue;
        }
        let modelled = &snapshot[shape_id];
        assert!(
            !modelled.is_null(),
            "the overlay restates {shape_id}, which the snapshot no longer declares"
        );
        assert_eq!(
            declared["type"], modelled["type"],
            "{shape_id} must be restated with the type the snapshot gives it, got {}",
            declared["type"]
        );

        // An enum is restated for its VALUE SET, which is what holds a submitted value to
        // the enumerated ones; a list for its element target, which is what classifies it.
        if declared["type"] == "enum" {
            assert_eq!(
                enum_values(declared),
                enum_values(modelled),
                "{shape_id} must be restated with the value set the snapshot declares"
            );
            continue;
        }
        if declared["type"] == "list" {
            assert_eq!(
                without_docs(&declared["member"]),
                without_docs(&modelled["member"]),
                "{shape_id} must be restated with the element the snapshot declares, got {}",
                declared["member"]
            );
            continue;
        }

        let declared_members = declared["members"].as_object().cloned().unwrap_or_default();
        let modelled_members = modelled["members"].as_object().cloned().unwrap_or_default();
        for (member, declaration) in &declared_members {
            if shape_id == UNION_SHAPE && LOCAL_MEMBERS.contains(&member.as_str()) {
                let target = declaration["target"].as_str().unwrap_or("<none>");
                assert!(
                    LOCAL_SHAPES.contains(&target),
                    "the local member {member} must target a locally declared shape, got \
                     {target}"
                );
                continue;
            }
            assert_eq!(
                without_docs(declaration),
                without_docs(&modelled_members[member]),
                "member {member} of {shape_id} must be restated as the snapshot declares it, \
                 got {declaration}"
            );
        }

        let added: Vec<&str> = declared_members
            .keys()
            .filter(|member| !modelled_members.contains_key(*member))
            .map(String::as_str)
            .collect();
        if shape_id == UNION_SHAPE {
            for member in modelled_members.keys() {
                assert!(
                    declared_members.contains_key(member),
                    "the overlay must restate the modelled member {member} of {shape_id}, or \
                     this service stops serving it"
                );
            }
            assert_eq!(
                added,
                LOCAL_MEMBERS.to_vec(),
                "{shape_id} may add {LOCAL_MEMBERS:?} and nothing else, added {added:?}"
            );
        } else {
            // Restated only so the declarations above can target it: its own Rust type is
            // generated from the snapshot, so nothing of its member list is read here.
            assert!(
                added.is_empty(),
                "{shape_id} is restated for the declarations above to target, so it may add \
                 no member; added {added:?}"
            );
        }
    }
}
