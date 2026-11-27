// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Cloud Map core loop over the wire: create an HTTP
//! namespace, resolve the asynchronous operation, create a service in it, register an
//! instance, discover it, deregister it — plus the reads that prove every answer came
//! from persisted state.
//!
//! Every request goes through `MultiServiceRouter::dispatch`, i.e. the same path the
//! emulator serves in production (`awsJson1_1`: `POST /` with `X-Amz-Target`).
//!
//! The request harness below (`emulator`, `call`, `ok`, `failure`, `discovered`) is
//! `pub(crate)`, so a sibling suite in `tests/all/` drives the emulator the same way
//! instead of growing a second copy of it.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_servicediscovery_route53autonaming_v20170314 as cloudmap;
use smithy_server_runtime::MultiServiceRouter;

/// The `X-Amz-Target` prefix from the crate's `OPERATIONS.md` "Wire protocol".
const TARGET_PREFIX: &str = "Route53AutoNaming_v20170314";

/// The region the test requests are signed for; every ARN must carry it.
pub(crate) const REGION: &str = "us-west-2";

/// One emulator with this service mounted on a fresh shared engine.
///
/// `pub(crate)` because the suites in `tests/all/` share this one harness rather than
/// each carrying a copy of the dispatch plumbing.
pub(crate) fn emulator() -> MultiServiceRouter {
    emulator_with_engine().0
}

/// The same emulator, plus the engine it was mounted on, so a test can seed the store
/// to reach a boundary condition without thousands of round trips.
pub(crate) fn emulator_with_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // These tests drive the router directly rather than signing requests.
    router.set_request_authentication(false);
    router.register_service(cloudmap::EmulatedService::new(Arc::clone(&engine)));
    // The hosted zone of a DNS namespace is a zone of THIS emulator's Route 53, which a
    // DNS namespace's create and its services' records reach in process — exactly as the
    // assembled emulator mounts both services on one engine.
    router.register_service(
        service_com_amazonaws_route53_awsdnsv20130401::EmulatedService::new(Arc::clone(&engine)),
    );
    (router, engine)
}

/// Dispatch one operation and return its HTTP status and JSON body.
pub(crate) async fn call(router: &MultiServiceRouter, operation: &str, body: Value) -> (u16, Value) {
    call_signed_for(router, REGION, operation, body).await
}

/// [`call`], with the Region the request is SIGNED for chosen by the caller.
///
/// The Region reaches a handler through the signature's credential scope, so a suite whose
/// rule generalises over it — a value a response takes from the Region the call was made in
/// — drives the same dispatch through here instead of carrying a second copy of it.
pub(crate) async fn call_signed_for(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    body: Value,
) -> (u16, Value) {
    let authorization = format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEMULATOR/20260101/{region}/servicediscovery/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature=unsigned"
    );
    let request = http::Request::builder()
        .method("POST")
        .uri("http://localhost/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
        .header("authorization", authorization)
        .body(bytes::Bytes::from(body.to_string()))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let json = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).expect("an awsJson1_1 body is JSON")
    };
    (status, json)
}

/// Dispatch an operation that must succeed, returning its output.
pub(crate) async fn ok(router: &MultiServiceRouter, operation: &str, body: Value) -> Value {
    let (status, json) = call(router, operation, body).await;
    assert_eq!(status, 200, "{operation} must succeed, got {status}: {json}");
    json
}

/// Dispatch an operation that must fail, returning its status and wire error code.
pub(crate) async fn failure(
    router: &MultiServiceRouter,
    operation: &str,
    body: Value,
) -> (u16, String) {
    let (status, code, message) = refusal(router, operation, body).await;
    assert_ne!(
        status, 200,
        "{operation} must fail, got {status} {code} {message:?}"
    );
    (status, code)
}

/// Dispatch an operation and report how it was REFUSED: its status, the wire error code
/// its body names, and the text that body carries, if any.
///
/// The awsJson error envelope names the class in `__type`, qualified by its namespace, and
/// carries the text under `Message` (the member this model declares, which a handler's own
/// error body uses) or `message` (the member the framework's own renderers write), so both
/// spellings are read HERE rather than once per suite — the same reason the rest of this
/// harness is shared.
pub(crate) async fn refusal(
    router: &MultiServiceRouter,
    operation: &str,
    body: Value,
) -> (u16, String, Option<String>) {
    let (status, json) = call(router, operation, body).await;
    let code = json["__type"]
        .as_str()
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string();
    let message = json
        .get("Message")
        .or_else(|| json.get("message"))
        .and_then(Value::as_str)
        .map(str::to_string);
    (status, code, message)
}

/// Create a namespace and return `(operation id, namespace id)`.
pub(crate) async fn namespace(router: &MultiServiceRouter, name: &str) -> (String, String) {
    let created = ok(router, "CreateHttpNamespace", json!({ "Name": name })).await;
    let operation_id = created["OperationId"]
        .as_str()
        .expect("CreateHttpNamespace returns an OperationId")
        .to_string();
    let operation = ok(
        router,
        "GetOperation",
        json!({ "OperationId": operation_id }),
    )
    .await;
    let namespace_id = operation["Operation"]["Targets"]["NAMESPACE"]
        .as_str()
        .expect("the CREATE_NAMESPACE operation targets the namespace it created")
        .to_string();
    (operation_id, namespace_id)
}

/// Create a public DNS namespace — the kind whose services carry Route 53 records — and
/// return its id.
///
/// `pub(crate)` for the same reason as the helpers above: the suites that need a DNS
/// namespace share this one rather than each carrying a copy of the create-then-resolve
/// round trip.
pub(crate) async fn dns_namespace(router: &MultiServiceRouter, name: &str) -> String {
    let created = ok(
        router,
        "CreatePublicDnsNamespace",
        json!({ "Name": format!("{name}.example.com") }),
    )
    .await;
    let operation_id = created["OperationId"]
        .as_str()
        .expect("CreatePublicDnsNamespace returns an OperationId")
        .to_string();
    let filed = ok(router, "GetOperation", json!({ "OperationId": operation_id })).await;
    filed["Operation"]["Targets"]["NAMESPACE"]
        .as_str()
        .expect("the CREATE_NAMESPACE operation targets the namespace it created")
        .to_string()
}

/// Create a service in `namespace_id` and return its id.
pub(crate) async fn service(router: &MultiServiceRouter, namespace_id: &str, name: &str) -> String {
    let created = ok(
        router,
        "CreateService",
        json!({ "Name": name, "NamespaceId": namespace_id }),
    )
    .await;
    created["Service"]["Id"]
        .as_str()
        .expect("CreateService returns the service it stored")
        .to_string()
}

/// Every entry a paginated listing serves for one request body, paging it to exhaustion: the
/// `operation` is the listing to call and `member` the output member holding its page, so one
/// pager serves every listing this service models instead of one copy per suite.
///
/// The `NextToken` each page hands back is fed to the next request, and the walk is bounded
/// so a listing that failed to terminate is reported rather than looping.
pub(crate) async fn all_page_entries(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    member: &str,
    body: Value,
) -> Vec<Value> {
    let mut request = body;
    let mut entries = Vec::new();
    let mut served = 0usize;
    loop {
        let listed = ok(router, operation, request.clone()).await;
        let page = listed[member]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: {operation} answers a list, got {listed}"))
            .clone();
        entries.extend(page);
        served += 1;
        assert!(
            served <= 64,
            "case {case:?}: paging {operation} must terminate, got {served} pages"
        );
        match listed["NextToken"].as_str() {
            Some(token) => request["NextToken"] = json!(token),
            None => return entries,
        }
    }
}

/// The resource ids one set of listing entries names, each read from the entry's `Id` member.
pub(crate) fn entry_ids(case: &str, entries: &[Value]) -> BTreeSet<String> {
    entries
        .iter()
        .map(|entry| {
            entry["Id"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: every entry names its resource, got {entry}")
                })
                .to_string()
        })
        .collect()
}

