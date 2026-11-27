// SPDX-License-Identifier: CC0-1.0

//! The `ServiceFunction` lifecycle, end to end over HTTP: `CreateServiceFunction` ->
//! `ListServiceFunctions`, and the cascade `DeleteService` performs over the functions a
//! service holds.
//!
//! The rule under test: a service function is a child of exactly one service, so a create
//! stores it under that service's record and every member of the `ServiceFunction` it
//! answers is projected from what was stored — the caller's `name`, `description` and
//! `criticality` as supplied, a minted `serviceFunctionId`, the `USER` source for a
//! caller-created function, `resourceCount` as the number of resources actually
//! associated, and `createdAt` / `updatedAt`; a listing projects the same stored records,
//! a `serviceArn` naming no stored service is refused `ResourceNotFoundException`, and
//! deleting the service removes its functions with it.
//!
//! Every suite drives the real wire path (`MultiServiceRouter::dispatch`), so the modelled
//! constraints, the restJson1 (de)serializers and the HTTP status codes are exercised
//! exactly as a client would exercise them.
//!
//! The suites are TABLES: each row is one input of the class the rule covers (name length
//! and shape, criticality, description presence and length, page size, scope), not one
//! recorded request.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_resiliencehubv2_ngrhservicecore::state::{
    MAX_RESULTS_MAX, MAX_RESULTS_MIN, MAX_SERVICE_FUNCTIONS_PER_SERVICE,
    SERVICE_FUNCTION_SOURCE_USER,
};
use service_com_amazonaws_resiliencehubv2_ngrhservicecore::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The modelled routes this suite drives.
const CREATE_SERVICE: &str = "/v2/create-service";
const CREATE_SERVICE_FUNCTION: &str = "/v2/create-service-function";
const CREATE_SERVICE_FUNCTION_RESOURCES: &str = "/v2/create-service-function-resources";
const CREATE_INPUT_SOURCE: &str = "/v2/create-input-source";
const DELETE_SERVICE: &str = "/v2/delete-service";
const DELETE_SERVICE_FUNCTION: &str = "/v2/delete-function";
const LIST_SERVICE_FUNCTIONS: &str = "/v2/list-functions";

/// A SigV4 `Authorization` header whose credential scope names this service's signing
/// name (`resiliencehub`) and the Region ARNs are minted in. Signature verification is off
/// for these tests, so the value only has to parse.
const SIGNED: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDTESTONLY00000000/20260101/us-east-1/resiliencehub/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0123456789abcdef";

/// `EntityId`'s modelled bounds (`smithy_query(query="shape", shape="EntityId")` reports
/// `length {min: 1, max: 255}` with pattern `^\S{1,255}$`), which every minted
/// `serviceFunctionId` must satisfy.
const ENTITY_ID_MIN_LENGTH: usize = 1;
const ENTITY_ID_MAX_LENGTH: usize = 255;

/// The values `ServiceFunctionCriticality` accepts on the wire.
///
/// SOURCE: `smithy_query(query="shape", shape="ServiceFunctionCriticality")` — an enum of
/// exactly `PRIMARY` and `SUPPLEMENTAL`.
const CRITICALITY_VALUES: [&str; 2] = ["PRIMARY", "SUPPLEMENTAL"];

/// An emulator holding only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let payload = body
        .map(|value| serde_json::to_vec(&value).expect("a JSON body serializes"))
        .unwrap_or_default();
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", SIGNED)
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(payload))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let raw = response.into_body();
    let parsed = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// Create a service named `name` and answer its ARN.
async fn create_service(router: &MultiServiceRouter, name: &str) -> String {
    let (status, body) = call(
        router,
        "POST",
        CREATE_SERVICE,
        Some(json!({
            "name": name,
            "regions": ["us-east-1"],
            "permissionModel": {"invokerRoleName": "ResilienceHubRole"},
        })),
    )
    .await;
    assert_eq!(status, 200, "creating service {name}: got {body:?}");
    body["service"]["serviceArn"]
        .as_str()
        .unwrap_or_else(|| panic!("no service.serviceArn in {body:?}"))
        .to_string()
}

async fn create_function(router: &MultiServiceRouter, body: Value) -> (u16, Value) {
    call(router, "POST", CREATE_SERVICE_FUNCTION, Some(body)).await
}

