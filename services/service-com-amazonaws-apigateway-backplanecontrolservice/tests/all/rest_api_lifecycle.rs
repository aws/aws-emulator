// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the REST API resource lifecycle: `CreateRestApi`, `GetRestApi`,
//! `GetRestApis`, `DeleteRestApi`, `CreateResource`, `GetResource`, `GetResources`,
//! `PutMethod` and `GetMethod`.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns.
//!
//! The tests assert RULES over sets of inputs, not one recorded call: a child
//! resource's path over a table of path parts and nesting depths, pagination over a
//! table of page sizes, `minimumCompressionSize` over its published bounds and one past
//! each, and the HTTP verbs a method accepts over the whole catalog plus verbs outside
//! it.
//!
//! [`Gateway`], [`Answer`], [`create_api`] and [`string_member`] are this crate's one
//! wire-test harness, so a sibling suite (`api_key_lifecycle`) reuses them rather than
//! standing a second emulator up its own way.

use std::collections::BTreeSet;
use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_apigateway_backplanecontrolservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// One emulator with this service mounted on a fresh shared engine.
pub(crate) struct Gateway {
    /// `pub(crate)` so a sibling suite can dispatch a request this harness does not
    /// build — the `execute_api_data_plane` suite drives UNSIGNED requests to a stage
    /// invoke URL, which carry neither this harness's credential scope nor its host.
    pub(crate) router: MultiServiceRouter,
    /// The shared engine the service is mounted on, kept so a suite can read a record a
    /// call PERSISTED where no operation of this service reports it — the stage's
    /// authorizer-cache watermark the `flush_stage_authorizers_cache` suite checks, which
    /// `Stage` models no member for.
    engine: Arc<engine::InMemoryEngine>,
}

/// One answered request: the HTTP status, the `x-amzn-errortype` error code when the
/// answer is a failure, and the parsed JSON body (`Value::Null` for an empty body).
pub(crate) struct Answer {
    pub(crate) status: u16,
    pub(crate) error: Option<String>,
    pub(crate) body: Value,
}

impl Gateway {
    pub(crate) fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // The tests exercise the service, not SigV4: requests carry a credential scope
        // so the router can name the target service, but no real signature.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    /// The engine this emulator's service is mounted on, so a suite can reach the very
    /// store the handlers wrote through [`service_state`].
    ///
    /// [`service_state`]: service_com_amazonaws_apigateway_backplanecontrolservice::state::service_state
    pub(crate) fn engine(&self) -> &engine::InMemoryEngine {
        &self.engine
    }

    pub(crate) async fn call(&self, method: &str, uri: &str, body: Option<Value>) -> Answer {
        let payload = body.map(|value| value.to_string()).unwrap_or_default();
        self.call_raw(method, uri, &payload).await
    }

    /// The same request with the body sent VERBATIM, for an operation whose payload is a
    /// blob rather than a JSON structure (`PutRestApi`'s OpenAPI definition, which AWS
    /// documents as a JSON *or* YAML file).
    pub(crate) async fn call_raw(&self, method: &str, uri: &str, payload: &str) -> Answer {
        let payload = payload.to_owned();
        let request = http::Request::builder()
            .method(method)
            .uri(uri)
            .header("host", "apigateway.us-east-1.amazonaws.com")
            .header(
                "authorization",
                "AWS4-HMAC-SHA256 \
                 Credential=AKIAIOSFODNN7EXAMPLE/20260101/us-east-1/apigateway/aws4_request, \
                 SignedHeaders=host, Signature=0",
            )
            .header("content-type", "application/json")
            .body(Bytes::from(payload))
            .expect("the test request is well formed");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        Answer {
            status,
            error,
            body,
        }
    }

    /// Dispatch a request BUILT BY THE CALLER through the same router, answering the whole
    /// HTTP response.
    ///
    /// The `call` helpers above address the control plane — one host, one credential scope,
    /// a JSON body — which is every modelled operation. The `execute-api` DATA plane is
    /// addressed differently (the api's own invoke host, any verb, any headers, often no
    /// credential at all) and is asserted on more than the JSON body (its status, its
    /// content type, its headers), so `execute_api_data_plane` builds its own requests and
    /// reads the raw answer, through this one seam rather than a second emulator.
    pub(crate) async fn send(
        &self,
        request: http::Request<Bytes>,
    ) -> http::Response<Bytes> {
        self.router.dispatch(request).await
    }
}

/// One emulator with this service AND AWS IAM mounted on ONE shared engine.
///
/// The account's CloudWatch role is resolved against IAM over the in-process transport, so
/// the service that answers that `GetRole` has to be mounted on the same router. Built on the
/// one constructor above, so this service is mounted and the router configured exactly once
/// in this target.
pub(crate) fn gateway_with_iam() -> Gateway {
    let mut gateway = Gateway::new();
    let engine = Arc::clone(&gateway.engine);
    gateway.router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(engine),
    );
    gateway
}

/// One emulator with this service, AWS IAM and Amazon CloudWatch Logs mounted on ONE shared
/// engine.
///
/// The account's CloudWatch role is resolved against IAM inside the `UpdateAccount` request,
/// and what a stage of that account then LOGS lands in CloudWatch Logs' own store, so a suite
/// that drives both halves needs all three services on one router. Built on
/// [`gateway_with_iam`], so there is still one place this service and IAM are mounted.
pub(crate) fn gateway_with_iam_and_logs() -> Gateway {
    let mut gateway = gateway_with_iam();
    let engine = Arc::clone(&gateway.engine);
    gateway.router.register_service(
        service_com_amazonaws_cloudwatchlogs_logs_20140328::EmulatedService::new(engine),
    );
    gateway
}

/// One emulator with this service, AWS IAM, Amazon CloudWatch Logs AND AWS Lambda mounted on
/// ONE shared engine.
///
/// The whole account-role sequence a client scripts around a stage's execution logging spans
/// all four: the role is resolved against IAM inside the `UpdateAccount` request, the method
/// the deployed stage serves is an `AWS_PROXY` integration Lambda runs, and what the stage
/// then logs lands in CloudWatch Logs' own store. Built on [`gateway_with_iam_and_logs`], so
/// there is still one place each of those services is mounted.
pub(crate) fn gateway_with_iam_logs_and_lambda() -> Gateway {
    let mut gateway = gateway_with_iam_and_logs();
    let engine = Arc::clone(&gateway.engine);
    gateway.router.register_service(
        service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(engine),
    );
    gateway
}

/// One emulator with this service, Amazon DynamoDB and AWS IAM mounted on ONE shared engine.
///
/// An `AWS` integration whose `uri` names a table action makes that call against the service
/// that OWNS the table, in process, under the role its `credentials` names — so a suite that
/// drives such an integration needs the table service to answer the call and IAM to hold the
/// role whose policies decide whether it is made. Built on the one constructor above, so this
/// service is mounted and the router configured exactly once in this target.
pub(crate) fn gateway_with_dynamodb_and_iam() -> Gateway {
    let mut gateway = Gateway::new();
    let engine = Arc::clone(&gateway.engine);
    gateway.router.register_service(
        service_com_amazonaws_dynamodb_dynamodb_20120810::EmulatedService::new(Arc::clone(&engine)),
    );
    gateway.router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(engine),
    );
    gateway
}