/// The `InstanceId`s `DiscoverInstances` answers with for the given request body.
pub(crate) async fn discovered(router: &MultiServiceRouter, body: Value) -> BTreeSet<String> {
    let found = ok(router, "DiscoverInstances", body).await;
    found["Instances"]
        .as_array()
        .expect("Instances is a list")
        .iter()
        .map(|instance| {
            instance["InstanceId"]
                .as_str()
                .expect("every summary names its instance")
                .to_string()
        })
        .collect()
}

#[tokio::test]
async fn the_cloud_map_core_loop_reads_back_everything_it_persisted() {
    let router = emulator();
    let (operation_id, namespace_id) = namespace(&router, "bootstrap-ns").await;
    assert!(
        namespace_id.starts_with("ns-"),
        "a namespace id is minted with the ns- prefix, got {namespace_id:?}"
    );

    // The namespace record the create persisted, read back by id.
    let read = ok(&router, "GetNamespace", json!({ "Id": namespace_id })).await;
    let ns = &read["Namespace"];
    let expected_arn = format!(
        "arn:aws:servicediscovery:{REGION}:{}:namespace/{namespace_id}",
        engine::EMULATOR_ACCOUNT_ID
    );
    assert_eq!(ns["Id"], json!(namespace_id), "GetNamespace echoes the id");
    assert_eq!(ns["Name"], json!("bootstrap-ns"), "got {ns}");
    assert_eq!(ns["Type"], json!("HTTP"), "got {ns}");
    assert_eq!(ns["Arn"], json!(expected_arn), "got {ns}");
    assert_eq!(ns["ServiceCount"], json!(0), "a new namespace holds nothing");
    assert_eq!(
        ns["Properties"]["HttpProperties"]["HttpName"],
        json!("bootstrap-ns"),
        "an HTTP namespace is discovered under its HttpName, got {ns}"
    );

    // A service in it: the full Service structure, computed from the record.
    let created = ok(
        &router,
        "CreateService",
        json!({ "Name": "bootstrap-svc", "NamespaceId": namespace_id, "Description": "core loop" }),
    )
    .await;
    let svc = &created["Service"];
    let service_id = svc["Id"].as_str().expect("a service id").to_string();
    assert!(
        service_id.starts_with("srv-"),
        "a service id is minted with the srv- prefix, got {service_id:?}"
    );
    assert_eq!(svc["NamespaceId"], json!(namespace_id), "got {svc}");
    assert_eq!(svc["InstanceCount"], json!(0), "got {svc}");
    assert_eq!(svc["Description"], json!("core loop"), "got {svc}");
    assert_eq!(
        svc["Arn"],
        json!(format!(
            "arn:aws:servicediscovery:{REGION}:{}:service/{service_id}",
            engine::EMULATOR_ACCOUNT_ID
        )),
        "got {svc}"
    );
    assert_eq!(
        svc["Type"],
        json!("HTTP"),
        "a service in an HTTP namespace registers no DNS records, got {svc}"
    );

    // The owning namespace's ServiceCount tracks the write.
    let read = ok(&router, "GetNamespace", json!({ "Id": namespace_id })).await;
    assert_eq!(
        read["Namespace"]["ServiceCount"],
        json!(1),
        "creating a service increments the namespace's ServiceCount, got {read}"
    );
    let read = ok(&router, "GetService", json!({ "Id": service_id })).await;
    assert_eq!(
        read["Service"], *svc,
        "GetService must report exactly what CreateService stored"
    );

    // Register an instance; the operation id resolves to a REGISTER_INSTANCE record.
    let attributes = json!({ "AWS_INSTANCE_IPV4": "10.0.0.1", "stage": "prod" });
    let registered = ok(
        &router,
        "RegisterInstance",
        json!({
            "ServiceId": service_id,
            "InstanceId": "i-1",
            "Attributes": attributes,
        }),
    )
    .await;
    let register_operation = registered["OperationId"]
        .as_str()
        .expect("RegisterInstance returns an OperationId")
        .to_string();
    let operation = ok(
        &router,
        "GetOperation",
        json!({ "OperationId": register_operation }),
    )
    .await;
    assert_eq!(
        operation["Operation"]["Type"],
        json!("REGISTER_INSTANCE"),
        "got {operation}"
    );
    let targets = &operation["Operation"]["Targets"];
    assert_eq!(targets["NAMESPACE"], json!(namespace_id), "got {targets}");
    assert_eq!(targets["SERVICE"], json!(service_id), "got {targets}");
    assert_eq!(targets["INSTANCE"], json!("i-1"), "got {targets}");

    // The instance, read back by id and by discovery.
    let read = ok(
        &router,
        "GetInstance",
        json!({ "ServiceId": service_id, "InstanceId": "i-1" }),
    )
    .await;
    assert_eq!(
        read["Instance"]["Attributes"], attributes,
        "GetInstance must report the exact attribute map that was registered, got {read}"
    );
    let found = ok(
        &router,
        "DiscoverInstances",
        json!({ "NamespaceName": "bootstrap-ns", "ServiceName": "bootstrap-svc" }),
    )
    .await;
    let instances = found["Instances"].as_array().expect("a list");
    assert_eq!(instances.len(), 1, "got {found}");
    assert_eq!(instances[0]["InstanceId"], json!("i-1"), "got {found}");
    assert_eq!(instances[0]["Attributes"], attributes, "got {found}");
    assert_eq!(
        instances[0]["NamespaceName"],
        json!("bootstrap-ns"),
        "got {found}"
    );
    assert_eq!(
        instances[0]["ServiceName"],
        json!("bootstrap-svc"),
        "got {found}"
    );
    let revision_after_register = found["InstancesRevision"]
        .as_i64()
        .expect("an InstancesRevision");
    assert!(
        revision_after_register > 0,
        "registering an instance bumps the revision, got {revision_after_register}"
    );
    let read = ok(&router, "GetService", json!({ "Id": service_id })).await;
    assert_eq!(
        read["Service"]["InstanceCount"],
        json!(1),
        "InstanceCount follows the instances actually stored, got {read}"
    );

    // Deregister: discovery goes empty and the revision moves again.
    ok(
        &router,
        "DeregisterInstance",
        json!({ "ServiceId": service_id, "InstanceId": "i-1" }),
    )
    .await;
    let found = ok(
        &router,
        "DiscoverInstances",
        json!({ "NamespaceName": "bootstrap-ns", "ServiceName": "bootstrap-svc" }),
    )
    .await;
    assert_eq!(
        found["Instances"].as_array().map(Vec::len),
        Some(0),
        "a deregistered instance is not discoverable, got {found}"
    );
    assert!(
        found["InstancesRevision"].as_i64().expect("a revision") > revision_after_register,
        "deregistering bumps the revision too, got {found}"
    );
    let (status, code) = failure(
        &router,
        "GetInstance",
        json!({ "ServiceId": service_id, "InstanceId": "i-1" }),
    )
    .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "InstanceNotFound"),
        "a deregistered instance is gone from the service record"
    );

    // And the namespace's own create operation is still resolvable.
    let operation = ok(
        &router,
        "GetOperation",
        json!({ "OperationId": operation_id }),
    )
    .await;
    assert_eq!(
        operation["Operation"]["Type"],
        json!("CREATE_NAMESPACE"),
        "got {operation}"
    );
}

