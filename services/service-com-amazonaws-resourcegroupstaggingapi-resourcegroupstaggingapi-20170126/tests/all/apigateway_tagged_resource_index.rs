// SPDX-License-Identifier: CC0-1.0

//! Amazon API Gateway's tagged resources, through this API: a resource the owning
//! service holds tags for is in `GetResources`' answers under the ARN that service
//! reports for it, a tag written here lands in that resource's OWN tags, and deleting
//! the resource there retracts the index entry with no untag.
//!
//! Every call drives the real wire path — the multi-service router over ONE shared
//! engine, exactly the arrangement `core/router` builds — so each resource is created
//! through API Gateway's own restJson endpoint and then looked for through this API's
//! awsJson one, which is what an SDK client does.
//!
//! The table spans the class the rule generalises over: every taggable resource kind
//! API Gateway stores (a REST API, a stage of one, an API key, a VPC link, a usage plan,
//! a custom domain name), so the rows differ in the ARN's resource-portion form (a
//! one-segment path, a four-segment path with a nested type, a path segment that is a
//! caller-supplied name rather than a minted id), in the `service:resourceType` token
//! each is selected by — two rows deliberately share one token — and in whether the
//! record is ready at once or still being provisioned. The API key the issue cited is
//! one row.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use service_com_amazonaws_apigateway_backplanecontrolservice as owner_apigateway;
use service_com_amazonaws_resourcegroupstaggingapi_resourcegroupstaggingapi_20170126::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The Region every request below is signed for: the one the tagging endpoint serves
/// and the one API Gateway writes into the ARNs it reports. Deliberately not the
/// emulator's default, so nothing can pass on a hard-coded Region.
const REGION: &str = "eu-west-1";

/// The SigV4 signing names the two services claim, which is what routes each request.
const TAGGING: &str = "tagging";
const APIGATEWAY: &str = "apigateway";

/// The tag every resource of this run carries, and the value that selects them.
const RUN_KEY: &str = "Run";
const RUN_VALUE: &str = "apigateway-tagging-index";

/// The per-row tag: which resource kind the row created, so one row can be selected
/// out of the index on its own.
const KIND_KEY: &str = "Kind";

/// The tag key the write legs add through this API and then remove again.
const OWNER_KEY: &str = "Owner";

/// A router carrying this API and API Gateway, over ONE engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // The tests exercise handler behaviour, not SigV4: the `Authorization` header is
    // present because it is what routes a request to a signing name.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    router.register_service(owner_apigateway::EmulatedService::new(engine));
    router
}

/// The `Authorization` header naming `signing_name` in [`REGION`].
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{signing_name}/aws4_request, \
         SignedHeaders=host, Signature=0"
    )
}

/// Parse a response body, or `Null` for the empty body a 202 carries.
fn parse(label: &str, body: &[u8]) -> serde_json::Value {
    if body.is_empty() {
        return serde_json::Value::Null;
    }
    serde_json::from_slice(body).unwrap_or_else(|error| {
        panic!(
            "{label} answered a body that is not JSON ({error}): {}",
            String::from_utf8_lossy(body)
        )
    })
}

/// One operation of this API: `POST /` with an `X-Amz-Target` and an awsJson body.
async fn tagging(
    router: &MultiServiceRouter,
    operation: &str,
    body: serde_json::Value,
) -> (u16, serde_json::Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header(
            "x-amz-target",
            format!("ResourceGroupsTaggingAPI_20170126.{operation}"),
        )
        .header("authorization", authorization(TAGGING))
        .body(bytes::Bytes::from(body.to_string()))
        .expect("build the request");
    let response = router.dispatch(request).await;
    (
        response.status().as_u16(),
        parse(operation, response.body()),
    )
}

/// One operation of API Gateway's own restJson endpoint: its modelled method and URI.
async fn apigateway(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<serde_json::Value>,
) -> (u16, serde_json::Value) {
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("apigateway.{REGION}.amazonaws.com"))
        .header("content-type", "application/json")
        .header("authorization", authorization(APIGATEWAY))
        .body(match &body {
            Some(body) => bytes::Bytes::from(body.to_string()),
            None => bytes::Bytes::new(),
        })
        .expect("build the request");
    let response = router.dispatch(request).await;
    let label = format!("{method} {uri}");
    (
        response.status().as_u16(),
        parse(&label, response.body()),
    )
}