/// One emulator with this service, Amazon SNS, Amazon SQS and IAM mounted on ONE shared
/// engine.
///
/// An `AWS` integration naming an SNS topic action is PERFORMED by making that call against
/// the service that OWNS the topic, in process, under the role its `credentials` names — and
/// what the topic then does with a published message is its subscribers' business, so a suite
/// that drives such an integration needs the topic service to answer the call, the queue
/// service to hold what a subscribed queue was delivered, and IAM to hold the role whose
/// policies decide whether the call is made. Built on the one constructor above, so this
/// service is mounted and the router configured exactly once in this target.
pub(crate) fn gateway_with_sns_sqs_and_iam() -> Gateway {
    let mut gateway = Gateway::new();
    let engine = Arc::clone(&gateway.engine);
    gateway.router.register_service(
        service_com_amazonaws_sns_amazonsimplenotificationservice::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    gateway.router.register_service(
        service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(Arc::clone(&engine)),
    );
    gateway.router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(engine),
    );
    gateway
}

/// One emulator with this service AND Amazon EC2 mounted on ONE shared engine.
///
/// A domain name access association's `accessAssociationSource` is resolved against the VPC
/// endpoints the account has, and those are EC2's records, so the service that answers that
/// `DescribeVpcEndpoints` has to be mounted on the same router. Built on the one constructor
/// above, so this service is mounted and the router configured exactly once in this target.
pub(crate) fn gateway_with_ec2() -> Gateway {
    let mut gateway = Gateway::new();
    let engine = Arc::clone(&gateway.engine);
    gateway
        .router
        .register_service(service_com_amazonaws_ec2_amazonec2::EmulatedService::new(engine));
    gateway
}

/// One emulator with this service, Certificate Manager and Amazon S3 mounted on ONE shared
/// engine.
///
/// A REGIONAL custom domain name presents the certificate Certificate Manager holds for its
/// `regionalCertificateArn` and enforces the mutual-TLS truststore the object store holds at
/// its `truststoreUri`, so both services have to be mounted on the same router for a case to
/// bring either through its OWN API. Built on the one constructor above, so this service is
/// mounted and the router configured exactly once in this target.
pub(crate) fn gateway_with_acm_and_s3() -> Gateway {
    let mut gateway = Gateway::new();
    let engine = Arc::clone(&gateway.engine);
    gateway.router.register_service(
        service_com_amazonaws_acm_certificatemanager::EmulatedService::new(Arc::clone(&engine)),
    );
    gateway
        .router
        .register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(engine));
    gateway
}

/// `CreateRestApi` for `name`, returning the API id and its root resource id.
pub(crate) async fn create_api(gateway: &Gateway, name: &str) -> (String, String) {
    let answer = gateway
        .call("POST", "/restapis", Some(json!({ "name": name })))
        .await;
    assert_eq!(
        answer.status, 201,
        "case CreateRestApi({name:?}): expected 201, got {} with {:?}",
        answer.status, answer.body
    );
    let id = string_member(&answer.body, "id", &format!("CreateRestApi({name:?})"));
    let root = string_member(
        &answer.body,
        "rootResourceId",
        &format!("CreateRestApi({name:?})"),
    );
    (id, root)
}