/// An operation is reported `SUBMITTED` while its success is still owed, and `SUCCESS`
/// once the clock passes the due instant the create stored.
#[tokio::test]
async fn an_operation_reaches_success_only_once_the_clock_owes_it() {
    let router = emulator();
    let created = ok(
        &router,
        "CreateHttpNamespace",
        json!({ "Name": "clock-ns" }),
    )
    .await;
    let operation_id = created["OperationId"].as_str().expect("an id").to_string();

    let immediate = ok(
        &router,
        "GetOperation",
        json!({ "OperationId": operation_id }),
    )
    .await;
    assert_eq!(
        immediate["Operation"]["Status"],
        json!("SUBMITTED"),
        "a caller polling straight away still sees the non-terminal status, got {immediate}"
    );

    // Blocking is fine here: the test owns the runtime, and the service never sleeps
    // for this span — it stores the due instant and materialises it on read.
    std::thread::sleep(cloudmap::state::OPERATION_SUCCESS_SPAN + Duration::from_millis(50));

    let settled = ok(
        &router,
        "GetOperation",
        json!({ "OperationId": operation_id }),
    )
    .await;
    assert_eq!(
        settled["Operation"]["Status"],
        json!("SUCCESS"),
        "the clock owed this operation its terminal status, got {settled}"
    );
    let created_at = settled["Operation"]["CreateDate"]
        .as_f64()
        .expect("a CreateDate");
    let updated_at = settled["Operation"]["UpdateDate"]
        .as_f64()
        .expect("an UpdateDate");
    assert!(
        updated_at > created_at,
        "the transition moves UpdateDate forward: created {created_at}, updated {updated_at}"
    );
    assert_eq!(
        settled["Operation"]["Targets"],
        immediate["Operation"]["Targets"],
        "the transition changes nothing but the status and the update date"
    );
}

/// `QueryParameters`: "Only instances that match all the specified key-value pairs are
/// returned" — asserted over a table of filters against three stored instances.
#[tokio::test]
async fn query_parameters_keep_only_instances_matching_every_pair() {
    let router = emulator();
    let (_, namespace_id) = namespace(&router, "filter-ns").await;
    let service_id = service(&router, &namespace_id, "filter-svc").await;

    let stored: BTreeMap<&str, Value> = BTreeMap::from([
        (
            "i-prod-a",
            json!({ "AWS_INSTANCE_IPV4": "10.0.0.1", "stage": "prod", "az": "1a" }),
        ),
        (
            "i-prod-b",
            json!({ "AWS_INSTANCE_IPV4": "10.0.0.2", "stage": "prod", "az": "1b" }),
        ),
        (
            "i-dev",
            json!({ "AWS_INSTANCE_IPV4": "10.0.0.3", "stage": "dev", "az": "1a" }),
        ),
    ]);
    for (instance_id, attributes) in &stored {
        ok(
            &router,
            "RegisterInstance",
            json!({
                "ServiceId": service_id,
                "InstanceId": instance_id,
                "Attributes": attributes,
            }),
        )
        .await;
    }

    // Each row differs in the number of pairs, the key, and whether the value matches.
    let cases: Vec<(&str, Value, Vec<&str>)> = vec![
        ("no filter", json!({}), vec!["i-dev", "i-prod-a", "i-prod-b"]),
        (
            "one pair",
            json!({ "stage": "prod" }),
            vec!["i-prod-a", "i-prod-b"],
        ),
        (
            "two pairs",
            json!({ "stage": "prod", "az": "1a" }),
            vec!["i-prod-a"],
        ),
        (
            "a pair whose value matches nothing",
            json!({ "stage": "staging" }),
            vec![],
        ),
        (
            "a key no instance carries",
            json!({ "tenant": "acme" }),
            vec![],
        ),
        (
            "pairs that no single instance carries together",
            json!({ "stage": "dev", "az": "1b" }),
            vec![],
        ),
        (
            // A key in the service's own reserved attribute namespace is not a custom
            // attribute and is refused rather than filtered on, which
            // `service_identity_and_error_bodies` asserts.
            "a custom key only one instance carries the value of",
            json!({ "az": "1b" }),
            vec!["i-prod-b"],
        ),
    ];

    for (case, query, expected) in cases {
        let mut body = json!({ "NamespaceName": "filter-ns", "ServiceName": "filter-svc" });
        if let Value::Object(pairs) = &query {
            if !pairs.is_empty() {
                body["QueryParameters"] = query.clone();
            }
        }
        let found = discovered(&router, body).await;
        let expected: BTreeSet<String> = expected.into_iter().map(String::from).collect();
        assert_eq!(
            found, expected,
            "case {case:?} with {query}: expected {expected:?}, got {found:?}"
        );
    }
}

/// `OptionalParameters` are opportunistic: they narrow the result when something
/// matches both filters, and are ignored when nothing does.
#[tokio::test]
async fn optional_parameters_narrow_the_result_only_when_they_match_something() {
    let router = emulator();
    let (_, namespace_id) = namespace(&router, "optional-ns").await;
    let service_id = service(&router, &namespace_id, "optional-svc").await;
    for (instance_id, attributes) in [
        ("i-1", json!({ "stage": "prod", "az": "1a" })),
        ("i-2", json!({ "stage": "prod", "az": "1b" })),
    ] {
        ok(
            &router,
            "RegisterInstance",
            json!({ "ServiceId": service_id, "InstanceId": instance_id, "Attributes": attributes }),
        )
        .await;
    }

    let cases: Vec<(&str, Value, Vec<&str>)> = vec![
        ("optional filter matches one", json!({ "az": "1a" }), vec!["i-1"]),
        (
            "optional filter matches none, so it is ignored",
            json!({ "az": "1c" }),
            vec!["i-1", "i-2"],
        ),
        (
            "optional filter matches every candidate",
            json!({ "stage": "prod" }),
            vec!["i-1", "i-2"],
        ),
    ];
    for (case, optional, expected) in cases {
        let found = discovered(
            &router,
            json!({
                "NamespaceName": "optional-ns",
                "ServiceName": "optional-svc",
                "QueryParameters": { "stage": "prod" },
                "OptionalParameters": optional,
            }),
        )
        .await;
        let expected: BTreeSet<String> = expected.into_iter().map(String::from).collect();
        assert_eq!(
            found, expected,
            "case {case:?} with {optional}: expected {expected:?}, got {found:?}"
        );
    }
}

/// `HealthStatus` "is ignored for services that don't have a health check configured,
/// and all instances are returned"; with one configured, the filter selects on the
/// instance's own status — which for a Route 53 health check is the verdict the service
/// computes once the check is due (`ROUTE53_HEALTH_CHECK_VERDICT`) — and a request that
/// names no status gets the instances the service still routes traffic to.
///
/// The filter values come from the model's `HealthStatusFilter` enum
/// (`HEALTH_STATUS_FILTER_VALUES` in `src/ops/types/shared_types.rs`).
#[tokio::test]
async fn the_health_status_filter_applies_only_to_a_health_checked_service() {
    let router = emulator();
    let (_, namespace_id) = namespace(&router, "health-ns").await;

    // One service without health checking, one with an HTTP health check.
    let plain_id = service(&router, &namespace_id, "plain-svc").await;
    let checked = ok(
        &router,
        "CreateService",
        json!({
            "Name": "checked-svc",
            "NamespaceId": namespace_id,
            "HealthCheckConfig": { "Type": "HTTP", "ResourcePath": "/ping", "FailureThreshold": 2 },
        }),
    )
    .await;
    let checked_id = checked["Service"]["Id"]
        .as_str()
        .expect("an id")
        .to_string();
    for service_id in [plain_id.as_str(), checked_id.as_str()] {
        ok(
            &router,
            "RegisterInstance",
            json!({ "ServiceId": service_id, "InstanceId": "i-1",
                    "Attributes": { "AWS_INSTANCE_IPV4": "10.0.0.9" } }),
        )
        .await;
    }

    // (service name, filter, expected instance ids)
    let cases: Vec<(&str, Option<&str>, Vec<&str>)> = vec![
        ("plain-svc", None, vec!["i-1"]),
        ("plain-svc", Some("ALL"), vec!["i-1"]),
        // Ignored without a health check: the instance is returned anyway.
        ("plain-svc", Some("HEALTHY"), vec!["i-1"]),
        ("plain-svc", Some("UNHEALTHY"), vec!["i-1"]),
        ("plain-svc", Some("HEALTHY_OR_ELSE_ALL"), vec!["i-1"]),
        // A request that names no status asks for the instances the service still
        // routes traffic to, and this one's instance is reported unhealthy.
        ("checked-svc", None, vec![]),
        ("checked-svc", Some("ALL"), vec!["i-1"]),
        // The service's own checker reported: nothing it can probe here answers, so
        // the instance is unhealthy rather than healthy.
        ("checked-svc", Some("HEALTHY"), vec![]),
        ("checked-svc", Some("UNHEALTHY"), vec!["i-1"]),
        // Failing open: no healthy instance, so everything is returned.
        ("checked-svc", Some("HEALTHY_OR_ELSE_ALL"), vec!["i-1"]),
    ];
    // The Route 53 verdict is owed, not instant, so the filter rows below are read
    // once the clock has reached it. Blocking is fine here: the test owns the runtime,
    // and the service never sleeps for the span.
    std::thread::sleep(cloudmap::state::ROUTE53_HEALTH_CHECK_SPAN + Duration::from_millis(50));
    for (service_name, filter, expected) in cases {
        let mut body = json!({ "NamespaceName": "health-ns", "ServiceName": service_name });
        if let Some(filter) = filter {
            body["HealthStatus"] = json!(filter);
        }
        let found = discovered(&router, body).await;
        let expected: BTreeSet<String> = expected.into_iter().map(String::from).collect();
        assert_eq!(
            found, expected,
            "case service {service_name:?} filter {filter:?}: expected {expected:?}, got {found:?}"
        );
    }
}