async fn list_functions(router: &MultiServiceRouter, query: &str) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("{LIST_SERVICE_FUNCTIONS}?{query}"),
        None,
    )
    .await
}

/// Create an input source for `service_arn` from `configuration`.
async fn create_input_source(
    router: &MultiServiceRouter,
    service_arn: &str,
    configuration: Value,
) {
    let (status, body) = call(
        router,
        "POST",
        CREATE_INPUT_SOURCE,
        Some(json!({"serviceArn": service_arn, "resourceConfiguration": configuration})),
    )
    .await;
    assert_eq!(
        status, 200,
        "creating the input source: got {status} {body:?}"
    );
}

/// Create one function of `service_arn` and answer its minted id.
async fn create_function_id(router: &MultiServiceRouter, service_arn: &str, name: &str) -> String {
    let (status, body) = create_function(
        router,
        json!({
            "serviceArn": service_arn,
            "name": name,
            "criticality": CRITICALITY_VALUES[0],
        }),
    )
    .await;
    assert_eq!(status, 200, "creating function {name}: got {status} {body:?}");
    body["serviceFunction"]["serviceFunctionId"]
        .as_str()
        .unwrap_or_else(|| panic!("no serviceFunctionId in {body:?}"))
        .to_string()
}

/// Associate `resources` with one function of `service_arn`.
async fn associate_resources(
    router: &MultiServiceRouter,
    service_arn: &str,
    service_function_id: &str,
    resources: &[String],
) -> (u16, Value) {
    call(
        router,
        "POST",
        CREATE_SERVICE_FUNCTION_RESOURCES,
        Some(json!({
            "serviceArn": service_arn,
            "serviceFunctionId": service_function_id,
            "resources": resources,
        })),
    )
    .await
}

/// The `ServiceFunction` the service's own listing reports for `service_function_id`: the
/// read every association below is proven through.
async fn listed_function(
    router: &MultiServiceRouter,
    case: &str,
    service_arn: &str,
    service_function_id: &str,
) -> Value {
    let (status, body) = list_functions(router, &format!("serviceArn={service_arn}")).await;
    listed(case, status, &body)
        .into_iter()
        .find(|function| function["serviceFunctionId"] == json!(service_function_id))
        .unwrap_or_else(|| panic!("case {case:?}: no function {service_function_id} in {body:?}"))
}

async fn delete_service(router: &MultiServiceRouter, service_arn: &str) -> (u16, Value) {
    call(
        router,
        "POST",
        DELETE_SERVICE,
        Some(json!({"serviceArn": service_arn})),
    )
    .await
}

/// The modelled error code of a refusal, from the restJson1 `__type` discriminator.
fn error_code(body: &Value) -> String {
    body["__type"]
        .as_str()
        .unwrap_or("<no __type>")
        .rsplit(['#', ':'])
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

/// The `serviceFunctions` array of a successful listing.
fn listed(case: &str, status: u16, body: &Value) -> Vec<Value> {
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from ListServiceFunctions, got {status} {body:?}"
    );
    body["serviceFunctions"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("case {case:?}: the @required serviceFunctions member is absent from {body:?}")
        })
        .clone()
}

/// One row of the create table: the inputs of a single `CreateServiceFunction` call.
#[derive(Debug, Clone)]
struct Row {
    case: &'static str,
    name: String,
    criticality: &'static str,
    description: Option<String>,
}

impl Row {
    fn body(&self, service_arn: &str) -> Value {
        let mut body = json!({
            "serviceArn": service_arn,
            "name": self.name,
            "criticality": self.criticality,
        });
        if let Some(description) = self.description.as_ref() {
            body["description"] = json!(description);
        }
        body
    }
}