/// `CreateResource` for `path_part` under `parent`, asserting it was accepted. Shared
/// with the sibling `patch_updates` and `delete_lifecycle` suites, which rename and
/// delete the resources they build.
pub(crate) async fn create_resource(
    gateway: &Gateway,
    api: &str,
    parent: &str,
    path_part: &str,
) -> Value {
    let answer = gateway
        .call(
            "POST",
            &format!("/restapis/{api}/resources/{parent}"),
            Some(json!({ "pathPart": path_part })),
        )
        .await;
    assert_eq!(
        answer.status, 201,
        "case CreateResource({path_part:?}): expected 201, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body
}

/// `CreateResource` for `path_part` under `parent`, answering the created resource's ID —
/// [`create_resource`] followed by the [`string_member`] read of its `id`.
///
/// This crate's one copy of that pair, shared by every suite that stands a resource up to
/// address it afterwards (`test_invoke_method`, `test_invoke_method_no_integration`,
/// `stage_invoke`), so none of them writes the read again.
pub(crate) async fn resource_under(
    gateway: &Gateway,
    api: &str,
    parent: &str,
    path_part: &str,
) -> String {
    let created = create_resource(gateway, api, parent, path_part).await;
    string_member(&created, "id", &format!("CreateResource({path_part})"))
}

/// Put one method on `api`'s ROOT resource, which is what makes the API DEPLOYABLE:
/// `CreateDeployment` refuses an API that carries no method, since a deployment is the
/// snapshot of the API's methods a stage serves.
///
/// The method goes on the root resource, which every API is created with, so nothing is
/// added to the API's resource TREE and a suite that asserts the tree is unaffected. This
/// crate's one copy of that step, shared by every suite that deploys an API only to have
/// a deployment or a stage to address.
pub(crate) async fn make_deployable(gateway: &Gateway, api: &str) {
    // Idempotent, so a suite may call it beside a helper that already does: a method is
    // only put when the API carries none at all, which is what `embed=methods` reports.
    let listed = gateway
        .call("GET", &format!("/restapis/{api}/resources?embed=methods"), None)
        .await;
    let carries_a_method = page_items(&listed.body, &format!("GetResources({api})"))
        .iter()
        .any(|resource| {
            resource["resourceMethods"]
                .as_object()
                .is_some_and(|methods| !methods.is_empty())
        });
    if carries_a_method {
        return;
    }
    let read = gateway.call("GET", &format!("/restapis/{api}"), None).await;
    let root = string_member(&read.body, "rootResourceId", &format!("GetRestApi({api})"));
    let answer = gateway
        .call(
            "PUT",
            &format!("/restapis/{api}/resources/{root}/methods/GET"),
            Some(json!({ "authorizationType": "NONE" })),
        )
        .await;
    assert_eq!(
        answer.status, 201,
        "case PutMethod on the root of {api}: expected 201, got {} with {:?}",
        answer.status, answer.body
    );
}

/// A required string member of a response body, with the case named on failure.
pub(crate) fn string_member(body: &Value, member: &str, case: &str) -> String {
    body[member]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: expected a string {member}, got {body:?}"))
        .to_owned()
}

/// The elements on one page of a paginated collection. The member's `jsonName` is
/// `item`, and the base-path-mapping collection of a domain name leaves it unset while the
/// domain name carries no mapping — so no element is reported as no element, whether the
/// member is absent or present and empty (every other collection's empty page).
pub(crate) fn page_items(body: &Value, case: &str) -> Vec<Value> {
    match body.get("item") {
        None | Some(Value::Null) => Vec::new(),
        Some(item) => item
            .as_array()
            .unwrap_or_else(|| panic!("case {case}: expected an item array, got {body:?}"))
            .clone(),
    }
}

/// `body` with `members` dropped, for comparing two reads of ONE record that may
/// legitimately differ in those members only — a status the clock owes, an id the service
/// minted, the instant the record last changed.
///
/// This crate's one copy of that strip, so a suite names the members its own rule excludes
/// instead of writing the same helper again (`without_status`, `without_cluster_status`
/// and `without_id` are thin wrappers over this).
pub(crate) fn without(body: &Value, members: &[&str]) -> Value {
    let mut copy = body.clone();
    if let Some(object) = copy.as_object_mut() {
        for member in members {
            object.remove(*member);
        }
    }
    copy
}

/// Assert `answer` is the modelled failure `(status, code)` — the status and the
/// `x-amzn-errortype` code, which is what a client's error handling keys off.
///
/// This crate's one modelled-failure assertion, so every suite checks a refusal the
/// same way.
pub(crate) fn assert_failure(answer: &Answer, status: u16, code: &str, case: &str) {
    assert_eq!(
        answer.status, status,
        "case {case}: expected {status}, got {} with {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error.as_deref(),
        Some(code),
        "case {case}: expected {code}, got {:?}",
        answer.error
    );
}

#[tokio::test]
async fn a_created_rest_api_is_persisted_and_read_back_member_for_member() {
    let gateway = Gateway::new();
    let request = json!({
        "name": "petstore",
        "description": "the pet store",
        "version": "2026-01-01",
        "binaryMediaTypes": ["image/png", "application/octet-stream"],
        "apiKeySource": "HEADER",
        "minimumCompressionSize": 1024,
        "policy": "{\"Version\":\"2012-10-17\"}",
        "endpointConfiguration": { "types": ["REGIONAL"] },
        "tags": { "team": "api", "env": "test" },
        "disableExecuteApiEndpoint": true,
    });
    let created = gateway.call("POST", "/restapis", Some(request)).await;
    assert_eq!(
        created.status, 201,
        "case CreateRestApi(full input): expected 201, got {} with {:?}",
        created.status, created.body
    );

    let id = string_member(&created.body, "id", "CreateRestApi(full input)");
    let root = string_member(&created.body, "rootResourceId", "CreateRestApi(full input)");
    assert_eq!(
        id.len(),
        10,
        "case CreateRestApi(full input): an API id is 10 characters, got {id:?}"
    );
    assert!(
        id.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()),
        "case CreateRestApi(full input): an API id is lowercase alphanumeric, got {id:?}"
    );
    assert_eq!(
        root.len(),
        10,
        "case CreateRestApi(full input): a root resource id is 10 characters, got {root:?}"
    );
    assert_ne!(
        id, root,
        "case CreateRestApi(full input): the API id and its root resource id must differ, both were {id:?}"
    );
    assert!(
        created.body["createdDate"].is_number(),
        "case CreateRestApi(full input): expected a createdDate timestamp, got {:?}",
        created.body["createdDate"]
    );

    // The read reports the persisted record, not a body synthesised from the request:
    // every member the create answered must come back identical.
    let fetched = gateway.call("GET", &format!("/restapis/{id}"), None).await;
    assert_eq!(
        fetched.status, 200,
        "case GetRestApi: expected 200, got {} with {:?}",
        fetched.status, fetched.body
    );
    for member in [
        "id",
        "name",
        "description",
        "version",
        "createdDate",
        "rootResourceId",
        "tags",
        "binaryMediaTypes",
        "apiKeySource",
        "minimumCompressionSize",
        "policy",
        "endpointConfiguration",
        "disableExecuteApiEndpoint",
        "apiStatus",
    ] {
        assert_eq!(
            fetched.body[member], created.body[member],
            "case GetRestApi({member}): expected the persisted {:?}, got {:?}",
            created.body[member], fetched.body[member]
        );
    }

    // The root resource is a real record: it is listed, it has no parent and no path
    // part, and its path is the API root.
    let resources = gateway
        .call("GET", &format!("/restapis/{id}/resources"), None)
        .await;
    let items = page_items(&resources.body, "GetResources(root only)");
    assert_eq!(
        items.len(),
        1,
        "case GetResources(root only): a new API has exactly its root resource, got {items:?}"
    );
    assert_eq!(
        items[0]["id"].as_str(),
        Some(root.as_str()),
        "case GetResources(root only): expected the rootResourceId {root:?}, got {:?}",
        items[0]["id"]
    );
    assert_eq!(
        items[0]["path"].as_str(),
        Some("/"),
        "case GetResources(root only): the root path is \"/\", got {:?}",
        items[0]["path"]
    );
    assert!(
        items[0]["parentId"].is_null() && items[0]["pathPart"].is_null(),
        "case GetResources(root only): the root resource has no parentId or pathPart, got {:?}",
        items[0]
    );
}

#[tokio::test]
async fn a_child_resources_path_is_its_parents_path_plus_its_own_path_part() {
    // Rows differ along what the rule generalises over: the form of the path part and
    // how deep the resource sits under the root.
    let cases: &[&[&str]] = &[
        &["pets"],
        &["pets", "{petId}"],
        &["a", "b", "c", "d"],
        &["{proxy+}"],
        &["v2.0"],
        &["order_items"],
        &["Mixed-Case"],
        &["%7Bid%7D"],
    ];

    let gateway = Gateway::new();
    for parts in cases {
        let case = format!("{parts:?}");
        let (api, root) = create_api(&gateway, &format!("api-{}", parts.join("-"))).await;
        let mut parent = root.clone();
        let mut expected_path = String::new();
        for part in *parts {
            expected_path = format!("{expected_path}/{part}");
            let body = create_resource(&gateway, &api, &parent, part).await;
            assert_eq!(
                body["path"].as_str(),
                Some(expected_path.as_str()),
                "case {case} at {part:?}: expected path {expected_path:?}, got {:?}",
                body["path"]
            );
            assert_eq!(
                body["parentId"].as_str(),
                Some(parent.as_str()),
                "case {case} at {part:?}: expected parentId {parent:?}, got {:?}",
                body["parentId"]
            );
            assert_eq!(
                body["pathPart"].as_str(),
                Some(*part),
                "case {case} at {part:?}: expected pathPart {part:?}, got {:?}",
                body["pathPart"]
            );
            parent = string_member(&body, "id", &case);

            // Round trip: the reader reports the same record the create answered.
            let read = gateway
                .call(
                    "GET",
                    &format!("/restapis/{api}/resources/{parent}"),
                    None,
                )
                .await;
            assert_eq!(
                read.status, 200,
                "case {case} at {part:?}: GetResource expected 200, got {} with {:?}",
                read.status, read.body
            );
            for member in ["id", "parentId", "pathPart", "path"] {
                assert_eq!(
                    read.body[member], body[member],
                    "case {case} at {part:?}: GetResource must report the persisted {member}: expected {:?}, got {:?}",
                    body[member], read.body[member]
                );
            }
        }
    }
}