/// `MaxResults` caps the list. The published range is 1..1000, so each bound is
/// accepted and one past each bound is refused before the handler runs.
#[tokio::test]
async fn max_results_caps_the_list_and_is_bounded_by_its_declared_range() {
    let router = emulator();
    let (_, namespace_id) = namespace(&router, "paging-ns").await;
    let service_id = service(&router, &namespace_id, "paging-svc").await;
    for index in 0..3 {
        ok(
            &router,
            "RegisterInstance",
            json!({
                "ServiceId": service_id,
                "InstanceId": format!("i-{index}"),
                "Attributes": { "AWS_INSTANCE_IPV4": format!("10.0.0.{index}") },
            }),
        )
        .await;
    }

    for (case, max_results, expected) in [
        ("the low bound", 1_i64, 1_usize),
        ("below the instance count", 2, 2),
        ("at the instance count", 3, 3),
        ("above the instance count", 10, 3),
        ("the high bound", 1000, 3),
    ] {
        let found = discovered(
            &router,
            json!({
                "NamespaceName": "paging-ns",
                "ServiceName": "paging-svc",
                "MaxResults": max_results,
            }),
        )
        .await;
        assert_eq!(
            found.len(),
            expected,
            "case {case:?} MaxResults={max_results}: expected {expected} instances, got {found:?}"
        );
    }

    // Unspecified returns everything up to the documented default of 100.
    let found = discovered(
        &router,
        json!({ "NamespaceName": "paging-ns", "ServiceName": "paging-svc" }),
    )
    .await;
    assert_eq!(found.len(), 3, "no MaxResults returns all, got {found:?}");

    for (case, max_results) in [("one below the low bound", 0_i64), ("one above the high bound", 1001)] {
        let (status, _) = call(
            &router,
            "DiscoverInstances",
            json!({
                "NamespaceName": "paging-ns",
                "ServiceName": "paging-svc",
                "MaxResults": max_results,
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?} MaxResults={max_results}: a value outside the declared range is refused"
        );
    }
}

/// Re-registering an instance id replaces its attributes instead of adding a second
/// instance, and a replayed `CreatorRequestId` replays the first result.
#[tokio::test]
async fn registering_is_a_replace_and_creator_request_ids_replay() {
    let router = emulator();
    let (_, namespace_id) = namespace(&router, "replace-ns").await;
    let service_id = service(&router, &namespace_id, "replace-svc").await;

    let revisions: Vec<i64> = {
        let mut seen = Vec::new();
        for attributes in [
            json!({ "AWS_INSTANCE_IPV4": "10.0.0.1" }),
            json!({ "AWS_INSTANCE_IPV4": "10.0.0.2", "stage": "prod" }),
            json!({ "stage": "canary" }),
        ] {
            ok(
                &router,
                "RegisterInstance",
                json!({ "ServiceId": service_id, "InstanceId": "i-1", "Attributes": attributes }),
            )
            .await;
            let read = ok(
                &router,
                "GetInstance",
                json!({ "ServiceId": service_id, "InstanceId": "i-1" }),
            )
            .await;
            assert_eq!(
                read["Instance"]["Attributes"], attributes,
                "re-registering replaces the attribute map, got {read}"
            );
            let service_read = ok(&router, "GetService", json!({ "Id": service_id })).await;
            assert_eq!(
                service_read["Service"]["InstanceCount"],
                json!(1),
                "re-registering the same id must not duplicate the instance, got {service_read}"
            );
            let found = ok(
                &router,
                "DiscoverInstances",
                json!({ "NamespaceName": "replace-ns", "ServiceName": "replace-svc" }),
            )
            .await;
            seen.push(found["InstancesRevision"].as_i64().expect("a revision"));
        }
        seen
    };
    assert!(
        revisions.windows(2).all(|pair| pair[1] > pair[0]),
        "every register bumps InstancesRevision, got {revisions:?}"
    );

    // The same CreatorRequestId replays the first operation id, both for a namespace
    // and for an instance registration.
    let first = ok(
        &router,
        "CreateHttpNamespace",
        json!({ "Name": "token-ns", "CreatorRequestId": "token-1" }),
    )
    .await;
    let replay = ok(
        &router,
        "CreateHttpNamespace",
        json!({ "Name": "token-ns", "CreatorRequestId": "token-1" }),
    )
    .await;
    assert_eq!(
        first["OperationId"], replay["OperationId"],
        "a replayed CreatorRequestId returns the first operation, got {replay}"
    );

    let first = ok(
        &router,
        "RegisterInstance",
        json!({ "ServiceId": service_id, "InstanceId": "i-2",
                "CreatorRequestId": "token-2", "Attributes": { "stage": "prod" } }),
    )
    .await;
    let replay = ok(
        &router,
        "RegisterInstance",
        json!({ "ServiceId": service_id, "InstanceId": "i-2",
                "CreatorRequestId": "token-2", "Attributes": { "stage": "dev" } }),
    )
    .await;
    assert_eq!(
        first["OperationId"], replay["OperationId"],
        "a replayed RegisterInstance token returns the first operation, got {replay}"
    );
    let read = ok(
        &router,
        "GetInstance",
        json!({ "ServiceId": service_id, "InstanceId": "i-2" }),
    )
    .await;
    assert_eq!(
        read["Instance"]["Attributes"],
        json!({ "stage": "prod" }),
        "the replay must not apply the second request's attributes, got {read}"
    );

    // And a service token replays the stored service rather than creating a second one.
    let first = ok(
        &router,
        "CreateService",
        json!({ "Name": "token-svc", "NamespaceId": namespace_id, "CreatorRequestId": "token-3" }),
    )
    .await;
    let replay = ok(
        &router,
        "CreateService",
        json!({ "Name": "token-svc", "NamespaceId": namespace_id, "CreatorRequestId": "token-3" }),
    )
    .await;
    assert_eq!(
        first["Service"], replay["Service"],
        "a replayed CreateService token returns the first service, got {replay}"
    );
}

/// An idempotency token identifies ONE request: reusing it for a DIFFERENT resource
/// must create that resource, never silently answer with the first call's result.
#[tokio::test]
async fn a_reused_token_naming_another_resource_is_not_a_replay() {
    let router = emulator();
    let (_, namespace_id) = namespace(&router, "scope-ns").await;
    let service_id = service(&router, &namespace_id, "scope-svc").await;
    let other_service_id = service(&router, &namespace_id, "scope-svc-2").await;

    // A namespace: same token, different Name.
    let first = ok(
        &router,
        "CreateHttpNamespace",
        json!({ "Name": "scope-ns-a", "CreatorRequestId": "shared-token" }),
    )
    .await;
    let second = ok(
        &router,
        "CreateHttpNamespace",
        json!({ "Name": "scope-ns-b", "CreatorRequestId": "shared-token" }),
    )
    .await;
    assert_ne!(
        first["OperationId"], second["OperationId"],
        "a token reused for another namespace name files its own operation, got {second}"
    );
    for name in ["scope-ns-a", "scope-ns-b"] {
        // The namespace must exist: discovery resolves it and then fails on the
        // service name, not on the namespace name.
        let (status, code) = failure(
            &router,
            "DiscoverInstances",
            json!({ "NamespaceName": name, "ServiceName": "scope-svc" }),
        )
        .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ServiceNotFound"),
            "namespace {name:?} must have been created by the reused token"
        );
    }

    // A service: same token, different Name in the same namespace.
    let first = ok(
        &router,
        "CreateService",
        json!({ "Name": "scope-a", "NamespaceId": namespace_id, "CreatorRequestId": "svc-token" }),
    )
    .await;
    let second = ok(
        &router,
        "CreateService",
        json!({ "Name": "scope-b", "NamespaceId": namespace_id, "CreatorRequestId": "svc-token" }),
    )
    .await;
    assert_ne!(
        first["Service"]["Id"], second["Service"]["Id"],
        "a token reused for another service name creates that service, got {second}"
    );
    assert_eq!(
        second["Service"]["Name"],
        json!("scope-b"),
        "the second create must be the service that was asked for, got {second}"
    );

    // An instance: same token, different InstanceId, and same token in another service.
    let first = ok(
        &router,
        "RegisterInstance",
        json!({ "ServiceId": service_id, "InstanceId": "i-1",
                "CreatorRequestId": "inst-token", "Attributes": { "stage": "prod" } }),
    )
    .await;
    let other_instance = ok(
        &router,
        "RegisterInstance",
        json!({ "ServiceId": service_id, "InstanceId": "i-2",
                "CreatorRequestId": "inst-token", "Attributes": { "stage": "dev" } }),
    )
    .await;
    let other_service = ok(
        &router,
        "RegisterInstance",
        json!({ "ServiceId": other_service_id, "InstanceId": "i-1",
                "CreatorRequestId": "inst-token", "Attributes": { "stage": "canary" } }),
    )
    .await;
    assert_ne!(
        first["OperationId"], other_instance["OperationId"],
        "a token reused for another instance id registers it, got {other_instance}"
    );
    assert_ne!(
        first["OperationId"], other_service["OperationId"],
        "a token reused in another service registers there, got {other_service}"
    );
    for (service, instance, stage) in [
        (service_id.as_str(), "i-1", "prod"),
        (service_id.as_str(), "i-2", "dev"),
        (other_service_id.as_str(), "i-1", "canary"),
    ] {
        let read = ok(
            &router,
            "GetInstance",
            json!({ "ServiceId": service, "InstanceId": instance }),
        )
        .await;
        assert_eq!(
            read["Instance"]["Attributes"],
            json!({ "stage": stage }),
            "every distinct request stored its own instance, got {read}"
        );
    }
}

