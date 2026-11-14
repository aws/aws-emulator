// SPDX-License-Identifier: CC0-1.0

//! What a RUNNING handler's own API Gateway SDK call hands back, served and refused: the
//! answer of the service that served it, over the wire, in one invocation.
//!
//! RULE: a call a handler makes through the SDK the managed runtime supplies is made to the
//! service that models it and is answered by that service alone — a served call hands the
//! handler the operation's modelled output document with the success status its `@http`
//! trait gives it, and a refused one is raised as `botocore.exceptions.ClientError`
//! carrying that service's OWN HTTP status, error code and message, the same three a client
//! of the service is answered with for the same request. So a handler can tell a served
//! call from a refusal and one refusal class from another, which is what negative-path code
//! keys on.
//!
//! The rows differ along what the rule generalises over rather than repeating one recorded
//! call: how many parameters the operation binds (none, one path label, two), whether the
//! call is served or refused, which refusal class it is (a 404 `NotFoundException` on the
//! API the request addresses, the same class on a record inside it, a 400
//! `BadRequestException` on a request the service will not act on) and whether the refusal's
//! sentence names something the request carried. Each row's expected status and code are the
//! ones this service's model declares for that operation
//! (`smithy_query(query="operation", operation="GetRestApis,GetDeployment,GetDeployments,\
//! CreateDeployment")`), and each refused row's expected message is the one the SERVICE
//! answers a client for the same request — read here rather than spelled, because the
//! wording is that service's own rule and not this seam's.
//!
//! Every row's resources are created as a CLIENT of API Gateway beforehand, so what the
//! handler reads back is state the service already holds: a call that answered without
//! reaching the service fails here.
//!
//! The suite RUNS a handler, so it takes the target's one invocation stream
//! ([`crate::invoke_request_id::one_invocation_stream_at_a_time`]): this emulator serves at
//! most `max_concurrent_runs` invocations at once for the whole process.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::event_source_mappings::{call, create_running_function, router_on};

/// Where `Invoke` is bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// The Region every request below is signed for.
const REGION: &str = "us-east-1";

/// How many seconds the handler has to make its calls in — generous room for every row,
/// because the rows are what is under test and not how fast this host runs.
const HANDLER_TIMEOUT: i64 = 60;

/// An identifier no API of this emulator has: the ten-character width API Gateway mints,
/// spelled out of the same alphabet.
const UNKNOWN_API: &str = "zzzzzzzzzz";

/// An identifier no deployment of the API below has.
const UNKNOWN_DEPLOYMENT: &str = "zzzzzz";

/// Lambda and Amazon API Gateway over ONE shared engine, as `core/router/src/lib.rs` mounts
/// them: the handler's SDK call is dispatched back into this same router, so the APIs this
/// suite creates are the APIs the handler reads.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = router_on(&engine);
    router.register_service(
        service_com_amazonaws_apigateway_backplanecontrolservice::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router
}

/// What one API Gateway request answered a CLIENT of the service.
#[derive(Debug)]
struct Answered {
    /// The HTTP status of the answer.
    status: u16,
    /// The modelled error shape a refusal names, from `x-amzn-errortype` or the document's
    /// own `__type`, without the shape id's namespace — empty for a served answer.
    code: String,
    /// The refusal's own message — empty for a served answer.
    message: String,
    /// The answer's document.
    body: Value,
}

/// One API Gateway call made the way a CLIENT makes it, answered without assertion so a
/// refusal is readable here.
async fn gateway(router: &MultiServiceRouter, method: &str, uri: &str, body: &Value) -> Answered {
    let payload = if body.is_null() {
        Vec::new()
    } else {
        serde_json::to_vec(body).expect("the test request body must serialize")
    };
    let request = http::Request::builder()
        .method(method)
        .uri(format!("http://apigateway.{REGION}.amazonaws.com{uri}"))
        .header("host", format!("apigateway.{REGION}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=emulator-test-key/20260101/{REGION}/apigateway/\
                 aws4_request, SignedHeaders=host, Signature=not-verified"
            ),
        )
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(payload))
        .expect("the test request must build");
    let response = router.dispatch(request).await;
    let (parts, body) = response.into_parts();
    let document: Value = if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&body).unwrap_or(Value::Null)
    };
    let header = parts
        .headers
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let named = header.or_else(|| {
        document
            .get("__type")
            .and_then(Value::as_str)
            .map(str::to_string)
    });
    let code = named
        .as_deref()
        .and_then(|named| named.split('#').next_back())
        .map(|shape| shape.split(':').next().unwrap_or(shape).trim().to_string())
        .unwrap_or_default();
    let message = document
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    Answered {
        status: parts.status.as_u16(),
        code,
        message,
        body: document,
    }
}