#[tokio::test]
async fn every_minted_id_is_distinct_and_lowercase_alphanumeric_at_its_records_width() {
    // The input space of the id rule is the creation sequence itself, so the loop
    // enumerates it exhaustively rather than sampling: 60 APIs, each with a root
    // resource, plus a child resource apiece — 180 minted ids. An API and the root
    // resource it is created with are ten characters wide, and a resource created inside
    // the API is six, so each id is checked at the width its record carries.
    let gateway = Gateway::new();
    let mut minted: BTreeSet<String> = BTreeSet::new();
    for n in 0..60 {
        let (api, root) = create_api(&gateway, &format!("api-{n}")).await;
        let child = create_resource(&gateway, &api, &root, "child").await;
        let child_id = string_member(&child, "id", &format!("resource of api-{n}"));
        for (record, width, id) in [
            ("the API", 10, api),
            ("its root resource", 10, root),
            ("its child resource", 6, child_id),
        ] {
            assert_eq!(
                id.len(),
                width,
                "case api-{n}: the id of {record} is {width} characters, got {id:?}"
            );
            assert!(
                id.chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()),
                "case api-{n}: every minted id is lowercase alphanumeric, got {id:?}"
            );
            assert!(
                minted.insert(id.clone()),
                "case api-{n}: the id {id:?} was minted twice"
            );
        }
    }
    assert_eq!(
        minted.len(),
        180,
        "expected 180 distinct ids, got {}",
        minted.len()
    );
}

#[tokio::test]
async fn get_rest_apis_pages_through_creation_order_on_position_and_limit() {
    let gateway = Gateway::new();
    let mut created = Vec::new();
    for n in 0..7 {
        let (id, _) = create_api(&gateway, &format!("api-{n}")).await;
        created.push(id);
    }

    let all = gateway.call("GET", "/restapis", None).await;
    assert_eq!(
        all.status, 200,
        "case GetRestApis(no limit): expected 200, got {} with {:?}",
        all.status, all.body
    );
    let listed: Vec<String> = page_items(&all.body, "GetRestApis(no limit)")
        .iter()
        .map(|item| string_member(item, "id", "GetRestApis(no limit)"))
        .collect();
    assert_eq!(
        listed, created,
        "case GetRestApis(no limit): expected the APIs in creation order {created:?}, got {listed:?}"
    );
    assert_eq!(
        all.body["position"].as_str(),
        None,
        "case GetRestApis(no limit): a page that holds the whole collection reports no \
         cursor, got {:?}",
        all.body["position"]
    );

    // Rows span the documented page-size range (default 25, maximum 500), its bounds,
    // and one past each bound — a size below 1 and a size above 500 are both clamped
    // into the range rather than refused.
    for limit in [0, 1, 2, 3, 7, 8, 25, 500, 501, 10_000] {
        let case = format!("limit={limit}");
        let effective = limit.clamp(1, 500) as usize;
        let mut seen: Vec<String> = Vec::new();
        let mut position: Option<String> = None;
        let mut pages = 0;
        loop {
            let uri = match &position {
                None => format!("/restapis?limit={limit}"),
                Some(cursor) => format!("/restapis?limit={limit}&position={cursor}"),
            };
            let answer = gateway.call("GET", &uri, None).await;
            assert_eq!(
                answer.status, 200,
                "case {case}: expected 200, got {} with {:?}",
                answer.status, answer.body
            );
            let items = page_items(&answer.body, &case);
            assert!(
                items.len() <= effective,
                "case {case}: a page holds at most {effective} items, got {}",
                items.len()
            );
            seen.extend(
                items
                    .iter()
                    .map(|item| string_member(item, "id", &case)),
            );
            pages += 1;
            assert!(pages <= 20, "case {case}: pagination did not terminate");
            match answer.body["position"].as_str() {
                None => break,
                Some(cursor) => position = Some(cursor.to_owned()),
            }
        }
        assert_eq!(
            seen, created,
            "case {case}: paging must reassemble creation order {created:?}, got {seen:?}"
        );
    }

    // A cursor naming nothing in the sequence is a bad request, not an empty page.
    let bogus = gateway
        .call("GET", "/restapis?position=nosuchcursor", None)
        .await;
    assert_eq!(
        bogus.status, 400,
        "case GetRestApis(unknown position): expected 400, got {} with {:?}",
        bogus.status, bogus.body
    );
    assert_eq!(
        bogus.error.as_deref(),
        Some("BadRequestException"),
        "case GetRestApis(unknown position): expected BadRequestException, got {:?}",
        bogus.error
    );
}

#[tokio::test]
async fn get_resources_pages_through_creation_order_with_the_root_first() {
    let gateway = Gateway::new();
    let (api, root) = create_api(&gateway, "petstore").await;
    let mut expected = vec![root.clone()];
    for part in ["pets", "owners", "vets", "clinics"] {
        let body = create_resource(&gateway, &api, &root, part).await;
        expected.push(string_member(&body, "id", part));
    }

    for limit in [1, 2, 5, 25, 500] {
        let case = format!("GetResources(limit={limit})");
        let mut seen: Vec<String> = Vec::new();
        let mut position: Option<String> = None;
        loop {
            let uri = match &position {
                None => format!("/restapis/{api}/resources?limit={limit}"),
                Some(cursor) => {
                    format!("/restapis/{api}/resources?limit={limit}&position={cursor}")
                }
            };
            let answer = gateway.call("GET", &uri, None).await;
            assert_eq!(
                answer.status, 200,
                "case {case}: expected 200, got {} with {:?}",
                answer.status, answer.body
            );
            let items = page_items(&answer.body, &case);
            assert!(
                items.len() <= limit,
                "case {case}: a page holds at most {limit} items, got {}",
                items.len()
            );
            seen.extend(items.iter().map(|item| string_member(item, "id", &case)));
            match answer.body["position"].as_str() {
                None => break,
                Some(cursor) => position = Some(cursor.to_owned()),
            }
        }
        assert_eq!(
            seen, expected,
            "case {case}: expected the root resource first then creation order {expected:?}, got {seen:?}"
        );
    }
}

#[tokio::test]
async fn minimum_compression_size_is_accepted_only_within_its_published_bounds() {
    // Source: the `minimumCompressionSize` member's documentation — "non-negative
    // between 0 and 10485760 (10M) bytes, inclusive". Both bounds are tested, and one
    // value past each.
    let cases: &[(i64, bool)] = &[
        (-1, false),
        (0, true),
        (1, true),
        (65_536, true),
        (10_485_759, true),
        (10_485_760, true),
        (10_485_761, false),
        (20_000_000, false),
    ];

    let gateway = Gateway::new();
    for (size, accepted) in cases {
        let case = format!("minimumCompressionSize={size}");
        let answer = gateway
            .call(
                "POST",
                "/restapis",
                Some(json!({ "name": format!("api{size}"), "minimumCompressionSize": size })),
            )
            .await;
        if *accepted {
            assert_eq!(
                answer.status, 201,
                "case {case}: expected 201, got {} with {:?}",
                answer.status, answer.body
            );
            let id = string_member(&answer.body, "id", &case);
            let read = gateway.call("GET", &format!("/restapis/{id}"), None).await;
            assert_eq!(
                read.body["minimumCompressionSize"].as_i64(),
                Some(*size),
                "case {case}: the persisted value must read back, got {:?}",
                read.body["minimumCompressionSize"]
            );
        } else {
            assert_eq!(
                answer.status, 400,
                "case {case}: expected 400, got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error.as_deref(),
                Some("BadRequestException"),
                "case {case}: expected BadRequestException, got {:?}",
                answer.error
            );
        }
    }
}