/// The create table: rows differ along `name` length and shape (the shortest `EntityLabel`,
/// spaces, an underscore and a hyphen, digits, the longest), `criticality` (both modelled
/// values) and `description` (absent, short, empty, the longest `EntityDescription`).
///
/// SOURCE of the bounds exercised: `EntityLabel` is `len 2..60` with pattern
/// `^[A-Za-z0-9][A-Za-z0-9 _\-]{1,59}$` and `EntityDescription` is `len ..500`
/// (`smithy_query(query="shape", shape="EntityLabel,EntityDescription")`).
fn create_rows() -> Vec<Row> {
    vec![
        Row {
            case: "shortest name, PRIMARY, no description",
            name: "Ab".to_string(),
            criticality: "PRIMARY",
            description: None,
        },
        Row {
            case: "name with spaces, SUPPLEMENTAL, short description",
            name: "Checkout Flow".to_string(),
            criticality: "SUPPLEMENTAL",
            description: Some("Serves customer checkout".to_string()),
        },
        Row {
            case: "name with an underscore and a hyphen, empty description",
            name: "order_api-edge".to_string(),
            criticality: "PRIMARY",
            description: Some(String::new()),
        },
        Row {
            case: "digit-led name, SUPPLEMENTAL",
            name: "90210 regional cache".to_string(),
            criticality: "SUPPLEMENTAL",
            description: Some("Caches catalogue reads".to_string()),
        },
        Row {
            case: "longest name and longest description",
            // 60 characters: EntityLabel's maximum.
            name: format!("Z{}", "y".repeat(59)),
            criticality: "PRIMARY",
            // 500 characters: EntityDescription's maximum.
            description: Some("d".repeat(500)),
        },
    ]
}

/// Assert that `function` is `row` as stored under `service_arn`.
fn assert_projects_row(case: &str, row: &Row, service_arn: &str, function: &Value) {
    assert_eq!(
        function["serviceArn"].as_str(),
        Some(service_arn),
        "case {case:?}: expected the parent service's ARN, got {:?}",
        function["serviceArn"]
    );
    assert_eq!(
        function["name"].as_str(),
        Some(row.name.as_str()),
        "case {case:?}: expected the supplied name, got {:?}",
        function["name"]
    );
    assert_eq!(
        function["criticality"].as_str(),
        Some(row.criticality),
        "case {case:?}: expected the supplied criticality, got {:?}",
        function["criticality"]
    );
    match row.description.as_deref() {
        Some(description) => assert_eq!(
            function["description"].as_str(),
            Some(description),
            "case {case:?}: expected the supplied description, got {:?}",
            function["description"]
        ),
        // An absent optional member is reported absent, not as an empty string nobody
        // stored.
        None => assert!(
            function.get("description").is_none(),
            "case {case:?}: expected no description member, got {:?}",
            function["description"]
        ),
    }
    assert_eq!(
        function["source"].as_str(),
        Some(SERVICE_FUNCTION_SOURCE_USER),
        "case {case:?}: a caller-created function's source is {SERVICE_FUNCTION_SOURCE_USER}, \
         got {:?}",
        function["source"]
    );
    assert_eq!(
        function["resourceCount"].as_i64(),
        Some(0),
        "case {case:?}: a function with no resources associated counts zero, got {:?}",
        function["resourceCount"]
    );
    let id = function["serviceFunctionId"].as_str().unwrap_or_default();
    assert!(
        (ENTITY_ID_MIN_LENGTH..=ENTITY_ID_MAX_LENGTH).contains(&id.len())
            && !id.chars().any(char::is_whitespace),
        "case {case:?}: expected an EntityId of {ENTITY_ID_MIN_LENGTH}..={ENTITY_ID_MAX_LENGTH} \
         non-whitespace characters, got {id:?}"
    );
    for member in ["createdAt", "updatedAt"] {
        assert!(
            function[member].is_number(),
            "case {case:?}: expected a {member} timestamp, got {:?}",
            function[member]
        );
    }
    assert_eq!(
        function["createdAt"], function["updatedAt"],
        "case {case:?}: a function nothing has updated reports updatedAt == createdAt, got \
         {:?} and {:?}",
        function["createdAt"], function["updatedAt"]
    );
}