/// `CreateRestApi` for `name`, asserting the modelled 201, answering the API's own id.
async fn create_api(router: &MultiServiceRouter, case: &str, name: &str) -> String {
    let answer = gateway(router, "POST", "/restapis", &json!({ "name": name })).await;
    assert_eq!(
        answer.status, 201,
        "case {case:?}: expected CreateRestApi {name:?} to answer 201, got {} ({:?})",
        answer.status, answer.body
    );
    answer.body["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the created API must carry an id: {:?}", answer.body))
        .to_string()
}

/// The id of the API's root resource, which `CreateRestApi` makes with it.
async fn root_resource(router: &MultiServiceRouter, case: &str, api: &str) -> String {
    let answer = gateway(
        router,
        "GET",
        &format!("/restapis/{api}/resources"),
        &Value::Null,
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected GetResources to answer 200, got {} ({:?})",
        answer.status, answer.body
    );
    answer.body["item"]
        .as_array()
        .and_then(|items| {
            items
                .iter()
                .find(|item| item["path"].as_str() == Some("/"))
                .and_then(|item| item["id"].as_str())
        })
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: the API must carry a root resource: {:?}",
                answer.body
            )
        })
        .to_string()
}

/// A method on the API's root resource, so the API has something a deployment can snapshot.
async fn put_root_method(router: &MultiServiceRouter, case: &str, api: &str, resource: &str) {
    let answer = gateway(
        router,
        "PUT",
        &format!("/restapis/{api}/resources/{resource}/methods/GET"),
        &json!({ "authorizationType": "NONE" }),
    )
    .await;
    assert_eq!(
        answer.status, 201,
        "case {case:?}: expected PutMethod to answer 201, got {} ({:?})",
        answer.status, answer.body
    );
}

/// `CreateDeployment` on `api`, asserting the modelled 201, answering the deployment's id.
async fn create_deployment(router: &MultiServiceRouter, case: &str, api: &str) -> String {
    let answer = gateway(
        router,
        "POST",
        &format!("/restapis/{api}/deployments"),
        &json!({ "description": "created by a client" }),
    )
    .await;
    assert_eq!(
        answer.status, 201,
        "case {case:?}: expected CreateDeployment to answer 201, got {} ({:?})",
        answer.status, answer.body
    );
    answer.body["id"]
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: the created deployment must carry an id: {:?}",
                answer.body
            )
        })
        .to_string()
}

/// What the SDK must hand the handler back for one row.
#[derive(Debug)]
enum Expect {
    /// Served: the status the operation's own `@http` trait gives a success, and the output
    /// document under the output shape's own member names.
    Served {
        /// That status.
        status: u16,
    },
    /// Refused: the modelled error shape and HTTP status below, with the message the SERVICE
    /// itself answers a client for the same request.
    Refused {
        /// The status the model declares for that error on this operation.
        status: u16,
        /// The modelled error shape's own name, which is the code this service's protocol
        /// reports.
        code: &'static str,
        /// The same request made as a client of the service.
        client: Answered,
        /// Text the refusal's sentence must carry, where the service's rule builds it from
        /// the request — `None` where the sentence is fixed.
        quotes: Option<String>,
    },
}

/// One row: the call the handler makes through the SDK, and what the SDK must hand back.
#[derive(Debug)]
struct Row {
    /// The row's name, reported by the handler beside its result and named in every
    /// assertion about it.
    name: &'static str,
    /// The client method the handler calls, which is the operation it names.
    op: &'static str,
    /// The keyword arguments it calls that method with.
    params: Value,
    /// What must come back.
    expect: Expect,
}

/// The handler the suite runs: it makes each call the event names, through the SDK the
/// runtime supplies, and reports what came back — the answer's own members for a served
/// call, the refusal's status, code and message for a refused one. It decides nothing
/// itself, so what is asserted is what the SDK gave the guest.
const HANDLER_SOURCE: &str = r#"
import boto3
import botocore.exceptions


def lambda_handler(event, context):
    client = boto3.client("apigateway")
    results = []
    for case in event["cases"]:
        try:
            answer = getattr(client, case["op"])(**case["params"])
        except botocore.exceptions.ClientError as error:
            results.append(
                {
                    "case": case["case"],
                    "served": False,
                    "status": error.response["ResponseMetadata"]["HTTPStatusCode"],
                    "code": error.response["Error"]["Code"],
                    "message": error.response["Error"]["Message"],
                }
            )
            continue
        results.append(
            {
                "case": case["case"],
                "served": True,
                "status": answer["ResponseMetadata"]["HTTPStatusCode"],
                "members": sorted(
                    name for name in answer if name != "ResponseMetadata"
                ),
                "id": answer.get("id"),
                "ids": [entry.get("id") for entry in answer.get("items", [])],
            }
        )
    return {"results": results}
