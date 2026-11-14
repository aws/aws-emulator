// SPDX-License-Identifier: CC0-1.0

//! What a RUNNING Lambda function's own aoss-signed request to an OpenSearch Serverless
//! COLLECTION's published endpoint is answered with, across the four crates that own the
//! halves of it: OpenSearch Serverless publishes the endpoint and holds the policies, IAM
//! holds the roles, STS mints the session the handler's request is signed as, and Lambda
//! runs the handler — so the suite lives here.
//!
//! Two rules are pinned, which is what the whole path takes:
//!
//! * a handler's own HTTP request may address any endpoint THIS emulator published,
//!   including the `host:port` a resource's own listener is published at — the address a
//!   collection reports in `collectionEndpoint` for a client to dial. Such an address names
//!   no service in its host, so WHICH service it asks is the credential scope the handler
//!   signed with, and the plane that published the address answers it. A handler that cannot
//!   reach it at all sees a name-resolution failure, which is not an answer the data plane
//!   ever gives;
//! * the collection's data plane admits only a principal one of the collection's `data`
//!   access policies NAMES: a role in a policy's `Principal` list has the request its
//!   `Permission` list covers served, a role no policy names at all is refused `403` with the
//!   OpenSearch error type `forbidden` — on every attempt, not after a window — and nothing
//!   the refused write asked for lands, while adding that role to the `Principal` list makes
//!   the identical request succeed.
//!
//! The rows span what the second rule generalises over: whether the policy names the
//! principal the function runs as, which is the ONE difference between the two functions
//! here (same runtime, same inline IAM grant, same signing discipline, same request), and
//! the same principal named and then not named — the denial is asserted repeatedly for one
//! role, and the grant is then added so the identical call is served.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::query_wire::{form_encode, opt_element};

/// The Region every request here is signed for.
const REGION: &str = "us-east-1";

/// The account this emulator serves, which every ARN below carries.
const ACCOUNT: &str = ::engine::EMULATOR_ACCOUNT_ID;

/// The RPC target prefix OpenSearch Serverless names its operations under.
const TARGET_PREFIX: &str = "OpenSearchServerless";

/// The collection the handlers write into.
const COLLECTION: &str = "xsvc-fn-aoss-grant";

/// The index inside it the writes address.
const INDEX: &str = "items";

/// The data access policy the collection carries.
const DATA_POLICY: &str = "xsvc-fn-aoss-data";

/// The roles the two functions run as: one named in the policy's `Principal` list, one
/// deliberately absent from it.
const GRANTED_ROLE: &str = "xsvc-fn-aoss-granted";
const EXCLUDED_ROLE: &str = "xsvc-fn-aoss-excluded";

/// The functions that run as those roles.
const GRANTED_FUNCTION: &str = "xsvc-fn-aoss-granted-writer";
const EXCLUDED_FUNCTION: &str = "xsvc-fn-aoss-excluded-writer";

/// The stack that deploys both functions.
const STACK: &str = "xsvc-fn-aoss-stack";

/// The `StackStatus` that stack must reach.
const CREATE_COMPLETE: &str = "CREATE_COMPLETE";

/// The status a collection serves its data plane in (`CollectionStatus$ACTIVE`).
const ACTIVE: &str = "ACTIVE";

/// How often a read looks again for work the CLOCK owes — a collection settling into
/// `ACTIVE`, a stack reaching [`CREATE_COMPLETE`] — and how many looks it takes. Generous:
/// the suite waits for the CONDITION and asserts nothing about how long it took.
const LOOK: Duration = Duration::from_millis(100);
const LOOKS: usize = 1_800;

/// How long one invocation is given, as the function's own `Timeout`: well past what a run
/// needs on any host, because what is under test is the answer the data plane gives and not
/// how fast an interpreter gets through the handler.
const FUNCTION_TIMEOUT: u64 = 120;

/// How many times the excluded role's write is attempted: the refusal is the answer on
/// EVERY attempt, so more than one is asserted.
const DENIAL_ATTEMPTS: usize = 3;

/// The handler both functions run: it SIGNS one request of its own for the `aoss` signing
/// name with the credentials the runtime hands it, sends it to the endpoint it was given,
/// and reports the status and body the endpoint answered — so a refusal is the probe's
/// answer rather than an exception.
const WRITER_HANDLER: &str = r#"
import json
import os
import urllib.error
import urllib.request

import botocore.session
from botocore.auth import SigV4Auth
from botocore.awsrequest import AWSRequest