#[tokio::test]
async fn a_method_is_stored_per_http_verb_and_refused_outside_the_verb_catalog() {
    // Source: the HTTP verbs API Gateway accepts for a method, plus its `ANY`
    // catch-all. `httpMethod` is modelled as a bare String, so the catalog is the
    // service's, not the model's. The rejected rows are verbs outside it, including two
    // real HTTP verbs API Gateway does not serve.
    let accepted = [
        "ANY", "DELETE", "GET", "HEAD", "OPTIONS", "PATCH", "POST", "PUT",
    ];
    let rejected = ["TRACE", "CONNECT", "BREW", "GETS"];

    let gateway = Gateway::new();
    let (api, root) = create_api(&gateway, "verbs").await;
    let resource = create_resource(&gateway, &api, &root, "pets").await;
    let resource_id = string_member(&resource, "id", "verbs/pets");

    for verb in accepted {
        let case = format!("PutMethod({verb})");
        let answer = gateway
            .call(
                "PUT",
                &format!("/restapis/{api}/resources/{resource_id}/methods/{verb}"),
                Some(json!({
                    "authorizationType": "NONE",
                    "operationName": format!("op{verb}"),
                    "requestParameters": { "method.request.querystring.q": true },
                })),
            )
            .await;
        assert_eq!(
            answer.status, 201,
            "case {case}: expected 201, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.body["httpMethod"].as_str(),
            Some(verb),
            "case {case}: expected httpMethod {verb:?}, got {:?}",
            answer.body["httpMethod"]
        );
        assert_eq!(
            answer.body["apiKeyRequired"].as_bool(),
            Some(false),
            "case {case}: apiKeyRequired defaults to false, got {:?}",
            answer.body["apiKeyRequired"]
        );

        // Round trip through the reader: the stored method, not the echoed request.
        let read = gateway
            .call(
                "GET",
                &format!("/restapis/{api}/resources/{resource_id}/methods/{verb}"),
                None,
            )
            .await;
        assert_eq!(
            read.status, 200,
            "case {case}: GetMethod expected 200, got {} with {:?}",
            read.status, read.body
        );
        for member in [
            "httpMethod",
            "authorizationType",
            "operationName",
            "apiKeyRequired",
            "requestParameters",
        ] {
            assert_eq!(
                read.body[member], answer.body[member],
                "case {case}: GetMethod must report the persisted {member}: expected {:?}, got {:?}",
                answer.body[member], read.body[member]
            );
        }

        // Re-putting the same verb is a conflict, not an overwrite.
        let again = gateway
            .call(
                "PUT",
                &format!("/restapis/{api}/resources/{resource_id}/methods/{verb}"),
                Some(json!({ "authorizationType": "AWS_IAM" })),
            )
            .await;
        assert_eq!(
            again.status, 409,
            "case {case} re-put: expected 409, got {} with {:?}",
            again.status, again.body
        );
        assert_eq!(
            again.error.as_deref(),
            Some("ConflictException"),
            "case {case} re-put: expected ConflictException, got {:?}",
            again.error
        );
        let unchanged = gateway
            .call(
                "GET",
                &format!("/restapis/{api}/resources/{resource_id}/methods/{verb}"),
                None,
            )
            .await;
        assert_eq!(
            unchanged.body["authorizationType"].as_str(),
            Some("NONE"),
            "case {case} re-put: a refused put must not change the stored method, got {:?}",
            unchanged.body["authorizationType"]
        );
    }

    // The verb is case-insensitive on the wire and names one method on the resource,
    // so a lowercase spelling resolves to the method already stored for that verb.
    let lowercase = gateway
        .call(
            "GET",
            &format!("/restapis/{api}/resources/{resource_id}/methods/get"),
            None,
        )
        .await;
    assert_eq!(
        lowercase.body["httpMethod"].as_str(),
        Some("GET"),
        "case GetMethod(get): expected the stored GET method, got {:?}",
        lowercase.body
    );

    for verb in rejected {
        let case = format!("PutMethod({verb})");
        let answer = gateway
            .call(
                "PUT",
                &format!("/restapis/{api}/resources/{resource_id}/methods/{verb}"),
                Some(json!({ "authorizationType": "NONE" })),
            )
            .await;
        assert_eq!(
            answer.status, 400,
            "case {case}: expected 400, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error.as_deref(),
            Some("BadRequestException"),
            "case {case}: expected BadRequestException, got {:?}",
            answer.error
        );
    }
}

#[tokio::test]
async fn resource_methods_carries_the_embedded_method_only_when_embed_asks_for_it() {
    let gateway = Gateway::new();
    let (api, root) = create_api(&gateway, "embedding").await;
    let resource = create_resource(&gateway, &api, &root, "pets").await;
    let resource_id = string_member(&resource, "id", "embedding/pets");
    assert!(
        resource["resourceMethods"].is_null(),
        "case CreateResource: a new resource reports no resourceMethods, got {:?}",
        resource["resourceMethods"]
    );
    let put = gateway
        .call(
            "PUT",
            &format!("/restapis/{api}/resources/{resource_id}/methods/GET"),
            Some(json!({ "authorizationType": "AWS_IAM", "apiKeyRequired": true })),
        )
        .await;
    assert_eq!(
        put.status, 201,
        "case PutMethod(GET): expected 201, got {} with {:?}",
        put.status, put.body
    );

    // Rows differ along the `embed` query parameter, which documents the embedding of
    // the `Method` resources as its only supported value.
    for (embed, expect_embedded) in [("", false), ("?embed=methods", true)] {
        let case = format!("GetResources({embed:?})");
        let answer = gateway
            .call(
                "GET",
                &format!("/restapis/{api}/resources{embed}"),
                None,
            )
            .await;
        let items = page_items(&answer.body, &case);
        let pets = items
            .iter()
            .find(|item| item["id"].as_str() == Some(resource_id.as_str()))
            .unwrap_or_else(|| panic!("case {case}: /pets must be listed, got {items:?}"));
        assert!(
            pets["resourceMethods"]["GET"].is_object(),
            "case {case}: resourceMethods must be keyed by the stored verb, got {:?}",
            pets["resourceMethods"]
        );
        let embedded = pets["resourceMethods"]["GET"]["httpMethod"].as_str();
        if expect_embedded {
            assert_eq!(
                embedded,
                Some("GET"),
                "case {case}: the embedded method must carry the stored record, got {:?}",
                pets["resourceMethods"]["GET"]
            );
            assert_eq!(
                pets["resourceMethods"]["GET"]["authorizationType"].as_str(),
                Some("AWS_IAM"),
                "case {case}: expected the stored authorizationType, got {:?}",
                pets["resourceMethods"]["GET"]
            );
        } else {
            assert_eq!(
                embedded, None,
                "case {case}: without embed only the verb is reported, got {:?}",
                pets["resourceMethods"]["GET"]
            );
        }
    }
}