#[tokio::test]
async fn every_created_service_function_is_stored_and_read_back() {
    let router = emulator();
    let service_arn = create_service(&router, "Payments").await;
    let rows = create_rows();

    let mut created: Vec<(Row, Value)> = Vec::new();
    for row in &rows {
        let (status, body) = create_function(&router, row.body(&service_arn)).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200 from CreateServiceFunction, got {status} {body:?}",
            row.case
        );
        let function = body["serviceFunction"].clone();
        assert!(
            !function.is_null(),
            "case {:?}: the @required serviceFunction member is absent from {body:?}",
            row.case
        );
        assert_projects_row(row.case, row, &service_arn, &function);
        created.push((row.clone(), function));
    }

    // Every function got its own identity, so no create overwrote another.
    let ids: BTreeSet<&str> = created
        .iter()
        .map(|(_, function)| function["serviceFunctionId"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(
        ids.len(),
        created.len(),
        "expected one distinct serviceFunctionId per create, got {created:?}"
    );

    // The ROUND TRIP: the listing projects the same stored records, in creation order.
    let (status, body) = list_functions(&router, &format!("serviceArn={service_arn}")).await;
    let functions = listed("round trip", status, &body);
    assert!(
        body.get("nextToken").is_none(),
        "every function fits one default-sized page, so no nextToken is due: {body:?}"
    );
    assert_eq!(
        functions.len(),
        created.len(),
        "expected every created function in the listing, got {functions:?}"
    );
    for ((row, at_create), from_list) in created.iter().zip(functions.iter()) {
        assert_projects_row(row.case, row, &service_arn, from_list);
        assert_eq!(
            at_create, from_list,
            "case {:?}: the listing must project the same stored record the create \
             answered, got {from_list:?}",
            row.case
        );
    }

    // A read leaves the store untouched, so two listings agree.
    let (status, again) = list_functions(&router, &format!("serviceArn={service_arn}")).await;
    assert_eq!(
        listed("repeated read", status, &again),
        functions,
        "two reads of the same service's functions must answer identically"
    );
}

#[tokio::test]
async fn a_service_arn_naming_no_service_is_refused_not_served() {
    let router = emulator();
    let real = create_service(&router, "Inventory").await;
    // A well-formed `Arn` for a service this emulator never created, and a service ARN
    // that WAS created and then deleted: neither has a record to hold or list a function.
    let deleted = create_service(&router, "Retired").await;
    let (status, body) = delete_service(&router, &deleted).await;
    assert_eq!(status, 200, "deleting the service: got {status} {body:?}");
    let never_created = format!("{real}-absent");

    for (case, service_arn) in [
        ("never created", never_created.as_str()),
        ("created then deleted", deleted.as_str()),
    ] {
        let (status, body) = create_function(
            &router,
            json!({"serviceArn": service_arn, "name": "Orders", "criticality": "PRIMARY"}),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected HTTP 404 from CreateServiceFunction, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {body:?}"
        );

        let (status, body) = list_functions(&router, &format!("serviceArn={service_arn}")).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected HTTP 404 from ListServiceFunctions, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {body:?}"
        );
    }
}