/// The operation store is bounded: it keeps at most `MAX_RETAINED_OPERATIONS`
/// records, and drops the oldest already-terminal record first so a freshly filed
/// operation is still resolvable.
#[test]
fn the_operation_store_is_bounded_and_evicts_the_oldest_terminal_record() {
    let mut state = cloudmap::ComAmazonawsServicediscoveryRoute53autonamingV20170314State::default();
    let bound = cloudmap::state::MAX_RETAINED_OPERATIONS;
    let base = std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);

    let mut filed = Vec::new();
    for index in 0..bound + 10 {
        let at = base + Duration::from_millis(index as u64);
        filed.push(state.record_operation(
            "REGISTER_INSTANCE",
            at,
            [("INSTANCE".to_string(), format!("i-{index}"))],
        ));
        assert!(
            state.operations.len() <= bound,
            "after {} records the store holds {}, over the bound of {bound}",
            index + 1,
            state.operations.len()
        );
    }

    // The newest records survive; the oldest ones were evicted.
    let newest = filed.last().expect("at least one operation");
    let settled = base + Duration::from_secs(60);
    assert!(
        state.operation(newest, settled).is_some(),
        "the most recently filed operation must still resolve"
    );
    let oldest = filed.first().expect("at least one operation");
    assert!(
        state.operation(oldest, settled).is_none(),
        "the oldest operation must have been evicted once the bound was passed"
    );
}

/// A token is forgotten with the operation it produced, so a replay never hands back
/// an `OperationId` `GetOperation` can no longer resolve.
#[test]
fn evicting_an_operation_forgets_the_token_that_produced_it() {
    let mut state = cloudmap::ComAmazonawsServicediscoveryRoute53autonamingV20170314State::default();
    let scope = cloudmap::state::IdempotencyScope::CreateHttpNamespace;
    let base = std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);

    let doomed = state.record_operation("CREATE_NAMESPACE", base, []);
    state.remember(scope, "old-token", "old-ns", &doomed);
    assert_eq!(
        state.replayed(scope, "old-token", "old-ns"),
        Some(doomed.clone()),
        "the token replays while its operation is retained"
    );

    // Push the first record out of the store.
    for index in 1..=cloudmap::state::MAX_RETAINED_OPERATIONS {
        state.record_operation(
            "CREATE_NAMESPACE",
            base + Duration::from_millis(index as u64),
            [],
        );
    }
    let live = state.record_operation(
        "CREATE_NAMESPACE",
        base + Duration::from_secs(1),
        [],
    );
    state.remember(scope, "new-token", "new-ns", &live);

    assert!(
        state.operation(&doomed, base + Duration::from_secs(60)).is_none(),
        "the first operation must have been evicted"
    );
    assert_eq!(
        state.replayed(scope, "old-token", "old-ns"),
        None,
        "the token for an evicted operation must be forgotten, got {:?}",
        state.replayed(scope, "old-token", "old-ns")
    );
    assert_eq!(
        state.replayed(scope, "new-token", "new-ns"),
        Some(live),
        "a token whose operation is still stored must keep replaying"
    );
}