"#;

/// Every row of the table, in one invocation, each asserted on what the SDK handed the guest
/// — and each refusal against what the service answers a client for the same request.
#[tokio::test]
async fn a_handlers_rest_api_calls_are_answered_by_the_service_that_models_them() {
    let _stream = crate::invoke_request_id::one_invocation_stream_at_a_time().await;
    let case = "a handler reading API Gateway through the runtime's SDK";
    let router = emulator();

    // Two APIs, so the parameterless list has more than one entry to report, and a third
    // with no method on any resource, which is a request the service will not act on.
    let first = create_api(&router, case, "orders-api").await;
    let second = create_api(&router, case, "invoices-api").await;
    let undeployable = create_api(&router, case, "empty-api").await;
    let root = root_resource(&router, case, &first).await;
    put_root_method(&router, case, &first, &root).await;
    let deployment = create_deployment(&router, case, &first).await;

    // The same three requests the refused rows make, made as a CLIENT of the service: their
    // answers are what the handler's SDK must hand back.
    let wrong_api = gateway(
        &router,
        "GET",
        &format!("/restapis/{UNKNOWN_API}/deployments/{deployment}"),
        &Value::Null,
    )
    .await;
    let wrong_deployment = gateway(
        &router,
        "GET",
        &format!("/restapis/{first}/deployments/{UNKNOWN_DEPLOYMENT}"),
        &Value::Null,
    )
    .await;
    let nothing_to_deploy = gateway(
        &router,
        "POST",
        &format!("/restapis/{undeployable}/deployments"),
        &json!({ "description": "nothing to snapshot" }),
    )
    .await;

    let rows = vec![
        Row {
            name: "a list that binds no parameter at all",
            op: "get_rest_apis",
            params: json!({}),
            expect: Expect::Served { status: 200 },
        },
        Row {
            name: "a read addressed by two path labels",
            op: "get_deployment",
            params: json!({ "restApiId": first.clone(), "deploymentId": deployment.clone() }),
            expect: Expect::Served { status: 200 },
        },
        Row {
            name: "a list addressed by one path label",
            op: "get_deployments",
            params: json!({ "restApiId": first.clone() }),
            expect: Expect::Served { status: 200 },
        },
        Row {
            name: "a read whose API does not exist",
            op: "get_deployment",
            params: json!({ "restApiId": UNKNOWN_API, "deploymentId": deployment.clone() }),
            expect: Expect::Refused {
                status: 404,
                code: "NotFoundException",
                client: wrong_api,
                // This service names the identifier it could not resolve, so the sentence
                // carries what the request sent.
                quotes: Some(UNKNOWN_API.to_string()),
            },
        },
        Row {
            name: "a read whose record inside the API does not exist",
            op: "get_deployment",
            params: json!({ "restApiId": first.clone(), "deploymentId": UNKNOWN_DEPLOYMENT }),
            expect: Expect::Refused {
                status: 404,
                code: "NotFoundException",
                client: wrong_deployment,
                quotes: None,
            },
        },
        Row {
            name: "a write the service will not act on",
            op: "create_deployment",
            params: json!({ "restApiId": undeployable.clone(), "description": "nothing to snapshot" }),
            expect: Expect::Refused {
                status: 400,
                code: "BadRequestException",
                client: nothing_to_deploy,
                quotes: None,
            },
        },
    ];

    // Each refused row's reference answer is the service's own modelled refusal, before any
    // of it is asked of the SDK.
    for row in &rows {
        if let Expect::Refused {
            status,
            code,
            client,
            ..
        } = &row.expect
        {
            assert_eq!(
                (client.status, client.code.as_str()),
                (*status, *code),
                "case {case:?} row {:?}: expected the service itself to answer the modelled \
                 refusal, got {} {:?} ({:?})",
                row.name,
                client.status,
                client.code,
                client.body
            );
            assert!(
                !client.message.is_empty(),
                "case {case:?} row {:?}: expected the service's own message, got {:?}",
                row.name,
                client.body
            );
        }
    }

    let caller = "rest-api-answer-fidelity-caller";
    create_running_function(
        &router,
        case,
        caller,
        HANDLER_SOURCE,
        HANDLER_TIMEOUT,
        &json!({}),
    )
    .await;

    let cases: Vec<Value> = rows
        .iter()
        .map(|row| json!({ "case": row.name, "op": row.op, "params": row.params }))
        .collect();
    let answered = call(
        &router,
        "POST",
        &format!("{FUNCTIONS_URI}/{caller}/invocations"),
        &json!({ "cases": cases }),
    )
    .await;
    assert_eq!(
        answered.status,
        200,
        "case {case:?}: expected HTTP 200 from Invoke, got {} {}",
        answered.status,
        answered.text()
    );
    assert_eq!(
        answered.header("x-amz-function-error"),
        None,
        "case {case:?}: the handler's own calls must not fault: {}",
        answered.text()
    );
    let payload = answered.json();
    let reported: BTreeMap<String, Value> = payload["results"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected the handler's results, got {payload}"))
        .iter()
        .map(|result| {
            (
                result["case"].as_str().unwrap_or_default().to_string(),
                result.clone(),
            )
        })
        .collect();
    assert_eq!(
        reported.len(),
        rows.len(),
        "case {case:?}: expected one result per row, got {payload}"
    );

    for row in &rows {
        let result = reported
            .get(row.name)
            .unwrap_or_else(|| panic!("case {case:?} row {:?}: no result: {payload}", row.name));
        match &row.expect {
            Expect::Served { status } => {
                assert_eq!(
                    result["served"],
                    json!(true),
                    "case {case:?} row {:?}: expected the call to be served, got {result}",
                    row.name
                );
                assert_eq!(
                    result["status"],
                    json!(status),
                    "case {case:?} row {:?}: expected the operation's own success status, got \
                     {result}",
                    row.name
                );
            }
            Expect::Refused {
                status,
                code,
                client,
                quotes,
            } => {
                assert_eq!(
                    result["served"],
                    json!(false),
                    "case {case:?} row {:?}: expected the call to be refused, got {result}",
                    row.name
                );
                assert_eq!(
                    result["status"],
                    json!(status),
                    "case {case:?} row {:?}: expected the refusal's own status, got {result}",
                    row.name
                );
                assert_eq!(
                    result["code"],
                    json!(code),
                    "case {case:?} row {:?}: expected the modelled error shape as the code, got \
                     {result}",
                    row.name
                );
                assert_eq!(
                    result["message"],
                    json!(client.message.clone()),
                    "case {case:?} row {:?}: expected the message the service answers a client \
                     ({:?}), got {result}",
                    row.name,
                    client.message
                );
                if let Some(quoted) = quotes {
                    assert!(
                        result["message"]
                            .as_str()
                            .is_some_and(|message| message.contains(quoted.as_str())),
                        "case {case:?} row {:?}: expected the sentence to name {quoted:?}, got \
                         {result}",
                        row.name
                    );
                }
            }
        }
    }

    // The served rows' own documents: what the SDK hands the guest is the output shape's
    // members, carrying the records the service already held.
    let listed = &reported["a list that binds no parameter at all"];
    let listed_ids: Vec<&str> = listed["ids"]
        .as_array()
        .map(|ids| ids.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    for api in [&first, &second, &undeployable] {
        assert!(
            listed_ids.contains(&api.as_str()),
            "case {case:?}: expected {api:?} among the listed APIs, got {listed}"
        );
    }
    // The output shape's own member names are what the guest reads, whatever the wire called
    // each one: this list's `items` travels as `item` (`jsonName`), and the guest sees
    // neither that spelling nor a member the output shape does not declare.
    let listed_members: Vec<&str> = listed["members"]
        .as_array()
        .map(|members| members.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    assert!(
        listed_members.contains(&"items") && !listed_members.contains(&"item"),
        "case {case:?}: expected the output shape's own `items` member, got {listed}"
    );

    let read = &reported["a read addressed by two path labels"];
    assert_eq!(
        read["id"],
        json!(deployment.clone()),
        "case {case:?}: expected the deployment the labels addressed, got {read}"
    );
    // The output shape's own members for the stored deployment: its id, the description it
    // was created with and the instant it was created at. `apiSummary` is the one member
    // this read carries only where the request asks for it with `embed`, so it is not here.
    assert_eq!(
        read["members"],
        json!(["createdDate", "description", "id"]),
        "case {case:?}: expected the output shape's own member names, got {read}"
    );

    let deployments = &reported["a list addressed by one path label"];
    let deployment_ids: Vec<&str> = deployments["ids"]
        .as_array()
        .map(|ids| ids.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    assert!(
        deployment_ids.contains(&deployment.as_str()),
        "case {case:?}: expected the API's own deployment to be listed, got {deployments}"
    );
}