#[tokio::test]
async fn a_function_name_is_taken_only_within_its_own_service() {
    let router = emulator();
    let first = create_service(&router, "Checkout").await;
    let second = create_service(&router, "Fulfilment").await;

    // The rows differ along the name's shape, so the rule is asserted over the class and
    // not over one spelling: (the name, the criticality the first create asks for).
    let rows: [(&str, &str); 3] = [
        ("Ab", CRITICALITY_VALUES[0]),
        ("Order Intake", CRITICALITY_VALUES[1]),
        ("order_api-edge", CRITICALITY_VALUES[0]),
    ];

    for (name, criticality) in rows {
        let (status, created) = create_function(
            &router,
            json!({"serviceArn": first, "name": name, "criticality": criticality}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected HTTP 200 for the first create, got {status} {created:?}"
        );

        // The same name again in the SAME service is a conflict, whichever criticality it
        // asks for.
        for repeat in CRITICALITY_VALUES {
            let (status, refused) = create_function(
                &router,
                json!({"serviceArn": first, "name": name, "criticality": repeat}),
            )
            .await;
            assert_eq!(
                status, 409,
                "case {name:?}/{repeat}: expected HTTP 409 for a duplicate name, got {status} \
                 {refused:?}"
            );
            assert_eq!(
                error_code(&refused),
                "ConflictException",
                "case {name:?}/{repeat}: expected ConflictException, got {refused:?}"
            );
            // The refusal names the resource type and the name the REQUEST supplied, and
            // nothing else — neither the parent service's ARN nor the id of the function
            // already holding the name.
            let expected = format!("ServiceFunction with name already exists: {name}");
            let message = refused["message"].as_str().unwrap_or_default();
            assert_eq!(
                message, expected,
                "case {name:?}/{repeat}: expected {expected:?}, got {message:?}"
            );
        }

        // The same name in ANOTHER service is a different function: the name is scoped to
        // its parent.
        let (status, elsewhere) = create_function(
            &router,
            json!({"serviceArn": second, "name": name, "criticality": criticality}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {name:?}: the same name in another service must be accepted, got {status} \
             {elsewhere:?}"
        );
        assert_ne!(
            elsewhere["serviceFunction"]["serviceFunctionId"],
            created["serviceFunction"]["serviceFunctionId"],
            "case {name:?}: two services' functions are distinct records, got {elsewhere:?}"
        );
    }

    // Each service lists only its own functions.
    for (case, service_arn) in [("first service", &first), ("second service", &second)] {
        let (status, body) = list_functions(&router, &format!("serviceArn={service_arn}")).await;
        let functions = listed(case, status, &body);
        assert_eq!(
            functions.len(),
            3,
            "case {case:?}: expected only this service's three functions, got {functions:?}"
        );
        for function in &functions {
            assert_eq!(
                function["serviceArn"].as_str(),
                Some(service_arn.as_str()),
                "case {case:?}: a listing reports only functions of the service asked for, \
                 got {function:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_replayed_client_token_returns_the_function_the_first_call_created() {
    let router = emulator();
    let service_arn = create_service(&router, "Idempotent").await;

    // The rows differ along the token's shape within `ClientToken`'s modelled
    // `^[A-Za-z0-9_.-]{1,64}$` / `len 1..64`: the shortest, a dotted one, and the longest.
    let tokens = [
        ("shortest token", "t".to_string()),
        ("dotted token", "req.2026-01-01_42".to_string()),
        ("longest token", "k".repeat(64)),
    ];

    for (index, (case, token)) in tokens.iter().enumerate() {
        let body = json!({
            "serviceArn": service_arn,
            "name": format!("Function {index}"),
            "criticality": CRITICALITY_VALUES[index % CRITICALITY_VALUES.len()],
            "clientToken": token,
        });
        let (status, first) = create_function(&router, body.clone()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 for the first call, got {status} {first:?}"
        );
        let (status, replay) = create_function(&router, body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 for the replay, got {status} {replay:?}"
        );
        assert_eq!(
            replay["serviceFunction"], first["serviceFunction"],
            "case {case:?}: a replayed idempotency token must answer the stored function, got \
             {replay:?}"
        );
    }

    let (status, body) = list_functions(&router, &format!("serviceArn={service_arn}")).await;
    let functions = listed("after the replays", status, &body);
    assert_eq!(
        functions.len(),
        tokens.len(),
        "a replay must not store a second function, got {functions:?}"
    );
}

/// RULE: the `clientToken` index of a service is bounded by the same ceiling as the
/// functions themselves — a delete takes the tokens that named the function it removed with
/// it — so repeating create-then-delete under a FRESH token each time never fills the store
/// the ceiling protects, however many times it is repeated.
#[tokio::test]
async fn a_create_delete_cycle_under_fresh_tokens_never_fills_the_store() {
    let router = emulator();
    let service_arn = create_service(&router, "Cycled").await;

    // One more cycle than the per-service ceiling: if a consumed token outlived the
    // function it created, the index would stand at the ceiling by now and this create
    // would be refused `ServiceQuotaExceededException` instead of served.
    for cycle in 0..=MAX_SERVICE_FUNCTIONS_PER_SERVICE {
        let (status, created) = create_function(
            &router,
            json!({
                "serviceArn": service_arn,
                "name": "Cycled-Function",
                "criticality": CRITICALITY_VALUES[cycle % CRITICALITY_VALUES.len()],
                "clientToken": format!("cycle-{cycle}"),
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case cycle {cycle}: expected the create to be served, got {status} {created:?}"
        );
        let service_function_id = created["serviceFunction"]["serviceFunctionId"]
            .as_str()
            .unwrap_or_else(|| panic!("case cycle {cycle}: no serviceFunctionId in {created:?}"))
            .to_string();
        let (status, deleted) = call(
            &router,
            "POST",
            DELETE_SERVICE_FUNCTION,
            Some(json!({
                "serviceArn": service_arn,
                "serviceFunctionId": service_function_id,
            })),
        )
        .await;
        assert_eq!(
            status, 200,
            "case cycle {cycle}: expected the delete to be served, got {status} {deleted:?}"
        );
    }

    let (status, body) = list_functions(&router, &format!("serviceArn={service_arn}")).await;
    let functions = listed("after the cycles", status, &body);
    assert!(
        functions.is_empty(),
        "case after the cycles: expected no function left, got {functions:?}"
    );
}

#[tokio::test]
async fn a_listing_pages_in_creation_order_and_refuses_a_token_it_did_not_issue() {
    let router = emulator();
    let service_arn = create_service(&router, "Paged").await;

    let names: Vec<String> = (0..7).map(|index| format!("Function {index}")).collect();
    for (index, name) in names.iter().enumerate() {
        let (status, body) = create_function(
            &router,
            json!({
                "serviceArn": service_arn,
                "name": name,
                "criticality": CRITICALITY_VALUES[index % CRITICALITY_VALUES.len()],
            }),
        )
        .await;
        assert_eq!(status, 200, "creating {name}: got {status} {body:?}");
    }

    // The rows differ along the page size: each modelled bound of `maxResults`
    // (`range 1..100`), a size that divides the set, one that does not, and the default
    // (no `maxResults` at all).
    let sizes: [(&str, Option<i32>); 5] = [
        ("the minimum page size", Some(MAX_RESULTS_MIN)),
        ("a page size that divides the set", Some(3)),
        ("a page size that does not divide the set", Some(4)),
        ("the maximum page size", Some(MAX_RESULTS_MAX)),
        ("the default page size", None),
    ];

    for (case, max_results) in sizes {
        let mut seen: Vec<String> = Vec::new();
        let mut next: Option<String> = None;
        let mut pages = 0usize;
        loop {
            let mut query = format!("serviceArn={service_arn}");
            if let Some(size) = max_results {
                query.push_str(&format!("&maxResults={size}"));
            }
            if let Some(token) = next.as_deref() {
                query.push_str(&format!("&nextToken={token}"));
            }
            let (status, body) = list_functions(&router, &query).await;
            let page = listed(case, status, &body);
            if let Some(size) = max_results {
                assert!(
                    page.len() <= size as usize,
                    "case {case:?}: a page must not exceed maxResults={size}, got {page:?}"
                );
            }
            for function in &page {
                seen.push(
                    function["name"]
                        .as_str()
                        .unwrap_or_else(|| panic!("case {case:?}: no name in {function:?}"))
                        .to_string(),
                );
            }
            pages += 1;
            assert!(
                pages <= names.len() + 1,
                "case {case:?}: paging did not terminate, saw {seen:?}"
            );
            next = body
                .get("nextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            if next.is_none() {
                break;
            }
        }
        assert_eq!(
            seen, names,
            "case {case:?}: paging must enumerate every function exactly once in creation \
             order"
        );
    }

    // A `nextToken` this service did not issue is a bad request, not an empty page. The
    // rows span a token that is not this service's encoding at all and one that is
    // well-formed but names a position never handed out ("999999" hex-encoded, which is
    // how this service encodes a page position).
    for (case, token) in [
        ("not this service's encoding", "not-a-token"),
        ("a position never handed out", "393939393939"),
    ] {
        let (status, body) = list_functions(
            &router,
            &format!("serviceArn={service_arn}&nextToken={token}"),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for a forged nextToken, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {body:?}"
        );
    }

    // And a `maxResults` outside the modelled range is refused rather than served at some
    // other size — each published bound is tested, and one past it on each side.
    for (case, size) in [
        ("one below the minimum", MAX_RESULTS_MIN - 1),
        ("one above the maximum", MAX_RESULTS_MAX + 1),
    ] {
        let (status, body) = list_functions(
            &router,
            &format!("serviceArn={service_arn}&maxResults={size}"),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for maxResults={size}, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {body:?}"
        );
    }
}

#[tokio::test]
async fn deleting_a_service_cascades_to_the_functions_it_holds() {
    let router = emulator();

    // The rows differ along how many functions the deleted service held.
    for (case, function_count) in [
        ("a service with no functions", 0usize),
        ("a service with one function", 1),
        ("a service with several functions", 4),
    ] {
        let doomed = create_service(&router, &format!("Doomed{function_count}")).await;
        let bystander = create_service(&router, &format!("Bystander{function_count}")).await;
        for index in 0..function_count {
            for service_arn in [&doomed, &bystander] {
                let (status, body) = create_function(
                    &router,
                    json!({
                        "serviceArn": service_arn,
                        "name": format!("Function {index}"),
                        "criticality": CRITICALITY_VALUES[index % CRITICALITY_VALUES.len()],
                    }),
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: creating function {index}: got {status} {body:?}"
                );
            }
        }

        let (status, body) = delete_service(&router, &doomed).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DeleteService, got {status} {body:?}"
        );

        // The deleted service's functions went with it: there is no service left to list
        // them under.
        let (status, body) = list_functions(&router, &format!("serviceArn={doomed}")).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected HTTP 404 listing a deleted service's functions, got \
             {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {body:?}"
        );

        // And the cascade reached only that service: the bystander keeps every function.
        let (status, body) = list_functions(&router, &format!("serviceArn={bystander}")).await;
        let functions = listed(case, status, &body);
        assert_eq!(
            functions.len(),
            function_count,
            "case {case:?}: an unrelated service must keep its functions, got {functions:?}"
        );

        // A service recreated under the deleted one's name starts with no functions, so
        // nothing leaked out of the removed record.
        let reborn = create_service(&router, &format!("Doomed{function_count}")).await;
        let (status, body) = list_functions(&router, &format!("serviceArn={reborn}")).await;
        let functions = listed(case, status, &body);
        assert!(
            functions.is_empty(),
            "case {case:?}: a freshly created service holds no functions, got {functions:?}"
        );
    }
}

/// A service's resources are DISCOVERED through its input sources, so a service with no input
/// source has discovered none: `CreateServiceFunctionResources` refuses every identifier with
/// the modelled `ValidationException` and associates nothing.
///
/// The rows differ along the FORM the identifier takes — an ARN, a bare name, a resource url —
/// and along how many identifiers one call names, since the refusal names the first the
/// service has not discovered.
#[tokio::test]
async fn a_resource_the_service_never_discovered_is_refused_and_associates_nothing() {
    let router = emulator();
    let service_arn = create_service(&router, "UndiscoveredResources").await;
    let function_id = create_function_id(&router, &service_arn, "Checkout").await;
    let created = listed_function(&router, "before any association", &service_arn, &function_id)
        .await;

    for (case, resources, refused) in [
        (
            "an ARN",
            vec!["arn:aws:s3:::rhv2-undiscovered-bucket".to_string()],
            "arn:aws:s3:::rhv2-undiscovered-bucket",
        ),
        (
            "a bare resource name",
            vec!["rhv2-undiscovered-table".to_string()],
            "rhv2-undiscovered-table",
        ),
        (
            "a resource url",
            vec!["https://sqs.us-east-1.amazonaws.com/000000000000/rhv2-queue".to_string()],
            "https://sqs.us-east-1.amazonaws.com/000000000000/rhv2-queue",
        ),
        (
            "several identifiers, none of them discovered",
            vec![
                "rhv2-first-undiscovered".to_string(),
                "rhv2-second-undiscovered".to_string(),
            ],
            "rhv2-first-undiscovered",
        ),
    ] {
        let (status, body) = associate_resources(&router, &service_arn, &function_id, &resources)
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for a resource the service never discovered, got \
             {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {body:?}"
        );
        let expected = format!(
            "Resource not found: {refused}. Resource must be discovered before it can be added \
             to a function."
        );
        assert_eq!(
            body["message"].as_str(),
            Some(expected.as_str()),
            "case {case:?}: expected {expected:?}, got {:?}",
            body["message"]
        );
        assert_eq!(
            body["reason"].as_str(),
            Some("INVALID_FIELD_VALUE"),
            "case {case:?}: the refusal reports the value as invalid, got {body:?}"
        );

        // Nothing was written: the function still counts no resources, and the record was
        // not touched at all, so its `updatedAt` is still its `createdAt`.
        let function = listed_function(&router, case, &service_arn, &function_id).await;
        assert_eq!(
            function["resourceCount"].as_i64(),
            Some(0),
            "case {case:?}: a refused association stores nothing, got {function:?}"
        );
        assert_eq!(
            function["updatedAt"], created["updatedAt"],
            "case {case:?}: a refused association must not touch the record, got {function:?}"
        );
    }
}

/// The resources of a service function are a SET of identifiers, and `resourceCount` is that
/// set's size: an identifier named twice, or named again by a later call, is associated once
/// and counted once, while a new one is added to what is already held.
///
/// The service here discovers through an input source whose resources nothing in this emulator
/// holds (tag-based discovery), so its resource set is unknown and no identifier is refused
/// for being outside it — which is what leaves these rows about the set itself.
#[tokio::test]
async fn associating_resources_keeps_them_as_a_set_and_counts_them_once() {
    let router = emulator();
    let service_arn = create_service(&router, "AssociatedResources").await;
    create_input_source(
        &router,
        &service_arn,
        json!({"resourceTags": [{"key": "Env", "values": ["prod"]}]}),
    )
    .await;
    let function_id = create_function_id(&router, &service_arn, "Checkout").await;
    let first = "arn:aws:s3:::rhv2-associated-bucket".to_string();
    let second = "rhv2-associated-table".to_string();

    for (case, resources, associated, expected_count) in [
        ("the first association", vec![first.clone()], vec![first.clone()], 1),
        (
            "the same identifier again",
            vec![first.clone()],
            vec![first.clone()],
            1,
        ),
        (
            "one identifier named twice in a call",
            vec![second.clone(), second.clone()],
            vec![second.clone()],
            2,
        ),
        (
            "both, one of them already held",
            vec![first.clone(), second.clone()],
            vec![first.clone(), second.clone()],
            2,
        ),
    ] {
        let (status, body) = associate_resources(&router, &service_arn, &function_id, &resources)
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} {body:?}"
        );
        assert_eq!(
            body["serviceArn"].as_str(),
            Some(service_arn.as_str()),
            "case {case:?}: the answer names the service the request did, got {body:?}"
        );
        assert_eq!(
            body["serviceFunctionId"].as_str(),
            Some(function_id.as_str()),
            "case {case:?}: the answer names the function the request did, got {body:?}"
        );
        assert_eq!(
            body["resources"],
            json!(associated),
            "case {case:?}: the answer reports the resources it associated, each once, got \
             {body:?}"
        );

        let function = listed_function(&router, case, &service_arn, &function_id).await;
        assert_eq!(
            function["resourceCount"].as_i64(),
            Some(expected_count),
            "case {case:?}: expected resourceCount {expected_count}, got {function:?}"
        );
    }
}

/// The resources are associated WITH a function OF a service, so a request naming either of
/// them wrongly is the modelled 404 about that resource — and an identifier of ANOTHER
/// service's function is no identifier of this one's.
#[tokio::test]
async fn associating_resources_resolves_the_service_and_the_function_first() {
    let router = emulator();
    let service_arn = create_service(&router, "ParentResolution").await;
    let function_id = create_function_id(&router, &service_arn, "Checkout").await;
    let other_service = create_service(&router, "OtherParent").await;
    let other_function = create_function_id(&router, &other_service, "Checkout").await;
    let missing_service = "arn:aws:resiliencehub:us-east-1:000000000000:service/absent";

    for (case, service, function, expected) in [
        (
            "a serviceArn naming no service",
            missing_service.to_string(),
            function_id.clone(),
            "Service not found: service/absent".to_string(),
        ),
        (
            "a serviceFunctionId naming no function",
            service_arn.clone(),
            "absent-function".to_string(),
            "Service function not found: absent-function".to_string(),
        ),
        (
            "a function of another service",
            service_arn.clone(),
            other_function.clone(),
            format!("Service function not found: {other_function}"),
        ),
    ] {
        let (status, body) =
            associate_resources(&router, &service, &function, &["rhv2-ghost".to_string()]).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected HTTP 404, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {body:?}"
        );
        assert_eq!(
            body["message"].as_str(),
            Some(expected.as_str()),
            "case {case:?}: expected {expected:?}, got {:?}",
            body["message"]
        );
    }

    // The other service's function was named, never written to.
    let untouched = listed_function(
        &router,
        "a function of another service",
        &other_service,
        &other_function,
    )
    .await;
    assert_eq!(
        untouched["resourceCount"].as_i64(),
        Some(0),
        "a refused association stores nothing anywhere, got {untouched:?}"
    );
}