/// Every resource store is bounded too, and the bound is answered with the modelled
/// `ResourceLimitExceeded` rather than by dropping something a caller created.
#[tokio::test]
async fn a_full_resource_store_is_refused_with_resource_limit_exceeded() {
    let (router, engine) = emulator_with_engine();
    let (_, namespace_id) = namespace(&router, "quota-ns").await;
    let service_id = service(&router, &namespace_id, "quota-svc").await;
    let state = engine.state::<cloudmap::ComAmazonawsServicediscoveryRoute53autonamingV20170314State>();

    // Seed each store up to its bound (far cheaper than the equivalent API calls).
    {
        let mut store = state.lock();
        let seeded = store
            .namespaces
            .get(&namespace_id)
            .cloned()
            .expect("the namespace just created");
        while store.namespaces.len() < cloudmap::state::MAX_NAMESPACES {
            let filler_id = format!("ns-filler-{}", store.namespaces.len());
            let mut filler = seeded.clone();
            filler.id = filler_id.clone();
            filler.name = filler_id.clone();
            filler.http_name = Some(filler_id.clone());
            store.namespaces.insert(filler_id, filler);
        }

        let seeded = store
            .services
            .get(&service_id)
            .cloned()
            .expect("the service just created");
        while store
            .services
            .values()
            .filter(|service| service.namespace_id == namespace_id)
            .count()
            < cloudmap::state::MAX_SERVICES_PER_NAMESPACE
        {
            let filler_id = format!("srv-filler-{}", store.services.len());
            let mut filler = seeded.clone();
            filler.id = filler_id.clone();
            filler.name = filler_id.clone();
            store.services.insert(filler_id, filler);
        }

        let service = store
            .services
            .get_mut(&service_id)
            .expect("the service just created");
        while service.instances.len() < cloudmap::state::MAX_INSTANCES_PER_SERVICE {
            let instance_id = format!("i-filler-{}", service.instances.len());
            service.instances.insert(
                instance_id.clone(),
                cloudmap::state::InstanceRecord {
                    instance_id,
                    attributes: std::collections::HashMap::new(),
                    creator_request_id: None,
                    created_by_account: cloudmap::state::owning_account(),
                    health_status: cloudmap::state::HEALTH_STATUS_UNKNOWN.to_string(),
                    pending_health_status: None,
                },
            );
        }
    }

    let cases: Vec<(&str, &str, Value)> = vec![
        (
            "one namespace past the namespace bound",
            "CreateHttpNamespace",
            json!({ "Name": "quota-ns-overflow" }),
        ),
        (
            "one service past the per-namespace bound",
            "CreateService",
            json!({ "Name": "quota-svc-overflow", "NamespaceId": namespace_id }),
        ),
        (
            "one instance past the per-service bound",
            "RegisterInstance",
            json!({ "ServiceId": service_id, "InstanceId": "i-overflow",
                    "Attributes": { "stage": "prod" } }),
        ),
    ];
    for (case, operation, body) in cases {
        let (status, code) = failure(&router, operation, body.clone()).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ResourceLimitExceeded"),
            "case {case:?} on {operation} with {body}: expected 400 ResourceLimitExceeded"
        );
    }

    // Replacing an instance that is already stored adds nothing, so the bound does
    // not refuse it.
    let replaced = ok(
        &router,
        "RegisterInstance",
        json!({ "ServiceId": service_id, "InstanceId": "i-filler-0",
                "Attributes": { "stage": "prod" } }),
    )
    .await;
    assert!(
        replaced["OperationId"].is_string(),
        "a replace at the bound must still be accepted, got {replaced}"
    );
}

/// `Service$Type` says what can discover the instances: `HTTP` when no DNS record is
/// registered for them, `DNS_HTTP` when both DNS queries and `DiscoverInstances` can
/// find them.
#[tokio::test]
async fn the_service_type_follows_whether_dns_records_are_registered() {
    let (router, engine) = emulator_with_engine();
    let (_, http_namespace) = namespace(&router, "type-ns").await;

    // A DNS namespace, so the rule's other arm is exercised too. Only HTTP namespaces
    // can be CREATED in this slice (CreatePrivateDnsNamespace is still 501), so the
    // record is seeded with the hosted zone such a namespace carries.
    let state = engine.state::<cloudmap::ComAmazonawsServicediscoveryRoute53autonamingV20170314State>();
    let dns_namespace = "ns-dnsprivate".to_string();
    {
        let mut store = state.lock();
        let mut record = store
            .namespaces
            .get(&http_namespace)
            .cloned()
            .expect("the namespace just created");
        record.id = dns_namespace.clone();
        record.name = "type-dns-ns".to_string();
        record.http_name = Some("type-dns-ns".to_string());
        record.namespace_type = "DNS_PRIVATE".to_string();
        record.hosted_zone_id = Some("Z00000000000000000001".to_string());
        store
            .namespace_ids_by_name
            .insert(record.name.clone(), dns_namespace.clone());
        store.namespaces.insert(dns_namespace.clone(), record);
    }

    let dns_config = json!({ "DnsRecords": [{ "Type": "A", "TTL": 60 }] });
    // (case, the namespace to create in, the extra request members, expected Type)
    // A `DnsConfig` against the HTTP namespace is not a row here: it is refused, which
    // `cloud_map_lifecycle::a_dns_config_is_refused_against_a_namespace_that_holds_no_hosted_zone`
    // pins.
    let cases: Vec<(&str, &str, Value, &str)> = vec![
        ("no DnsConfig", http_namespace.as_str(), json!({}), "HTTP"),
        ("Type=HTTP", http_namespace.as_str(), json!({ "Type": "HTTP" }), "HTTP"),
        (
            "no DnsConfig in a DNS namespace, so no records are registered",
            dns_namespace.as_str(),
            json!({}),
            "HTTP",
        ),
        (
            "a DnsConfig in a DNS namespace, so DNS queries find it too",
            dns_namespace.as_str(),
            json!({ "DnsConfig": dns_config.clone() }),
            "DNS_HTTP",
        ),
        (
            "Type=HTTP in a DNS namespace, which registers no records",
            dns_namespace.as_str(),
            json!({ "Type": "HTTP", "DnsConfig": dns_config.clone() }),
            "HTTP",
        ),
    ];
    for (index, (case, namespace_id, extra, expected)) in cases.into_iter().enumerate() {
        let mut body = json!({ "Name": format!("type-svc-{index}"), "NamespaceId": namespace_id });
        if let Value::Object(members) = extra {
            for (key, value) in members {
                body[key.as_str()] = value;
            }
        }
        let created = ok(&router, "CreateService", body.clone()).await;
        assert_eq!(
            created["Service"]["Type"],
            json!(expected),
            "case {case:?} from {body}: expected Type {expected}, got {created}"
        );
        if let Some(dns) = body.get("DnsConfig") {
            assert_eq!(
                created["Service"]["DnsConfig"]["DnsRecords"],
                dns["DnsRecords"],
                "case {case:?}: the stored DnsConfig is reported back, got {created}"
            );
        }
    }
}

/// The tag quota the `TooManyTagsException` documentation states ("the maximum number
/// of tags that can be applied to a resource is 50") is enforced on both creates.
#[tokio::test]
async fn tags_up_to_the_documented_quota_are_accepted_and_one_more_is_refused() {
    let router = emulator();
    let (_, namespace_id) = namespace(&router, "tag-ns").await;

    let tags = |count: usize| -> Value {
        Value::Array(
            (0..count)
                .map(|index| json!({ "Key": format!("k{index}"), "Value": format!("v{index}") }))
                .collect(),
        )
    };
    let quota = cloudmap::state::MAX_TAGS_PER_RESOURCE;

    for (case, count, expect_ok) in [
        ("no tags", 0_usize, true),
        ("one tag", 1, true),
        ("at the quota", quota, true),
        ("one past the quota", quota + 1, false),
    ] {
        let namespace_body = json!({ "Name": format!("tagged-ns-{count}"), "Tags": tags(count) });
        let public_namespace_body = json!({
            "Name": format!("tagged-public-{count}.example.com"),
            "Tags": tags(count),
        });
        let service_body = json!({
            "Name": format!("tagged-svc-{count}"),
            "NamespaceId": namespace_id,
            "Tags": tags(count),
        });
        for (operation, body) in [
            ("CreateHttpNamespace", namespace_body),
            ("CreatePublicDnsNamespace", public_namespace_body),
            ("CreateService", service_body),
        ] {
            let (status, json) = call(&router, operation, body).await;
            if expect_ok {
                assert_eq!(
                    status, 200,
                    "case {case:?} ({count} tags) on {operation}: expected success, got {status} {json}"
                );
            } else {
                let code = json["__type"].as_str().unwrap_or_default();
                assert_eq!(
                    status, 400,
                    "case {case:?} ({count} tags) on {operation}: expected 400, got {status} {json}"
                );
                assert!(
                    code.ends_with("TooManyTagsException"),
                    "case {case:?} on {operation}: expected TooManyTagsException, got {code:?}"
                );
            }
        }
    }
}