def handler(event, context):
    url = os.environ["COLLECTION_ENDPOINT"] + event["path"]
    document = event.get("document")
    body = json.dumps(document).encode("utf-8") if document is not None else b""
    signed = AWSRequest(
        method=event["method"],
        url=url,
        data=body,
        headers={"Content-Type": "application/json"},
    )
    credentials = botocore.session.get_session().get_credentials()
    SigV4Auth(credentials, "aoss", os.environ["AWS_REGION"]).add_auth(signed)
    sending = urllib.request.Request(
        signed.url, data=body if body else None, method=signed.method
    )
    for name, value in signed.headers.items():
        sending.add_header(name, value)
    try:
        with urllib.request.urlopen(sending) as answered:
            return {"status": answered.status, "body": answered.read().decode("utf-8")}
    except urllib.error.HTTPError as refused:
        return {"status": refused.code, "body": refused.read().decode("utf-8")}
"#;

/// The emulator as it is ASSEMBLED, on one engine: the request the handler signs is
/// dispatched back into this same router, so the collection it writes into is the collection
/// this suite created.
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = aws_emulator_router::build_router(engine);
    router.set_request_authentication(false);
    router
}

/// A SigV4 `Authorization` header whose credential scope names `service`, which is what
/// routes the request. The signature is never checked: request authentication is off.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=not-verified"
    )
}

/// One awsQuery call of `service`, answered with the HTTP status and the XML body.
async fn query(
    router: &MultiServiceRouter,
    service: &str,
    version: &str,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version={version}");
    for (key, value) in members {
        body.push('&');
        body.push_str(&form_encode(key));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{service}.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization(service))
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// One IAM call that must succeed.
async fn iam(router: &MultiServiceRouter, action: &str, members: &[(&str, &str)]) -> String {
    let (status, body) = query(router, "iam", "2010-05-08", action, members).await;
    assert_eq!(
        status, 200,
        "setup: IAM {action} must succeed, got {status} with {body}"
    );
    body
}

/// One OpenSearch Serverless control-plane call over its `awsJson1_0` wire.
async fn aoss(router: &MultiServiceRouter, operation: &str, body: Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("aoss.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
        .header("authorization", authorization("aoss"))
        .body(Bytes::from(body.to_string()))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, parsed)
}

/// One OpenSearch Serverless call that must succeed.
async fn aoss_ok(router: &MultiServiceRouter, operation: &str, body: Value) -> Value {
    let (status, answered) = aoss(router, operation, body).await;
    assert_eq!(
        status, 200,
        "setup: aoss {operation} must succeed, got {status} with {answered}"
    );
    answered
}

/// One Lambda `restJson1` call, as its status, its headers and its parsed body.
async fn lambda(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: &[u8],
) -> (u16, http::HeaderMap, Value) {
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("lambda.{REGION}.amazonaws.com"))
        .header("content-type", "application/json")
        .header("authorization", authorization("lambda"))
        .body(Bytes::from(body.to_vec()))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let (parts, body) = response.into_parts();
    let parsed = if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&body)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&body).to_string()))
    };
    (parts.status.as_u16(), parts.headers, parsed)
}

/// The ARN of a role of this account.
fn role_arn(name: &str) -> String {
    format!("arn:aws:iam::{ACCOUNT}:role/{name}")
}

/// The trust policy a Lambda execution role needs.
fn trust_policy() -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": {"Service": "lambda.amazonaws.com"},
            "Action": "sts:AssumeRole",
        }],
    })
    .to_string()
}

/// The identity-based grant BOTH roles carry: the one IAM action AWS's data-access
/// documentation names for reaching a collection's data plane. It is identical on the two
/// roles, so the only difference between them is the data access policy's `Principal` list.
fn api_access_all() -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Action": "aoss:APIAccessAll",
            "Resource": "*",
        }],
    })
    .to_string()
}

/// A `data` access policy document granting the collection and every index of it to
/// `principals` — the shape AWS's "Data access control for Amazon OpenSearch Serverless"
/// documents.
fn data_policy(principals: &[String]) -> String {
    json!([{
        "Rules": [
            {
                "ResourceType": "collection",
                "Resource": [format!("collection/{COLLECTION}")],
                "Permission": ["aoss:CreateCollectionItems", "aoss:DescribeCollectionItems"],
            },
            {
                "ResourceType": "index",
                "Resource": [format!("index/{COLLECTION}/*")],
                "Permission": [
                    "aoss:CreateIndex",
                    "aoss:DescribeIndex",
                    "aoss:ReadDocument",
                    "aoss:WriteDocument",
                    "aoss:UpdateIndex",
                    "aoss:DeleteIndex",
                ],
            },
        ],
        "Principal": principals,
    }])
    .to_string()
}