#[tokio::test]
async fn a_duplicate_path_part_under_one_parent_is_a_conflict() {
    let gateway = Gateway::new();
    let (api, root) = create_api(&gateway, "conflicts").await;

    // Rows differ in where the duplicate sits: directly under the root, and under a
    // nested parent. A sibling with a different part, and the same part under a
    // DIFFERENT parent, are both accepted.
    let first = create_resource(&gateway, &api, &root, "pets").await;
    let pets = string_member(&first, "id", "conflicts/pets");
    let nested = create_resource(&gateway, &api, &pets, "pets").await;
    assert_eq!(
        nested["path"].as_str(),
        Some("/pets/pets"),
        "case same part under a different parent: expected /pets/pets, got {:?}",
        nested["path"]
    );
    create_resource(&gateway, &api, &root, "owners").await;

    for parent in [root.as_str(), pets.as_str()] {
        let case = format!("CreateResource(duplicate pets under {parent})");
        let answer = gateway
            .call(
                "POST",
                &format!("/restapis/{api}/resources/{parent}"),
                Some(json!({ "pathPart": "pets" })),
            )
            .await;
        assert_eq!(
            answer.status, 409,
            "case {case}: expected 409, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error.as_deref(),
            Some("ConflictException"),
            "case {case}: expected ConflictException, got {:?}",
            answer.error
        );
        // The refusal is built from the request: it names the segment that collides.
        assert_eq!(
            answer.body["message"].as_str(),
            Some("Another resource with the same parent already has this name: pets"),
            "case {case}: the refusal must name the colliding segment, got {:?}",
            answer.body["message"]
        );
    }

    // A different colliding segment is named in its own refusal: the message follows
    // the request, it is not one fixed sentence.
    let owners = gateway
        .call(
            "POST",
            &format!("/restapis/{api}/resources/{root}"),
            Some(json!({ "pathPart": "owners" })),
        )
        .await;
    assert_failure(
        &owners,
        409,
        "ConflictException",
        "CreateResource(duplicate owners under the root)",
    );
    assert_eq!(
        owners.body["message"].as_str(),
        Some("Another resource with the same parent already has this name: owners"),
        "case CreateResource(duplicate owners under the root): the refusal must name the colliding segment, got {:?}",
        owners.body["message"]
    );

    // An empty path part names no segment at all.
    let empty = gateway
        .call(
            "POST",
            &format!("/restapis/{api}/resources/{root}"),
            Some(json!({ "pathPart": "" })),
        )
        .await;
    assert_eq!(
        empty.status, 400,
        "case CreateResource(empty pathPart): expected 400, got {} with {:?}",
        empty.status, empty.body
    );

    // The refused calls left nothing behind.
    let listed = gateway
        .call("GET", &format!("/restapis/{api}/resources"), None)
        .await;
    let items = page_items(&listed.body, "GetResources after conflicts");
    assert_eq!(
        items.len(),
        4,
        "case GetResources after conflicts: expected root, /pets, /pets/pets and /owners, got {items:?}"
    );
}

#[tokio::test]
async fn an_unknown_identifier_is_a_not_found_on_every_operation_that_resolves_one() {
    let gateway = Gateway::new();
    let (api, root) = create_api(&gateway, "notfound").await;
    let resource = create_resource(&gateway, &api, &root, "pets").await;
    let resource_id = string_member(&resource, "id", "notfound/pets");
    let missing = "zzzzzzzzzz";

    // Rows span every operation of the slice that resolves an identifier, and for each
    // the identifier it resolves.
    let cases: Vec<(&str, String, Option<Value>)> = vec![
        ("GET", format!("/restapis/{missing}"), None),
        ("DELETE", format!("/restapis/{missing}"), None),
        ("GET", format!("/restapis/{missing}/resources"), None),
        (
            "GET",
            format!("/restapis/{missing}/resources/{root}"),
            None,
        ),
        (
            "GET",
            format!("/restapis/{api}/resources/{missing}"),
            None,
        ),
        (
            "POST",
            format!("/restapis/{missing}/resources/{root}"),
            Some(json!({ "pathPart": "x" })),
        ),
        (
            "POST",
            format!("/restapis/{api}/resources/{missing}"),
            Some(json!({ "pathPart": "x" })),
        ),
        (
            "PUT",
            format!("/restapis/{missing}/resources/{root}/methods/GET"),
            Some(json!({ "authorizationType": "NONE" })),
        ),
        (
            "PUT",
            format!("/restapis/{api}/resources/{missing}/methods/GET"),
            Some(json!({ "authorizationType": "NONE" })),
        ),
        (
            "GET",
            format!("/restapis/{api}/resources/{resource_id}/methods/GET"),
            None,
        ),
    ];

    for (method, uri, body) in cases {
        let case = format!("{method} {uri}");
        let answer = gateway.call(method, &uri, body).await;
        assert_eq!(
            answer.status, 404,
            "case {case}: expected 404, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error.as_deref(),
            Some("NotFoundException"),
            "case {case}: expected NotFoundException, got {:?}",
            answer.error
        );
    }
}

#[tokio::test]
async fn deleting_a_rest_api_removes_it_and_its_whole_resource_tree() {
    let gateway = Gateway::new();
    let (kept, _) = create_api(&gateway, "kept").await;
    let (doomed, root) = create_api(&gateway, "doomed").await;
    let resource = create_resource(&gateway, &doomed, &root, "pets").await;
    let resource_id = string_member(&resource, "id", "doomed/pets");
    gateway
        .call(
            "PUT",
            &format!("/restapis/{doomed}/resources/{resource_id}/methods/GET"),
            Some(json!({ "authorizationType": "NONE" })),
        )
        .await;

    let deleted = gateway
        .call("DELETE", &format!("/restapis/{doomed}"), None)
        .await;
    assert_eq!(
        deleted.status, 202,
        "case DeleteRestApi: expected 202, got {} with {:?}",
        deleted.status, deleted.body
    );
    assert!(
        deleted.body.is_null() || deleted.body == json!({}),
        "case DeleteRestApi: expected an empty body, got {:?}",
        deleted.body
    );

    for (method, uri) in [
        ("GET", format!("/restapis/{doomed}")),
        ("GET", format!("/restapis/{doomed}/resources")),
        (
            "GET",
            format!("/restapis/{doomed}/resources/{resource_id}"),
        ),
        (
            "GET",
            format!("/restapis/{doomed}/resources/{resource_id}/methods/GET"),
        ),
        ("DELETE", format!("/restapis/{doomed}")),
    ] {
        let case = format!("after delete: {method} {uri}");
        let answer = gateway.call(method, &uri, None).await;
        assert_eq!(
            answer.status, 404,
            "case {case}: expected 404, got {} with {:?}",
            answer.status, answer.body
        );
    }

    let listed = gateway.call("GET", "/restapis", None).await;
    let ids: Vec<String> = page_items(&listed.body, "GetRestApis after delete")
        .iter()
        .map(|item| string_member(item, "id", "GetRestApis after delete"))
        .collect();
    assert_eq!(
        ids,
        vec![kept],
        "case GetRestApis after delete: the deleted API must be gone, got {ids:?}"
    );
}