/// Every modelled refusal in this slice, over a table of requests that name something
/// that does not exist or already does.
#[tokio::test]
async fn unknown_and_conflicting_resources_answer_their_modelled_errors() {
    let router = emulator();
    let (_, namespace_id) = namespace(&router, "errors-ns").await;
    let service_id = service(&router, &namespace_id, "errors-svc").await;
    ok(
        &router,
        "RegisterInstance",
        json!({ "ServiceId": service_id, "InstanceId": "i-1", "Attributes": { "stage": "prod" } }),
    )
    .await;

    let cases: Vec<(&str, &str, Value, u16, &str)> = vec![
        (
            "an operation id nothing filed",
            "GetOperation",
            json!({ "OperationId": "nosuchoperation" }),
            400,
            "OperationNotFound",
        ),
        (
            "a namespace id nothing created",
            "GetNamespace",
            json!({ "Id": "ns-nosuchnamespace" }),
            400,
            "NamespaceNotFound",
        ),
        (
            "a service in a namespace that does not exist",
            "CreateService",
            json!({ "Name": "orphan-svc", "NamespaceId": "ns-nosuchnamespace" }),
            400,
            "NamespaceNotFound",
        ),
        (
            "a service with no namespace named at all",
            "CreateService",
            json!({ "Name": "orphan-svc" }),
            400,
            "InvalidInput",
        ),
        (
            "both health check styles at once",
            "CreateService",
            json!({
                "Name": "two-checks-svc",
                "NamespaceId": namespace_id,
                "HealthCheckConfig": { "Type": "HTTP" },
                "HealthCheckCustomConfig": { "FailureThreshold": 1 },
            }),
            400,
            "InvalidInput",
        ),
        (
            "a namespace name that is already taken",
            "CreateHttpNamespace",
            json!({ "Name": "errors-ns" }),
            // An HTTP namespace claims no DNS name, so a repeat of its create is the
            // duplicate REQUEST naming the operation already on file for that name; the
            // DNS creates answer the name conflict, which `namespace_name_conflict` states
            // over every pair of creates.
            400,
            "DuplicateRequest",
        ),
        (
            "a service name that is already taken in the namespace",
            "CreateService",
            json!({ "Name": "errors-svc", "NamespaceId": namespace_id }),
            400,
            "ServiceAlreadyExists",
        ),
        (
            "a service id nothing created",
            "GetService",
            json!({ "Id": "srv-nosuchservice" }),
            400,
            "ServiceNotFound",
        ),
        (
            "registering into a service that does not exist",
            "RegisterInstance",
            json!({ "ServiceId": "srv-nosuchservice", "InstanceId": "i-9",
                    "Attributes": { "stage": "prod" } }),
            400,
            "ServiceNotFound",
        ),
        (
            "deregistering from a service that does not exist",
            "DeregisterInstance",
            json!({ "ServiceId": "srv-nosuchservice", "InstanceId": "i-1" }),
            400,
            "ServiceNotFound",
        ),
        (
            "deregistering an instance that was never registered",
            "DeregisterInstance",
            json!({ "ServiceId": service_id, "InstanceId": "i-never" }),
            400,
            "InstanceNotFound",
        ),
        (
            "an instance that was never registered",
            "GetInstance",
            json!({ "ServiceId": service_id, "InstanceId": "i-never" }),
            400,
            "InstanceNotFound",
        ),
        (
            "discovery in a namespace that does not exist",
            "DiscoverInstances",
            json!({ "NamespaceName": "nosuch-ns", "ServiceName": "errors-svc" }),
            400,
            "NamespaceNotFound",
        ),
        (
            "discovery of a service that does not exist",
            "DiscoverInstances",
            json!({ "NamespaceName": "errors-ns", "ServiceName": "nosuch-svc" }),
            400,
            "ServiceNotFound",
        ),
    ];

    for (case, operation, body, expected_status, expected_code) in cases {
        let (status, code) = failure(&router, operation, body.clone()).await;
        assert_eq!(
            (status, code.as_str()),
            (expected_status, expected_code),
            "case {case:?} on {operation} with {body}: expected {expected_status} {expected_code}"
        );
    }

    // The refusal names the create operation already on file for the name, so a caller can
    // poll that operation rather than guess which call took the name.
    let create_operation = ok(&router, "CreateHttpNamespace", json!({ "Name": "poll-ns" })).await
        ["OperationId"]
        .as_str()
        .expect("an OperationId")
        .to_string();
    let owning_namespace = ok(
        &router,
        "GetOperation",
        json!({ "OperationId": create_operation }),
    )
    .await["Operation"]["Targets"]["NAMESPACE"]
        .as_str()
        .expect("the CREATE_NAMESPACE operation targets the namespace it created")
        .to_string();
    let (_, conflict) = call(&router, "CreateHttpNamespace", json!({ "Name": "poll-ns" })).await;
    assert_eq!(
        conflict["DuplicateOperationId"],
        json!(create_operation),
        "DuplicateRequest names the create operation on file for the name, got {conflict}"
    );
    // That operation is the one that created the namespace holding the name.
    assert_eq!(
        ok(
            &router,
            "GetNamespace",
            json!({ "Id": owning_namespace.clone() })
        )
        .await["Namespace"]["Name"],
        json!("poll-ns"),
        "the operation the refusal names created the namespace that holds the name"
    );
    let (_, conflict) = call(
        &router,
        "CreateService",
        json!({ "Name": "errors-svc", "NamespaceId": namespace_id }),
    )
    .await;
    assert_eq!(
        conflict["ServiceId"], json!(service_id),
        "ServiceAlreadyExists names the service that already exists, got {conflict}"
    );
}

/// A namespace or service may be named by its ARN as well as its id — the members that
/// name one are modelled as `Arn`.
#[tokio::test]
async fn resources_can_be_named_by_arn_as_well_as_by_id() {
    let router = emulator();
    let (_, namespace_id) = namespace(&router, "arn-ns").await;
    let namespace_arn = ok(&router, "GetNamespace", json!({ "Id": namespace_id })).await
        ["Namespace"]["Arn"]
        .as_str()
        .expect("an ARN")
        .to_string();

    let created = ok(
        &router,
        "CreateService",
        json!({ "Name": "arn-svc", "NamespaceId": namespace_arn }),
    )
    .await;
    assert_eq!(
        created["Service"]["NamespaceId"],
        json!(namespace_id),
        "a namespace named by ARN resolves to the stored namespace, got {created}"
    );
    let service_arn = created["Service"]["Arn"].as_str().expect("an ARN").to_string();
    let service_id = created["Service"]["Id"].as_str().expect("an id").to_string();

    ok(
        &router,
        "RegisterInstance",
        json!({ "ServiceId": service_arn, "InstanceId": "i-1",
                "Attributes": { "AWS_INSTANCE_IPV4": "10.0.0.1" } }),
    )
    .await;
    let read = ok(&router, "GetService", json!({ "Id": service_id })).await;
    assert_eq!(
        read["Service"]["InstanceCount"],
        json!(1),
        "a service named by ARN is the same record as the one named by id, got {read}"
    );
}