/// The `id` an API Gateway create answered with, asserting the create succeeded with
/// the status that operation's own model declares.
async fn created_id(
    router: &MultiServiceRouter,
    case: &str,
    uri: &str,
    expected_status: u16,
    body: serde_json::Value,
) -> String {
    let (status, answered) = apigateway(router, "POST", uri, Some(body)).await;
    assert_eq!(
        status, expected_status,
        "case {case:?}: expected HTTP {expected_status} from POST {uri}, got {status} {answered}"
    );
    answered["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: POST {uri} answered no id: {answered}"))
        .to_string()
}

/// The ARNs of a `GetResources` page, sorted, so a page is compared as a SET.
fn page_arns(body: &serde_json::Value) -> Vec<String> {
    let mut arns: Vec<String> = body["ResourceTagMappingList"]
        .as_array()
        .unwrap_or_else(|| panic!("a GetResources body with no ResourceTagMappingList: {body}"))
        .iter()
        .map(|mapping| {
            mapping["ResourceARN"]
                .as_str()
                .unwrap_or_else(|| panic!("a mapping with no ResourceARN: {mapping}"))
                .to_string()
        })
        .collect();
    arns.sort();
    arns
}

/// The tag map a `GetResources` page reports for one ARN.
fn page_tags(body: &serde_json::Value, arn: &str) -> BTreeMap<String, String> {
    let mapping = body["ResourceTagMappingList"]
        .as_array()
        .unwrap_or_else(|| panic!("a GetResources body with no ResourceTagMappingList: {body}"))
        .iter()
        .find(|mapping| mapping["ResourceARN"] == serde_json::json!(arn))
        .unwrap_or_else(|| panic!("{arn} is not in the page: {body}"));
    mapping["Tags"]
        .as_array()
        .unwrap_or_else(|| panic!("a mapping with no Tags: {mapping}"))
        .iter()
        .map(|tag| {
            (
                tag["Key"].as_str().expect("Tag.Key").to_string(),
                tag["Value"].as_str().expect("Tag.Value").to_string(),
            )
        })
        .collect()
}

/// The tag map an API Gateway read reports on the record itself.
fn record_tags(body: &serde_json::Value) -> BTreeMap<String, String> {
    body["tags"]
        .as_object()
        .unwrap_or_else(|| panic!("an API Gateway body with no tags: {body}"))
        .iter()
        .map(|(key, value)| {
            (
                key.clone(),
                value
                    .as_str()
                    .unwrap_or_else(|| panic!("a tag value that is not a string: {value}"))
                    .to_string(),
            )
        })
        .collect()
}

/// A `GetResources` page, asserting the call succeeded.
async fn get_resources(
    router: &MultiServiceRouter,
    case: &str,
    request: serde_json::Value,
) -> serde_json::Value {
    let (status, body) = tagging(router, "GetResources", request.clone()).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from GetResources({request}), got {status} {body}"
    );
    body
}

/// A `TagFilters` entry selecting one key and value.
fn filter(key: &str, value: &str) -> serde_json::Value {
    serde_json::json!({ "Key": key, "Values": [value] })
}

/// One taggable API Gateway resource kind, created with tags through API Gateway's own
/// API: the row the index must carry.
struct Row {
    /// The case name, named by every assertion.
    name: &'static str,
    /// This row's `Kind` tag value, which selects it alone.
    kind: &'static str,
    /// The ARN API Gateway reports the resource under.
    arn: String,
    /// The `service:resourceType` token `ResourceTypeFilters` selects it by — read off
    /// the ARN's resource portion, so a stage's token is its API's.
    resource_type: &'static str,
    /// The URI of the operation that reads the record back, for the round trip.
    read_uri: String,
}