#[tokio::test]
async fn clone_from_copies_the_source_resource_tree_under_fresh_identifiers() {
    let gateway = Gateway::new();
    let (source, source_root) = create_api(&gateway, "source").await;
    let pets = create_resource(&gateway, &source, &source_root, "pets").await;
    let pets_id = string_member(&pets, "id", "source/pets");
    create_resource(&gateway, &source, &pets_id, "{petId}").await;
    gateway
        .call(
            "PUT",
            &format!("/restapis/{source}/resources/{pets_id}/methods/GET"),
            Some(json!({ "authorizationType": "NONE" })),
        )
        .await;

    let cloned = gateway
        .call(
            "POST",
            "/restapis",
            Some(json!({ "name": "clone", "cloneFrom": source })),
        )
        .await;
    assert_eq!(
        cloned.status, 201,
        "case CreateRestApi(cloneFrom): expected 201, got {} with {:?}",
        cloned.status, cloned.body
    );
    let clone_id = string_member(&cloned.body, "id", "CreateRestApi(cloneFrom)");

    let source_paths = resource_paths(&gateway, &source).await;
    let clone_paths = resource_paths(&gateway, &clone_id).await;
    assert_eq!(
        clone_paths, source_paths,
        "case CreateRestApi(cloneFrom): expected the source's paths {source_paths:?}, got {clone_paths:?}"
    );

    let clone_resources = gateway
        .call(
            "GET",
            &format!("/restapis/{clone_id}/resources?embed=methods"),
            None,
        )
        .await;
    let items = page_items(&clone_resources.body, "GetResources(clone)");
    let clone_pets = items
        .iter()
        .find(|item| item["path"].as_str() == Some("/pets"))
        .unwrap_or_else(|| panic!("case cloneFrom: /pets must be cloned, got {items:?}"));
    assert_ne!(
        clone_pets["id"].as_str(),
        Some(pets_id.as_str()),
        "case cloneFrom: a cloned resource gets a fresh id, got {:?}",
        clone_pets["id"]
    );
    assert_eq!(
        clone_pets["resourceMethods"]["GET"]["authorizationType"].as_str(),
        Some("NONE"),
        "case cloneFrom: the source's methods must be cloned, got {:?}",
        clone_pets["resourceMethods"]
    );

    // The source is untouched by the clone.
    let source_again = resource_paths(&gateway, &source).await;
    assert_eq!(
        source_again, source_paths,
        "case cloneFrom: the source API must be unchanged, got {source_again:?}"
    );

    // An unknown `cloneFrom` is refused, and nothing is created for it.
    let before = page_items(
        &gateway.call("GET", "/restapis", None).await.body,
        "GetRestApis before bad clone",
    )
    .len();
    let bad = gateway
        .call(
            "POST",
            "/restapis",
            Some(json!({ "name": "bad", "cloneFrom": "zzzzzzzzzz" })),
        )
        .await;
    assert_eq!(
        bad.status, 400,
        "case CreateRestApi(unknown cloneFrom): expected 400, got {} with {:?}",
        bad.status, bad.body
    );
    let after = page_items(
        &gateway.call("GET", "/restapis", None).await.body,
        "GetRestApis after bad clone",
    )
    .len();
    assert_eq!(
        after, before,
        "case CreateRestApi(unknown cloneFrom): a refused create must persist nothing, expected {before} APIs, got {after}"
    );
}

/// The paths of an API's resources, in the order the listing reports them.
pub(crate) async fn resource_paths(gateway: &Gateway, api: &str) -> Vec<String> {
    let answer = gateway
        .call("GET", &format!("/restapis/{api}/resources"), None)
        .await;
    page_items(&answer.body, "resource_paths")
        .iter()
        .map(|item| string_member(item, "path", "resource_paths"))
        .collect()
}

/// Assert one bounded-member case: at the bound the request is served, one past it the
/// request is refused with the modelled `BadRequestException`.
async fn expect_bounded(
    gateway: &Gateway,
    case: &str,
    method: &str,
    uri: &str,
    body: Value,
    accepted: bool,
) {
    let answer = gateway.call(method, uri, Some(body)).await;
    if accepted {
        assert!(
            answer.status == 200 || answer.status == 201,
            "case {case}: expected the request at the bound to be served, got {} with {:?}",
            answer.status,
            answer.body
        );
    } else {
        assert_eq!(
            answer.status, 400,
            "case {case}: expected 400 one past the bound, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error.as_deref(),
            Some("BadRequestException"),
            "case {case}: expected BadRequestException, got {:?}",
            answer.error
        );
    }
}

/// A tag map of `count` entries.
fn tag_map(count: usize) -> Value {
    Value::Object(
        (0..count)
            .map(|n| (format!("key{n}"), Value::String("value".to_string())))
            .collect(),
    )
}

/// A one-entry map whose key is `chars` characters long.
fn single_key_map(chars: usize, value: &str) -> Value {
    Value::Object(
        [("k".repeat(chars), Value::String(value.to_string()))]
            .into_iter()
            .collect(),
    )
}