/// A token is only a replay while the thing it produced is still there: after the
/// instance is deregistered, the same `CreatorRequestId` must register it again rather
/// than answer success for a write it did not make.
#[tokio::test]
async fn a_token_is_not_replayed_once_its_instance_is_gone() {
    let router = emulator();
    let (_, namespace_id) = namespace(&router, "stale-ns").await;
    let service_id = service(&router, &namespace_id, "stale-svc").await;
    let register = json!({
        "ServiceId": service_id,
        "InstanceId": "i-1",
        "CreatorRequestId": "stale-token",
        "Attributes": { "AWS_INSTANCE_IPV4": "10.0.0.1" },
    });

    let first = ok(&router, "RegisterInstance", register.clone()).await;
    ok(
        &router,
        "DeregisterInstance",
        json!({ "ServiceId": service_id, "InstanceId": "i-1" }),
    )
    .await;
    let again = ok(&router, "RegisterInstance", register.clone()).await;
    assert_ne!(
        first["OperationId"], again["OperationId"],
        "the re-registration is its own operation, not the stale token's, got {again}"
    );

    let read = ok(
        &router,
        "GetInstance",
        json!({ "ServiceId": service_id, "InstanceId": "i-1" }),
    )
    .await;
    assert_eq!(
        read["Instance"]["Attributes"],
        json!({ "AWS_INSTANCE_IPV4": "10.0.0.1" }),
        "the re-registration must have stored the instance, got {read}"
    );
    let found = discovered(
        &router,
        json!({ "NamespaceName": "stale-ns", "ServiceName": "stale-svc" }),
    )
    .await;
    assert_eq!(
        found,
        BTreeSet::from(["i-1".to_string()]),
        "the re-registered instance must be discoverable, got {found:?}"
    );

    // While it IS registered, the same token still replays.
    let replay = ok(&router, "RegisterInstance", register).await;
    assert_eq!(
        again["OperationId"], replay["OperationId"],
        "a retry of a stored registration still replays, got {replay}"
    );
}

/// `AWS_INIT_HEALTH_STATUS`: "If the service configuration includes
/// HealthCheckCustomConfig, you can optionally use AWS_INIT_HEALTH_STATUS to specify
/// the initial status of the custom health check, HEALTHY or UNHEALTHY. If you don't
/// specify a value for AWS_INIT_HEALTH_STATUS, the initial status is HEALTHY."
#[tokio::test]
async fn a_custom_health_check_takes_its_initial_status_from_the_attribute() {
    let router = emulator();
    let (_, namespace_id) = namespace(&router, "init-ns").await;
    let custom = ok(
        &router,
        "CreateService",
        json!({
            "Name": "custom-svc",
            "NamespaceId": namespace_id,
            "HealthCheckCustomConfig": { "FailureThreshold": 1 },
        }),
    )
    .await;
    let custom_id = custom["Service"]["Id"].as_str().expect("an id").to_string();
    // The same instances in a service with no health checking at all, for contrast.
    let plain_id = service(&router, &namespace_id, "plain-svc").await;

    // (case, the attribute value, the status the summary must report)
    let cases: Vec<(&str, Option<&str>, &str)> = vec![
        ("absent, so the documented default", None, "HEALTHY"),
        ("explicitly healthy", Some("HEALTHY"), "HEALTHY"),
        ("explicitly unhealthy", Some("UNHEALTHY"), "UNHEALTHY"),
    ];
    for (index, (case, attribute, expected)) in cases.iter().enumerate() {
        let instance_id = format!("i-{index}");
        let mut attributes = json!({ "AWS_INSTANCE_IPV4": format!("10.0.0.{index}") });
        if let Some(value) = attribute {
            attributes["AWS_INIT_HEALTH_STATUS"] = json!(value);
        }
        for (service_id, service_name, expected) in [
            (&custom_id, "custom-svc", *expected),
            // Without a custom config nothing has reported on the instance.
            (&plain_id, "plain-svc", "UNKNOWN"),
        ] {
            ok(
                &router,
                "RegisterInstance",
                json!({ "ServiceId": service_id, "InstanceId": instance_id,
                        "Attributes": attributes }),
            )
            .await;
            // `ALL`, so the row whose initial status is UNHEALTHY is reported rather
            // than left out of the view a request that names no status gets.
            let found = ok(
                &router,
                "DiscoverInstances",
                json!({ "NamespaceName": "init-ns", "ServiceName": service_name,
                        "HealthStatus": "ALL" }),
            )
            .await;
            let reported = found["Instances"]
                .as_array()
                .expect("a list")
                .iter()
                .find(|summary| summary["InstanceId"] == json!(instance_id))
                .map(|summary| summary["HealthStatus"].clone())
                .unwrap_or(Value::Null);
            assert_eq!(
                reported,
                json!(expected),
                "case {case:?} in {service_name}: expected {expected}, got {reported} ({found})"
            );
            // The directive is not an attribute of the instance, so it is not
            // reported back among them.
            let read = ok(
                &router,
                "GetInstance",
                json!({ "ServiceId": service_id, "InstanceId": instance_id }),
            )
            .await;
            assert_eq!(
                read["Instance"]["Attributes"],
                json!({ "AWS_INSTANCE_IPV4": format!("10.0.0.{index}") }),
                "case {case:?} in {service_name}: AWS_INIT_HEALTH_STATUS is consumed, \
                 not stored as an attribute, got {read}"
            );
        }
    }

    // A value the documented pair does not contain is refused, naming the attribute.
    let (status, code) = failure(
        &router,
        "RegisterInstance",
        json!({ "ServiceId": custom_id, "InstanceId": "i-bad",
                "Attributes": { "AWS_INIT_HEALTH_STATUS": "MAYBE" } }),
    )
    .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "InvalidInput"),
        "an initial status outside HEALTHY/UNHEALTHY is refused"
    );

    // And the healthy ones are exactly what the HEALTHY filter selects.
    let healthy = discovered(
        &router,
        json!({ "NamespaceName": "init-ns", "ServiceName": "custom-svc",
                "HealthStatus": "HEALTHY" }),
    )
    .await;
    assert_eq!(
        healthy,
        BTreeSet::from(["i-0".to_string(), "i-1".to_string()]),
        "the HEALTHY filter selects the instances whose initial status is HEALTHY, got {healthy:?}"
    );
    let unhealthy = discovered(
        &router,
        json!({ "NamespaceName": "init-ns", "ServiceName": "custom-svc",
                "HealthStatus": "UNHEALTHY" }),
    )
    .await;
    assert_eq!(
        unhealthy,
        BTreeSet::from(["i-2".to_string()]),
        "the UNHEALTHY filter selects the one that asked for UNHEALTHY, got {unhealthy:?}"
    );
}

/// The summary's `NamespaceName`/`ServiceName` come from the stored records, so a
/// namespace whose `HttpName` differs from its `Name` reports the `HttpName`.
#[tokio::test]
async fn discovery_reports_the_stored_names_not_the_requested_ones() {
    let (router, engine) = emulator_with_engine();
    let (_, namespace_id) = namespace(&router, "stored-ns").await;
    let service_id = service(&router, &namespace_id, "stored-svc").await;
    ok(
        &router,
        "RegisterInstance",
        json!({ "ServiceId": service_id, "InstanceId": "i-1",
                "Attributes": { "AWS_INSTANCE_IPV4": "10.0.0.1" } }),
    )
    .await;

    // Give the namespace a distinct HttpName, the shape the docs describe when a
    // reused Name forced a hash onto it.
    let state = engine.state::<cloudmap::ComAmazonawsServicediscoveryRoute53autonamingV20170314State>();
    state
        .lock()
        .namespaces
        .get_mut(&namespace_id)
        .expect("the namespace just created")
        .http_name = Some("stored-ns-a1b2c3".to_string());

    for (case, requested) in [
        ("the HttpName", "stored-ns-a1b2c3"),
        ("the Name, through the fallback", "stored-ns"),
    ] {
        let found = ok(
            &router,
            "DiscoverInstances",
            json!({ "NamespaceName": requested, "ServiceName": "stored-svc" }),
        )
        .await;
        let summary = &found["Instances"][0];
        assert_eq!(
            summary["NamespaceName"],
            json!("stored-ns-a1b2c3"),
            "case {case:?}: the summary reports the stored HttpName, got {found}"
        );
        assert_eq!(
            summary["ServiceName"],
            json!("stored-svc"),
            "case {case:?}: the summary reports the stored service name, got {found}"
        );
    }
}