/// Create one of every taggable resource kind API Gateway stores, each tagged at
/// creation with the run tag and its own `Kind`, and return the rows plus the id of an
/// UNTAGGED API key — which the index must never report.
async fn create_rows(router: &MultiServiceRouter) -> (Vec<Row>, String) {
    /// The tag map a create carries.
    fn tags(kind: &str) -> serde_json::Value {
        serde_json::json!({ RUN_KEY: RUN_VALUE, KIND_KEY: kind })
    }
    /// The path-style, account-less ARN API Gateway reports a resource under.
    fn arn(resource_path: &str) -> String {
        format!("arn:aws:apigateway:{REGION}::{resource_path}")
    }

    let rest_api_id = created_id(
        router,
        "restapis",
        "/restapis",
        201,
        serde_json::json!({ "name": "tagging-index-api", "tags": tags("restapi") }),
    )
    .await;
    // A deployment is the snapshot of the API's methods a stage serves, so the API must
    // carry one before it can be deployed. The method goes on the ROOT resource every
    // API is created with, so nothing is added to the API's resource tree.
    let (status, read) = apigateway(router, "GET", &format!("/restapis/{rest_api_id}"), None).await;
    assert_eq!(
        status, 200,
        "case \"restapis\": expected HTTP 200 from GetRestApi, got {status} {read}"
    );
    let root_resource_id = read["rootResourceId"]
        .as_str()
        .unwrap_or_else(|| panic!("case \"restapis\": GetRestApi answered no rootResourceId: {read}"))
        .to_string();
    let (status, method) = apigateway(
        router,
        "PUT",
        &format!("/restapis/{rest_api_id}/resources/{root_resource_id}/methods/GET"),
        Some(serde_json::json!({ "authorizationType": "NONE" })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case \"method\": expected HTTP 201 from PutMethod, got {status} {method}"
    );
    // And the method's integration, which is what a stage serves a matched request with:
    // an API holding a method with none cannot be deployed at all.
    let (status, integration) = apigateway(
        router,
        "PUT",
        &format!("/restapis/{rest_api_id}/resources/{root_resource_id}/methods/GET/integration"),
        Some(serde_json::json!({ "type": "MOCK" })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case \"integration\": expected HTTP 201 from PutIntegration, got {status} {integration}"
    );
    // A stage needs a deployment to point at; the deployment itself carries no tags.
    let deployment_id = created_id(
        router,
        "deployments",
        &format!("/restapis/{rest_api_id}/deployments"),
        201,
        serde_json::json!({ "description": "tagging-index" }),
    )
    .await;
    let stage_name = "prod";
    let (status, stage) = apigateway(
        router,
        "POST",
        &format!("/restapis/{rest_api_id}/stages"),
        Some(serde_json::json!({
            "stageName": stage_name,
            "deploymentId": deployment_id,
            "tags": tags("stage"),
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case \"stages\": expected HTTP 201 from CreateStage, got {status} {stage}"
    );

    let api_key_id = created_id(
        router,
        "apikeys",
        "/apikeys",
        201,
        serde_json::json!({
            "name": "tagging-index-key",
            "enabled": true,
            "tags": tags("apikey"),
        }),
    )
    .await;
    // `CreateVpcLink` answers 202, the status its own model declares: the link is
    // provisioned asynchronously, which does not change that it is stored and tagged.
    let vpc_link_id = created_id(
        router,
        "vpclinks",
        "/vpclinks",
        202,
        serde_json::json!({
            "name": "tagging-index-link",
            "targetArns": [
                format!("arn:aws:elasticloadbalancing:{REGION}:000000000000:loadbalancer/net/index/1")
            ],
            "tags": tags("vpclink"),
        }),
    )
    .await;
    let usage_plan_id = created_id(
        router,
        "usageplans",
        "/usageplans",
        201,
        serde_json::json!({ "name": "tagging-index-plan", "tags": tags("usageplan") }),
    )
    .await;
    let domain_name = "index.example.com";
    let (status, domain) = apigateway(
        router,
        "POST",
        "/domainnames",
        // A custom domain name is created with the certificate its (default, edge-optimized)
        // endpoint presents: a create that names none is refused, and which certificate that
        // is has nothing to do with the tag index under test.
        Some(serde_json::json!({
            "domainName": domain_name,
            "certificateArn": format!(
                "arn:aws:acm:{REGION}:{}:certificate/11111111-2222-3333-4444-555555555555",
                engine::emulator_account_id()
            ),
            "tags": tags("domainname"),
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case \"domainnames\": expected HTTP 201 from CreateDomainName, got {status} {domain}"
    );

    // "GetResources does not return untagged resources": a key created with no tags at
    // all is the control row.
    let untagged_key_id = created_id(
        router,
        "an untagged apikey",
        "/apikeys",
        201,
        serde_json::json!({ "name": "tagging-index-untagged-key", "enabled": true }),
    )
    .await;

    let rows = vec![
        Row {
            name: "a REST API",
            kind: "restapi",
            arn: arn(&format!("/restapis/{rest_api_id}")),
            resource_type: "apigateway:restapis",
            read_uri: format!("/restapis/{rest_api_id}"),
        },
        Row {
            // The nested form: the resource portion is four segments, and its leading
            // one is the API's, so this row shares the row above's type token.
            name: "a stage of that API",
            kind: "stage",
            arn: arn(&format!("/restapis/{rest_api_id}/stages/{stage_name}")),
            resource_type: "apigateway:restapis",
            read_uri: format!("/restapis/{rest_api_id}/stages/{stage_name}"),
        },
        Row {
            // The issue's own row.
            name: "an API key",
            kind: "apikey",
            arn: arn(&format!("/apikeys/{api_key_id}")),
            resource_type: "apigateway:apikeys",
            read_uri: format!("/apikeys/{api_key_id}"),
        },
        Row {
            // A record the CLOCK moves (a VPC link is provisioned asynchronously): its
            // index row is there from the create, whatever status it reports.
            name: "a VPC link",
            kind: "vpclink",
            arn: arn(&format!("/vpclinks/{vpc_link_id}")),
            resource_type: "apigateway:vpclinks",
            read_uri: format!("/vpclinks/{vpc_link_id}"),
        },
        Row {
            name: "a usage plan",
            kind: "usageplan",
            arn: arn(&format!("/usageplans/{usage_plan_id}")),
            resource_type: "apigateway:usageplans",
            read_uri: format!("/usageplans/{usage_plan_id}"),
        },
        Row {
            // The one kind whose path segment is a caller-supplied name, dots and all,
            // rather than a minted id.
            name: "a custom domain name",
            kind: "domainname",
            arn: arn(&format!("/domainnames/{domain_name}")),
            resource_type: "apigateway:domainnames",
            read_uri: format!("/domainnames/{domain_name}"),
        },
    ];
    (rows, untagged_key_id)
}

#[tokio::test]
async fn a_resource_tagged_at_creation_is_discoverable_by_tag_and_resource_type() {
    let router = emulator();
    let (rows, untagged_key_id) = create_rows(&router).await;

    // Every row, selected on its own by the two tags its create carried.
    for row in &rows {
        let body = get_resources(
            &router,
            row.name,
            serde_json::json!({
                "ResourceTypeFilters": [row.resource_type],
                "TagFilters": [filter(RUN_KEY, RUN_VALUE), filter(KIND_KEY, row.kind)],
            }),
        )
        .await;
        assert_eq!(
            page_arns(&body),
            vec![row.arn.clone()],
            "case {:?}: expected exactly [{}], got {:?}",
            row.name,
            row.arn,
            page_arns(&body)
        );
        let reported = page_tags(&body, &row.arn);
        let expected: BTreeMap<String, String> = [
            (RUN_KEY.to_string(), RUN_VALUE.to_string()),
            (KIND_KEY.to_string(), row.kind.to_string()),
        ]
        .into_iter()
        .collect();
        assert_eq!(
            reported, expected,
            "case {:?}: expected the tags the create carried {expected:?}, got {reported:?}",
            row.name
        );
    }

    // One query per distinct `service:resourceType` token: it selects every row of that
    // type and nothing else, which is what the two rows sharing a token pin.
    let tokens: BTreeSet<&str> = rows.iter().map(|row| row.resource_type).collect();
    for token in tokens {
        let body = get_resources(
            &router,
            token,
            serde_json::json!({
                "ResourceTypeFilters": [token],
                "TagFilters": [filter(RUN_KEY, RUN_VALUE)],
            }),
        )
        .await;
        let mut expected: Vec<String> = rows
            .iter()
            .filter(|row| row.resource_type == token)
            .map(|row| row.arn.clone())
            .collect();
        expected.sort();
        assert_eq!(
            page_arns(&body),
            expected,
            "case {token:?}: expected {expected:?}, got {:?}",
            page_arns(&body)
        );
    }

    // The BARE service token selects every resource of the service: "specifying a
    // service of ec2 returns all Amazon EC2 resources" (`GetResources`'
    // `ResourceTypeFilters` documentation).
    let mut every_arn: Vec<String> = rows.iter().map(|row| row.arn.clone()).collect();
    every_arn.sort();
    let body = get_resources(
        &router,
        "the bare apigateway token",
        serde_json::json!({
            "ResourceTypeFilters": ["apigateway"],
            "TagFilters": [filter(RUN_KEY, RUN_VALUE)],
        }),
    )
    .await;
    assert_eq!(
        page_arns(&body),
        every_arn,
        "case \"the bare apigateway token\": expected {every_arn:?}, got {:?}",
        page_arns(&body)
    );

    // …and with no filter at all the page is every row and NOTHING else: the untagged
    // API key is not a tagged resource.
    let body = get_resources(&router, "no filters", serde_json::json!({})).await;
    assert_eq!(
        page_arns(&body),
        every_arn,
        "case \"no filters\": expected {every_arn:?}, got {:?}",
        page_arns(&body)
    );
    let untagged_arn = format!("arn:aws:apigateway:{REGION}::/apikeys/{untagged_key_id}");
    assert!(
        !page_arns(&body).contains(&untagged_arn),
        "case \"no filters\": an untagged resource must not be reported, got {:?}",
        page_arns(&body)
    );
}

#[tokio::test]
async fn a_tag_written_through_this_api_lands_in_the_resources_own_tags() {
    let router = emulator();
    let (rows, _) = create_rows(&router).await;

    for row in &rows {
        // Write through THIS API, against the ARN the index reported.
        let (status, body) = tagging(
            &router,
            "TagResources",
            serde_json::json!({
                "ResourceARNList": [row.arn],
                "Tags": { OWNER_KEY: row.kind },
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200 from TagResources, got {status} {body}",
            row.name
        );
        assert_eq!(
            body["FailedResourcesMap"],
            serde_json::json!({}),
            "case {:?}: expected an empty FailedResourcesMap, got {}",
            row.name,
            body["FailedResourcesMap"]
        );

        // …and read it back through the OWNING service's own read of that record.
        let (status, record) = apigateway(&router, "GET", &row.read_uri, None).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200 from GET {}, got {status} {record}",
            row.name, row.read_uri
        );
        let held = record_tags(&record);
        let expected: BTreeMap<String, String> = [
            (RUN_KEY.to_string(), RUN_VALUE.to_string()),
            (KIND_KEY.to_string(), row.kind.to_string()),
            (OWNER_KEY.to_string(), row.kind.to_string()),
        ]
        .into_iter()
        .collect();
        assert_eq!(
            held, expected,
            "case {:?}: expected the record to hold {expected:?}, got {held:?}",
            row.name
        );

        // The same tag is in this API's own answer, because the two read one map.
        let body = get_resources(
            &router,
            row.name,
            serde_json::json!({ "ResourceARNList": [row.arn] }),
        )
        .await;
        assert_eq!(
            page_tags(&body, &row.arn),
            expected,
            "case {:?}: expected GetResources to report {expected:?}, got {:?}",
            row.name,
            page_tags(&body, &row.arn)
        );

        // Removing it through this API removes it from the record, and only it.
        let (status, body) = tagging(
            &router,
            "UntagResources",
            serde_json::json!({
                "ResourceARNList": [row.arn],
                "TagKeys": [OWNER_KEY],
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200 from UntagResources, got {status} {body}",
            row.name
        );
        let (_, record) = apigateway(&router, "GET", &row.read_uri, None).await;
        let held = record_tags(&record);
        let expected: BTreeMap<String, String> = [
            (RUN_KEY.to_string(), RUN_VALUE.to_string()),
            (KIND_KEY.to_string(), row.kind.to_string()),
        ]
        .into_iter()
        .collect();
        assert_eq!(
            held, expected,
            "case {:?}: expected the untag to leave {expected:?}, got {held:?}",
            row.name
        );
    }
}

#[tokio::test]
async fn an_apigateway_arn_the_owner_will_not_tag_fails_with_that_services_own_refusal() {
    let router = emulator();
    // One live resource, so the batch below is not refused for want of any resolvable
    // ARN and each row's own `FailedResourcesMap` entry is what is asserted.
    let (rows, _) = create_rows(&router).await;
    let live = &rows[0];

    // (case, ARN, expected StatusCode, expected ErrorCode)
    //
    // The rows differ along WHY the owning service will not tag the ARN: a well-formed
    // ARN of a taggable kind that names no stored record is a missing resource, and an
    // ARN whose resource portion is not a taggable resource of the service at all is a
    // bad request. Both statuses and codes are API Gateway's own
    // (`smithy_query(query="shape", shape="NotFoundException,BadRequestException")` on
    // its model: 404 and 400), because `FailureInfo` reports "any valid error code
    // returned by the … service that hosts the resource".
    let ghost = format!("arn:aws:apigateway:{REGION}::/apikeys/nosuchkey");
    let plan = format!("arn:aws:apigateway:{REGION}::/usageplans/nosuchplan");
    let not_taggable = format!("arn:aws:apigateway:{REGION}::/restapis/abcdef/resources/ghi");
    let deployment = format!("arn:aws:apigateway:{REGION}::/restapis/abcdef/deployments/jkl");
    let rows: &[(&str, &str, u16, &str)] = &[
        ("an API key that was never created", &ghost, 404, "NotFoundException"),
        ("a usage plan that was never created", &plan, 404, "NotFoundException"),
        (
            "a resource of an API, which this service does not tag",
            &not_taggable,
            400,
            "BadRequestException",
        ),
        (
            "a deployment of an API, which this service does not tag",
            &deployment,
            400,
            "BadRequestException",
        ),
    ];

    for (case, arn, status_code, error_code) in rows {
        let (status, body) = tagging(
            &router,
            "TagResources",
            serde_json::json!({
                "ResourceARNList": [arn, live.arn],
                "Tags": { OWNER_KEY: "team-a" },
            }),
        )
        .await;
        // A per-resource failure, not a request fault: the live ARN of the same batch
        // is tagged and the call is a 200.
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from TagResources, got {status} {body}"
        );
        let failure = &body["FailedResourcesMap"][*arn];
        assert_eq!(
            failure["StatusCode"],
            serde_json::json!(status_code),
            "case {case:?}: expected StatusCode {status_code}, got {}",
            body["FailedResourcesMap"]
        );
        assert_eq!(
            failure["ErrorCode"],
            serde_json::json!(error_code),
            "case {case:?}: expected ErrorCode {error_code}, got {}",
            body["FailedResourcesMap"]
        );
        // …and nothing was indexed under the refused ARN: it is not a resource this
        // API now reports tags for.
        let page = get_resources(
            &router,
            case,
            serde_json::json!({ "ResourceARNList": [arn] }),
        )
        .await;
        assert!(
            page_arns(&page).is_empty(),
            "case {case:?}: a refused ARN must not enter the index, got {:?}",
            page_arns(&page)
        );
    }

    // The `apigateway` namespace is SHARED: API Gateway V2's own resources carry it
    // (`/apis/{apiId}`), and that service is a different model with a different store, so
    // an ARN of this namespace that is not this service's to resolve is NOT refused —
    // it keeps the answer every namespace no linked owner serves gets, with this API's
    // own store as its tag home.
    let shared = format!("arn:aws:apigateway:{REGION}::/apis/v2apiid");
    let (status, body) = tagging(
        &router,
        "TagResources",
        serde_json::json!({
            "ResourceARNList": [shared],
            "Tags": { RUN_KEY: RUN_VALUE, KIND_KEY: "v2api" },
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"a V2 API of the shared namespace\": expected HTTP 200, got {status} {body}"
    );
    assert_eq!(
        body["FailedResourcesMap"],
        serde_json::json!({}),
        "case \"a V2 API of the shared namespace\": expected no failure, got {}",
        body["FailedResourcesMap"]
    );
    let page = get_resources(
        &router,
        "a V2 API of the shared namespace",
        serde_json::json!({ "ResourceARNList": [shared] }),
    )
    .await;
    assert_eq!(
        page_arns(&page),
        vec![shared.clone()],
        "case \"a V2 API of the shared namespace\": expected [{shared}], got {:?}",
        page_arns(&page)
    );
}

#[tokio::test]
async fn deleting_a_resource_through_its_owner_retracts_its_index_entry() {
    let router = emulator();
    let (rows, _) = create_rows(&router).await;

    // The API key is the one kind of this table whose delete is implemented; the rest
    // of the rows are what the retraction must LEAVE alone.
    let key = rows
        .iter()
        .find(|row| row.kind == "apikey")
        .expect("the table carries an API key row");
    let (status, body) = apigateway(&router, "DELETE", &key.read_uri, None).await;
    assert_eq!(
        status, 202,
        "case {:?}: expected HTTP 202 from DELETE {}, got {status} {body}",
        key.name, key.read_uri
    );

    let mut expected: Vec<String> = rows
        .iter()
        .filter(|row| row.kind != "apikey")
        .map(|row| row.arn.clone())
        .collect();
    expected.sort();
    let body = get_resources(
        &router,
        "after the delete",
        serde_json::json!({ "TagFilters": [filter(RUN_KEY, RUN_VALUE)] }),
    )
    .await;
    assert_eq!(
        page_arns(&body),
        expected,
        "case \"after the delete\": expected the deleted key to be retracted with no untag, \
         leaving {expected:?}, got {:?}",
        page_arns(&body)
    );

    // The owning service agrees the record is gone.
    let (status, record) = apigateway(&router, "GET", &key.read_uri, None).await;
    assert_eq!(
        status, 404,
        "case \"after the delete\": expected HTTP 404 from GET {}, got {status} {record}",
        key.read_uri
    );
}