#[tokio::test]
async fn caller_sized_members_are_served_at_their_bound_and_refused_one_past_it() {
    // Every bound is a published quota or, where API Gateway publishes none, this
    // emulator's own resource guard; the test reads the constants rather than
    // restating their values, and exercises each AT the bound and ONE past it.
    use service_com_amazonaws_apigateway_backplanecontrolservice::state::{
        MAX_COLLECTION_ENTRIES, MAX_PATH_PART_CHARS, MAX_POLICY_CHARS, MAX_STRING_CHARS, MAX_TAGS,
        MAX_TAG_KEY_CHARS, MAX_TAG_VALUE_CHARS,
    };

    let gateway = Gateway::new();
    let (api, root) = create_api(&gateway, "bounds").await;
    let resource = create_resource(&gateway, &api, &root, "pets").await;
    let resource_id = string_member(&resource, "id", "bounds/pets");

    for (bound, accepted) in [(MAX_STRING_CHARS, true), (MAX_STRING_CHARS + 1, false)] {
        expect_bounded(
            &gateway,
            &format!("name of {bound} characters"),
            "POST",
            "/restapis",
            json!({ "name": "n".repeat(bound) }),
            accepted,
        )
        .await;
    }
    for (bound, accepted) in [(MAX_TAGS, true), (MAX_TAGS + 1, false)] {
        expect_bounded(
            &gateway,
            &format!("{bound} tags"),
            "POST",
            "/restapis",
            json!({ "name": format!("tags{bound}"), "tags": tag_map(bound) }),
            accepted,
        )
        .await;
    }
    for (bound, accepted) in [(MAX_TAG_KEY_CHARS, true), (MAX_TAG_KEY_CHARS + 1, false)] {
        expect_bounded(
            &gateway,
            &format!("tag key of {bound} characters"),
            "POST",
            "/restapis",
            json!({ "name": format!("tagkey{bound}"), "tags": single_key_map(bound, "v") }),
            accepted,
        )
        .await;
    }
    for (bound, accepted) in [(MAX_TAG_VALUE_CHARS, true), (MAX_TAG_VALUE_CHARS + 1, false)] {
        expect_bounded(
            &gateway,
            &format!("tag value of {bound} characters"),
            "POST",
            "/restapis",
            json!({ "name": format!("tagvalue{bound}"), "tags": { "key": "v".repeat(bound) } }),
            accepted,
        )
        .await;
    }
    for (bound, accepted) in [(MAX_POLICY_CHARS, true), (MAX_POLICY_CHARS + 1, false)] {
        expect_bounded(
            &gateway,
            &format!("policy of {bound} characters"),
            "POST",
            "/restapis",
            json!({ "name": format!("policy{bound}"), "policy": "p".repeat(bound) }),
            accepted,
        )
        .await;
    }
    for (bound, accepted) in [
        (MAX_COLLECTION_ENTRIES, true),
        (MAX_COLLECTION_ENTRIES + 1, false),
    ] {
        let types: Vec<String> = (0..bound).map(|n| format!("image/x{n}")).collect();
        expect_bounded(
            &gateway,
            &format!("{bound} binaryMediaTypes"),
            "POST",
            "/restapis",
            json!({ "name": format!("media{bound}"), "binaryMediaTypes": types }),
            accepted,
        )
        .await;
    }
    for (bound, accepted) in [
        (MAX_PATH_PART_CHARS, true),
        (MAX_PATH_PART_CHARS + 1, false),
    ] {
        expect_bounded(
            &gateway,
            &format!("pathPart of {bound} characters"),
            "POST",
            &format!("/restapis/{api}/resources/{root}"),
            json!({ "pathPart": "p".repeat(bound) }),
            accepted,
        )
        .await;
    }
    // Each row puts a DIFFERENT verb, so the second is refused for its size and not
    // for conflicting with the first.
    for (bound, verb, accepted) in [
        (MAX_COLLECTION_ENTRIES, "POST", true),
        (MAX_COLLECTION_ENTRIES + 1, "PUT", false),
    ] {
        let parameters = Value::Object(
            (0..bound)
                .map(|n| (format!("method.request.querystring.q{n}"), Value::Bool(true)))
                .collect(),
        );
        expect_bounded(
            &gateway,
            &format!("{bound} requestParameters"),
            "PUT",
            &format!("/restapis/{api}/resources/{resource_id}/methods/{verb}"),
            json!({ "authorizationType": "NONE", "requestParameters": parameters }),
            accepted,
        )
        .await;
    }
}

#[tokio::test]
async fn the_store_refuses_to_grow_past_the_published_record_quotas() {
    // The caps are API Gateway's own default quotas: "Resources per API: 300" and
    // "APIs per account per Region: 600". Past either, the modelled
    // `LimitExceededException` is answered rather than letting the store grow.
    use service_com_amazonaws_apigateway_backplanecontrolservice::state::{
        MAX_RESOURCES_PER_API, MAX_REST_APIS,
    };

    let gateway = Gateway::new();
    let (api, root) = create_api(&gateway, "quota").await;

    // The implicit root resource counts towards the per-API quota, so this fills it.
    for n in 1..MAX_RESOURCES_PER_API {
        create_resource(&gateway, &api, &root, &format!("r{n}")).await;
    }
    let over_resources = gateway
        .call(
            "POST",
            &format!("/restapis/{api}/resources/{root}"),
            Some(json!({ "pathPart": "one-too-many" })),
        )
        .await;
    assert_eq!(
        over_resources.status, 429,
        "case resource {} of an API: expected 429, got {} with {:?}",
        MAX_RESOURCES_PER_API + 1,
        over_resources.status,
        over_resources.body
    );
    assert_eq!(
        over_resources.error.as_deref(),
        Some("LimitExceededException"),
        "case resource past the quota: expected LimitExceededException, got {:?}",
        over_resources.error
    );

    // One API exists already, so this fills the account quota.
    for n in 1..MAX_REST_APIS {
        create_api(&gateway, &format!("api-{n}")).await;
    }
    let over_apis = gateway
        .call("POST", "/restapis", Some(json!({ "name": "one-too-many" })))
        .await;
    assert_eq!(
        over_apis.status, 429,
        "case API {} of an account: expected 429, got {} with {:?}",
        MAX_REST_APIS + 1,
        over_apis.status,
        over_apis.body
    );
    assert_eq!(
        over_apis.error.as_deref(),
        Some("LimitExceededException"),
        "case API past the quota: expected LimitExceededException, got {:?}",
        over_apis.error
    );

    // The store is bounded, not sealed: deleting an API frees its slot, and a clone of
    // a full tree is refused for the same quota rather than doubling the store.
    let deleted = gateway
        .call("DELETE", &format!("/restapis/{api}"), None)
        .await;
    assert_eq!(
        deleted.status, 202,
        "case delete to free a slot: expected 202, got {} with {:?}",
        deleted.status, deleted.body
    );
    let after_delete = gateway
        .call("POST", "/restapis", Some(json!({ "name": "after-delete" })))
        .await;
    assert_eq!(
        after_delete.status, 201,
        "case create after delete: expected 201, got {} with {:?}",
        after_delete.status, after_delete.body
    );
}

#[tokio::test]
async fn reading_does_not_change_what_a_later_caller_sees() {
    let gateway = Gateway::new();
    let (api, root) = create_api(&gateway, "stable").await;
    let resource = create_resource(&gateway, &api, &root, "pets").await;
    let resource_id = string_member(&resource, "id", "stable/pets");
    gateway
        .call(
            "PUT",
            &format!("/restapis/{api}/resources/{resource_id}/methods/GET"),
            Some(json!({ "authorizationType": "NONE" })),
        )
        .await;

    // Every reporting operation of the slice, read twice: the second answer must equal
    // the first, so none of them mutated the record it reported.
    for uri in [
        "/restapis".to_string(),
        format!("/restapis/{api}"),
        format!("/restapis/{api}/resources"),
        format!("/restapis/{api}/resources?embed=methods"),
        format!("/restapis/{api}/resources/{resource_id}"),
        format!("/restapis/{api}/resources/{resource_id}/methods/GET"),
    ] {
        let first = gateway.call("GET", &uri, None).await;
        let second = gateway.call("GET", &uri, None).await;
        assert_eq!(
            first.body, second.body,
            "case GET {uri}: a read must be repeatable, first {:?} then {:?}",
            first.body, second.body
        );
    }
}