/// Create the collection, its network policy and its data access policy, and answer the
/// endpoint the collection publishes once it is ACTIVE beside the policy's current version.
async fn published_collection(router: &MultiServiceRouter) -> (String, String) {
    // The endpoint is reachable in this suite, so what the rows assert is the data-access
    // decision alone ("you must have at least one network policy that applies to a
    // collection in order to access its endpoint").
    aoss_ok(
        router,
        "CreateSecurityPolicy",
        json!({
            "type": "network",
            "name": "xsvc-fn-aoss-net",
            "policy": json!({
                "Rules": [{ "ResourceType": "collection", "Resource": ["collection/*"] }],
                "AllowFromPublic": true,
            })
            .to_string(),
        }),
    )
    .await;
    let created = aoss_ok(
        router,
        "CreateAccessPolicy",
        json!({
            "type": "data",
            "name": DATA_POLICY,
            "policy": data_policy(&[role_arn(GRANTED_ROLE)]),
        }),
    )
    .await;
    let version = created["accessPolicyDetail"]["policyVersion"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        !version.is_empty(),
        "setup: CreateAccessPolicy must mint a policyVersion, got {created}"
    );
    let collection = aoss_ok(
        router,
        "CreateCollection",
        json!({
            "name": COLLECTION,
            "type": "SEARCH",
            "encryptionConfig": {"aWSOwnedKey": true},
        }),
    )
    .await;
    let id = collection["createCollectionDetail"]["id"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        !id.is_empty(),
        "setup: CreateCollection must mint an id, got {collection}"
    );
    for _ in 0..LOOKS {
        let read = aoss_ok(router, "BatchGetCollection", json!({ "ids": [id] })).await;
        let detail = read["collectionDetails"][0].clone();
        if detail["status"].as_str() == Some(ACTIVE) {
            let endpoint = detail["collectionEndpoint"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            assert!(
                endpoint.starts_with("https://"),
                "setup: an ACTIVE collection must publish an endpoint, got {detail}"
            );
            return (endpoint, version);
        }
        tokio::time::sleep(LOOK).await;
    }
    panic!("setup: the collection never reached {ACTIVE}");
}

/// Create both roles and the stack that deploys the two functions against `endpoint`.
async fn deployed_functions(router: &MultiServiceRouter, endpoint: &str) {
    for name in [GRANTED_ROLE, EXCLUDED_ROLE] {
        let created = iam(
            router,
            "CreateRole",
            &[
                ("RoleName", name),
                ("AssumeRolePolicyDocument", &trust_policy()),
            ],
        )
        .await;
        assert_eq!(
            opt_element(&created, "Arn"),
            Some(role_arn(name)),
            "setup: the ARN IAM minted for {name:?} is the one the policy names, got {created}"
        );
        iam(
            router,
            "PutRolePolicy",
            &[
                ("RoleName", name),
                ("PolicyName", "aoss-api-access"),
                ("PolicyDocument", &api_access_all()),
            ],
        )
        .await;
    }
    let function = |name: &str, role: &str| {
        json!({
            "Type": "AWS::Lambda::Function",
            "Properties": {
                "FunctionName": name,
                "Runtime": "python3.12",
                "Handler": "index.handler",
                "Role": role_arn(role),
                "Timeout": FUNCTION_TIMEOUT,
                "Environment": {"Variables": {"COLLECTION_ENDPOINT": endpoint}},
                "Code": {"ZipFile": WRITER_HANDLER},
            },
        })
    };
    let template = json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Resources": {
            "Granted": function(GRANTED_FUNCTION, GRANTED_ROLE),
            "Excluded": function(EXCLUDED_FUNCTION, EXCLUDED_ROLE),
        },
    })
    .to_string();
    let (status, created) = query(
        router,
        "cloudformation",
        "2010-05-15",
        "CreateStack",
        &[
            ("StackName", STACK),
            ("TemplateBody", &template),
            ("Capabilities.member.1", "CAPABILITY_IAM"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: CreateStack must be accepted, got {status} with {created}"
    );
    let mut described = String::new();
    for _ in 0..LOOKS {
        let (_, body) = query(
            router,
            "cloudformation",
            "2010-05-15",
            "DescribeStacks",
            &[("StackName", STACK)],
        )
        .await;
        described = body;
        if opt_element(&described, "StackStatus").as_deref() == Some(CREATE_COMPLETE) {
            return;
        }
        tokio::time::sleep(LOOK).await;
    }
    panic!("setup: the functions' stack must reach {CREATE_COMPLETE}, got {described}");
}

/// What one probe reports: the status the endpoint answered and the body it carried.
#[derive(Debug)]
struct Answered {
    status: u64,
    body: Value,
    /// The whole payload, for a failure message.
    payload: Value,
}

impl Answered {
    /// The `error.type` an OpenSearch error document carries, or `""` for a body carrying
    /// none.
    fn error_type(&self) -> &str {
        self.body["error"]["type"].as_str().unwrap_or_default()
    }
}

/// Invoke `function` once with one request for it to sign and send, and answer what the
/// endpoint told it.
async fn probed(
    router: &MultiServiceRouter,
    case: &str,
    function: &str,
    method: &str,
    path: &str,
    document: Option<Value>,
) -> Answered {
    let event = json!({
        "method": method,
        "path": path,
        "document": document,
    });
    let (status, headers, payload) = lambda(
        router,
        "POST",
        &format!("/2015-03-31/functions/{function}/invocations"),
        event.to_string().as_bytes(),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the invocation must be served, got {status} with {payload}"
    );
    let faulted = headers
        .get("x-amz-function-error")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    assert_eq!(
        faulted, None,
        "case {case:?}: the handler must reach the endpoint and run to the end, got \
         {faulted:?} with {payload}"
    );
    let body = payload["body"]
        .as_str()
        .and_then(|text| serde_json::from_str::<Value>(text).ok())
        .unwrap_or(Value::Null);
    Answered {
        status: payload["status"].as_u64().unwrap_or_default(),
        body,
        payload,
    }
}

/// The whole path: the published endpoint is reachable from a running function, the policy's
/// `Principal` list decides what it answers, and adding the excluded role to that list makes
/// its identical write succeed.
#[tokio::test]
async fn the_principal_list_of_a_data_access_policy_decides_what_a_functions_write_is_answered()
{
    let router = build_router();
    let (endpoint, policy_version) = published_collection(&router).await;
    deployed_functions(&router, &endpoint).await;

    // The positive control: the role the policy NAMES has its write served out of the
    // collection's own records.
    let case = "the role the policy's Principal list names";
    let written = probed(
        &router,
        case,
        GRANTED_FUNCTION,
        "PUT",
        &format!("/{INDEX}/_doc/good-doc"),
        Some(json!({"field": "hello-search"})),
    )
    .await;
    assert!(
        (200..=201).contains(&written.status),
        "case {case:?}: a write the policy grants must be served, got {} with {}",
        written.status,
        written.payload
    );
    assert_eq!(
        written.body["result"].as_str(),
        Some("created"),
        "case {case:?}: the write receipt must report the document created, got {}",
        written.payload
    );
    // Round trip: the document is there to be read back through the same endpoint.
    let read = probed(
        &router,
        case,
        GRANTED_FUNCTION,
        "GET",
        &format!("/{INDEX}/_doc/good-doc"),
        None,
    )
    .await;
    assert_eq!(
        (read.status, read.body["_source"]["field"].as_str()),
        (200, Some("hello-search")),
        "case {case:?}: the document written must read back, got {}",
        read.payload
    );

    // The role the policy names NOWHERE is refused, with the same request, on every
    // attempt — nothing is withdrawn here and nothing was ever granted to it.
    let case = "a role no data access policy names";
    for attempt in 1..=DENIAL_ATTEMPTS {
        let refused = probed(
            &router,
            case,
            EXCLUDED_FUNCTION,
            "PUT",
            &format!("/{INDEX}/_doc/bad-doc"),
            Some(json!({"field": "should-not-be-written"})),
        )
        .await;
        assert_eq!(
            (refused.status, refused.error_type()),
            (403, "forbidden"),
            "case {case:?} (attempt {attempt} of {DENIAL_ATTEMPTS}): the write must be \
             refused 403 forbidden, got {} with {}",
            refused.status,
            refused.payload
        );
    }

    // Nothing the refused writes asked for landed: the authorized role, which may read
    // every document of the index, finds no such document.
    let absent = probed(
        &router,
        case,
        GRANTED_FUNCTION,
        "GET",
        &format!("/{INDEX}/_doc/bad-doc"),
        None,
    )
    .await;
    assert_eq!(
        absent.status, 404,
        "case {case:?}: the denied write must have landed nothing, got {} with {}",
        absent.status, absent.payload
    );

    // The decision follows the live policy state: naming the role makes the identical
    // request succeed.
    let case = "the same role once the Principal list names it";
    aoss_ok(
        &router,
        "UpdateAccessPolicy",
        json!({
            "type": "data",
            "name": DATA_POLICY,
            "policyVersion": policy_version,
            "policy": data_policy(&[role_arn(GRANTED_ROLE), role_arn(EXCLUDED_ROLE)]),
        }),
    )
    .await;
    let written = probed(
        &router,
        case,
        EXCLUDED_FUNCTION,
        "PUT",
        &format!("/{INDEX}/_doc/bad-doc"),
        Some(json!({"field": "now-granted"})),
    )
    .await;
    assert!(
        (200..=201).contains(&written.status),
        "case {case:?}: the identical write must be served once the policy names the role, \
         got {} with {}",
        written.status,
        written.payload
    );
    assert_eq!(
        written.body["result"].as_str(),
        Some("created"),
        "case {case:?}: the write receipt must report the document created, got {}",
        written.payload
    );
}
