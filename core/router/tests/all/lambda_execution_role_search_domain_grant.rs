// SPDX-License-Identifier: CC0-1.0

//! What a RUNNING function's own signed request to an Amazon OpenSearch Service domain is
//! authorized against: the identity-based policies in force on the function's execution
//! role, beside the domain's own access policy.
//!
//! RULE: a signed data-plane request to a domain is authorized ONCE, against the resource
//! the request ADDRESSES (`domain/<name>/<the path it carries>`) and on the policy evidence
//! this emulator can read: it is admitted when either the policies in force on the calling
//! principal or the domain's access policy allows the `es:ESHttp*` action the request's
//! method asks for on that path — whichever KIND of identity policy carries the grant (a
//! policy embedded in the role, a managed policy of the account attached to it, or one of
//! AWS's own whose document this emulator cannot read) — and refused `403` only once the
//! grants this emulator CAN read say nothing about the path and the domain's policy does not
//! allow it either.
//!
//! Five crates have to agree for that to hold — IAM holds the role and the policies attached
//! to it, STS mints the session the handler's own request is signed as, Lambda runs the
//! handler, CloudFormation deploys the function, and Amazon OpenSearch Service takes the
//! decision — so the suite lives here.
//!
//! The rows differ along what the rule generalises over: WHERE the role's grant lives
//! (embedded in the role, an account-managed policy attached to it, an AWS-managed policy
//! attached to it, none at all) and WHICH resource its statement names (every path of the
//! domain, every path of the index the handler writes to, every path of another index). Each
//! row is probed twice — with the grant in place, and again once it has been removed the way
//! that kind of grant is removed — so a refusal that arrives while the grant is in force
//! fails here just as a write that is still served after the grant is gone does.
//!
//! Every wait here is for a CONDITION and never for a span: the domain publishing an
//! endpoint, the stack reaching `CREATE_COMPLETE`, the function reaching `Active`, and Lambda
//! having room to serve a probe's invocation at all. Each is looked at again until it holds
//! or a generous deadline passes, so what the rows assert — which writes the domain admits
//! and which it refuses — cannot be decided by how fast or how loaded this host happens to
//! be.

use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::query_wire::{form_encode, opt_element};

/// The Region every request here is signed for.
const REGION: &str = "us-east-1";

/// The account this emulator serves, which every ARN below carries.
const ACCOUNT: &str = ::engine::EMULATOR_ACCOUNT_ID;

/// The domain every row writes into.
const DOMAIN: &str = "xsvc-fn-grant-domain";

/// The engine release the domain runs.
const ENGINE_VERSION: &str = "OpenSearch_2.11";

/// The index the handler posts documents into.
const INDEX: &str = "grants";

/// Another index of the same domain, which one row's statement names instead.
const OTHER_INDEX: &str = "elsewhere";

/// How often to look while waiting for work the clock owes — the domain coming up, the
/// stack reaching `CREATE_COMPLETE`, the function reaching `Active` — and how many looks to
/// take. [`LOOK`] is also the span between a probe's looks, which is bounded by a deadline of
/// its own ([`SERVED_DEADLINE`]). The spans waited on are those services' own, so nothing
/// here sleeps for a fixed span expecting work to be done, and every deadline is generous so
/// a loaded host cannot decide the outcome.
const LOOK: Duration = Duration::from_millis(100);
const LOOKS: usize = 1_800;

/// The `StackStatus` the function's stack must reach.
const CREATE_COMPLETE: &str = "CREATE_COMPLETE";

/// How long one invocation is given, as the function's own `Timeout`: well past what a run
/// needs on any host, because what is under test is the authorization decision and not how
/// fast an interpreter gets through the handler.
const FUNCTION_TIMEOUT: u64 = 120;

/// The role the DOMAIN's access policy names — a separate control role, not the role the
/// function runs as, so the domain carries an access policy that says nothing about the
/// caller and every grant under test is the caller's own identity policy.
const CONTROL_ROLE: &str = "xsvc-fn-grant-control";

/// The role the function runs as.
const EXECUTION_ROLE: &str = "xsvc-fn-grant-execution";

/// The function the handler runs in.
const FUNCTION: &str = "xsvc-fn-grant-writer";

/// The policy name a row's embedded document is held under.
const INLINE_POLICY: &str = "search-write";

/// The account-managed policy one row attaches, named for the row so each is its own record.
fn managed_policy(ordinal: usize) -> String {
    format!("xsvc-fn-grant-managed-{ordinal}")
}

/// An AWS-managed policy's ARN, whose document is AWS's own: this emulator publishes a
/// catalog of such documents and this one is not in it, so what it grants cannot be read
/// here. Named from the IAM console's own list of Amazon OpenSearch Service policies.
const AWS_MANAGED_POLICY_ARN: &str = "arn:aws:iam::aws:policy/AmazonOpenSearchServiceFullAccess";

/// The handler every probe runs: it SIGNS an index request of its own with the credentials
/// the runtime hands it and reports the status the domain answered, so a refusal is the
/// probe's answer rather than an exception.
const WRITER_HANDLER: &str = r#"
import json
import os
import urllib.error
import urllib.request

import botocore.session
from botocore.auth import SigV4Auth
from botocore.awsrequest import AWSRequest


def handler(event, context):
    endpoint = os.environ["SEARCH_ENDPOINT"]
    index = event["index"]
    body = json.dumps(event["document"]).encode("utf-8")
    url = "https://" + endpoint + "/" + index + "/_doc"
    signed = AWSRequest(
        method="POST", url=url, data=body, headers={"Content-Type": "application/json"}
    )
    credentials = botocore.session.get_session().get_credentials()
    SigV4Auth(credentials, "es", os.environ["AWS_REGION"]).add_auth(signed)
    sending = urllib.request.Request(signed.url, data=body, method=signed.method)
    for name, value in signed.headers.items():
        sending.add_header(name, value)
    try:
        with urllib.request.urlopen(sending) as answered:
            return {"status": answered.status, "body": answered.read().decode("utf-8")}
    except urllib.error.HTTPError as refused:
        return {"status": refused.code, "error": refused.read().decode("utf-8")}
"#;

/// The emulator as it is ASSEMBLED, on one engine: the request the handler signs is
/// dispatched back into this same router, so the domain it writes into is the domain this
/// suite created.
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

/// One IAM call, which every change to the grants in force goes through.
async fn iam(
    router: &MultiServiceRouter,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    query(router, "iam", "2010-05-08", action, members).await
}

/// One Amazon OpenSearch Service control-plane call over its `restJson1` wire.
async fn opensearch(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(path)
        .header("host", format!("es.{REGION}.amazonaws.com"))
        .header("authorization", authorization("es"));
    let payload = match body {
        Some(document) => {
            builder = builder.header("content-type", "application/json");
            Bytes::from(document.to_string())
        }
        None => Bytes::new(),
    };
    let request = builder.body(payload).expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, parsed)
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

/// The ARN of an account-managed policy of this account.
fn policy_arn(name: &str) -> String {
    format!("arn:aws:iam::{ACCOUNT}:policy/{name}")
}

/// The domain's ARN, which every policy document here names as its resource.
fn domain_arn() -> String {
    format!("arn:aws:es:{REGION}:{ACCOUNT}:domain/{DOMAIN}")
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

/// The document that grants the data-plane write on this domain, as a client writes one:
/// the `es:ESHttp*` actions on `resource`.
fn write_grant(resource: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Action": ["es:ESHttpPost", "es:ESHttpPut", "es:ESHttpGet"],
            "Resource": resource,
        }],
    })
    .to_string()
}

/// The access policy the DOMAIN carries: a document naming the separate control role, so the
/// domain's own policy says nothing about the role the function runs as.
fn domain_access_policy() -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": {"AWS": role_arn(CONTROL_ROLE)},
            "Action": "es:ESHttp*",
            "Resource": format!("{}/*", domain_arn()),
        }],
    })
    .to_string()
}

/// Create the domain every row writes into and wait until the service publishes an endpoint
/// for it, which is the address the handler signs its request to.
async fn created_domain(router: &MultiServiceRouter) -> String {
    let (status, created) = opensearch(
        router,
        "POST",
        "/2021-01-01/opensearch/domain",
        Some(json!({
            "DomainName": DOMAIN,
            "EngineVersion": ENGINE_VERSION,
            "ClusterConfig": {"InstanceType": "t3.small.search", "InstanceCount": 1},
            "EBSOptions": {"EBSEnabled": true, "VolumeType": "gp3", "VolumeSize": 10},
            "AccessPolicies": domain_access_policy(),
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: CreateDomain({DOMAIN:?}) must succeed, got {status} with {created}"
    );
    let mut described = Value::Null;
    for _ in 0..LOOKS {
        let (_, answered) = opensearch(
            router,
            "GET",
            &format!("/2021-01-01/opensearch/domain/{DOMAIN}"),
            None,
        )
        .await;
        described = answered;
        let published = described["DomainStatus"]["Endpoint"].as_str().unwrap_or_default();
        if !published.is_empty() {
            return published.to_string();
        }
        tokio::time::sleep(LOOK).await;
    }
    panic!("setup: the domain must publish an endpoint once it is up, got {described}");
}

/// Create the execution role and the stack that deploys the function, and answer the address
/// the function writes to.
async fn deployed_function(router: &MultiServiceRouter, endpoint: &str) {
    for name in [EXECUTION_ROLE, CONTROL_ROLE] {
        let (status, created) = iam(
            router,
            "CreateRole",
            &[
                ("RoleName", name),
                ("AssumeRolePolicyDocument", &trust_policy()),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "setup: CreateRole({name:?}) must succeed, got {status} with {created}"
        );
        assert_eq!(
            opt_element(&created, "Arn"),
            Some(role_arn(name)),
            "setup: the ARN IAM minted for {name:?} is the one the documents name, got {created}"
        );
    }
    let template = json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Resources": {
            "Writer": {
                "Type": "AWS::Lambda::Function",
                "Properties": {
                    "FunctionName": FUNCTION,
                    "Runtime": "python3.12",
                    "Handler": "index.handler",
                    "Role": role_arn(EXECUTION_ROLE),
                    "Timeout": FUNCTION_TIMEOUT,
                    "Environment": {"Variables": {"SEARCH_ENDPOINT": endpoint}},
                    "Code": {"ZipFile": WRITER_HANDLER},
                },
            },
        },
    })
    .to_string();
    let (status, created) = query(
        router,
        "cloudformation",
        "2010-05-15",
        "CreateStack",
        &[
            ("StackName", "xsvc-fn-grant-stack"),
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
    let mut deployed = false;
    for _ in 0..LOOKS {
        let (_, body) = query(
            router,
            "cloudformation",
            "2010-05-15",
            "DescribeStacks",
            &[("StackName", "xsvc-fn-grant-stack")],
        )
        .await;
        described = body;
        if opt_element(&described, "StackStatus").as_deref() == Some(CREATE_COMPLETE) {
            deployed = true;
            break;
        }
        tokio::time::sleep(LOOK).await;
    }
    assert!(
        deployed,
        "setup: the function's stack must reach {CREATE_COMPLETE}, got {described}"
    );
    activated_function(router).await;
}

/// The `State` a function has to be in to take an invocation: the model documents the
/// provisioning window a newly created function passes through first on `CreateFunction`
/// ("During this time, you can't invoke or modify the function"), and `Invoke` answers
/// `ResourceConflictException` for a call that arrives inside it.
const STATE_ACTIVE: &str = "Active";

/// Wait until Lambda reports the deployed function as [`STATE_ACTIVE`], which is the
/// condition a probe's invocation really needs rather than the one the stack's own status
/// states: the stack is complete once CloudFormation has created the function, while the
/// function's own provisioning is settled by a span of its service's clock. Waiting for the
/// function's state here is what keeps the first probe from racing that span, whichever of
/// the two spans this host happens to finish first.
async fn activated_function(router: &MultiServiceRouter) {
    let mut read = (0, Value::Null);
    for _ in 0..LOOKS {
        let (status, _, answered) = lambda(
            router,
            "GET",
            &format!("/2015-03-31/functions/{FUNCTION}/configuration"),
            &[],
        )
        .await;
        read = (status, answered);
        if read.1["State"].as_str() == Some(STATE_ACTIVE) {
            return;
        }
        tokio::time::sleep(LOOK).await;
    }
    let (status, answered) = read;
    panic!(
        "setup: the deployed function must reach {STATE_ACTIVE}, last read answered {status} \
         with {answered}"
    );
}

/// The statuses Lambda answers an invocation it has NOT served with, each of which says "not
/// now" and carries no decision about the write: the throughput limit of the account's
/// concurrent executions was reached (`TooManyRequestsException`, `@httpError(429)`), or the
/// function is not `Active` yet (`ResourceConflictException`, `@httpError(409)`, the window
/// `CreateFunction` documents).
///
/// Whether one arrives is decided by what the rest of this process is running at that instant
/// and by nothing a row varies, so a probe LOOKS AGAIN on one — the condition it is waiting
/// for is its invocation being served — instead of reading an authorization decision out of a
/// run that never happened. Nothing is tolerated by looking again: a refusal that still
/// stands at the deadline fails the row, carrying the status and the payload it answered
/// with. Every OTHER answer fails the row at once, the status Lambda gives a run it could not
/// drive and a handler the sandbox stopped included: those are defects of what is under test
/// rather than a service with no room right now.
const UNSERVED: &[u16] = &[409, 429];

/// How long a probe waits for Lambda to serve its invocation: the same generous span the
/// status polls here take ([`LOOK`] over [`LOOKS`] looks), read as a DEADLINE rather than as
/// a number of looks, because one look of this wait is a whole invocation and not a cheap
/// read — so the span this states is the span it takes.
const SERVED_DEADLINE: Duration = Duration::from_secs(180);

/// Invoke the function and answer what the handler reported: the status the domain gave its
/// signed write, and the whole payload for a failure message.
///
/// Looks again, every [`LOOK`] until [`SERVED_DEADLINE`] passes, for as long as Lambda has
/// not served the invocation ([`UNSERVED`]) — so what this answers is always a decision the
/// domain took about a handler that ran, on a loaded host as on an idle one. Every other
/// answer is asserted on exactly as it was: a handler that raised, or a status the domain
/// never gives, fails the row it belongs to.
async fn probed_write(router: &MultiServiceRouter, case: &str, document_id: &str) -> (u64, Value) {
    let event = json!({
        "index": INDEX,
        "document": {"id": document_id, "case": case},
    });
    let deadline = Instant::now() + SERVED_DEADLINE;
    loop {
        let (status, headers, payload) = lambda(
            router,
            "POST",
            &format!("/2015-03-31/functions/{FUNCTION}/invocations"),
            event.to_string().as_bytes(),
        )
        .await;
        if UNSERVED.contains(&status) {
            assert!(
                Instant::now() < deadline,
                "case {case:?}: Lambda must serve this probe's invocation, last answered \
                 {status} with {payload}"
            );
            tokio::time::sleep(LOOK).await;
            continue;
        }
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
            "case {case:?}: the handler must run to the end, got {faulted:?} with {payload}"
        );
        let answered = payload["status"].as_u64().unwrap_or_default();
        return (answered, payload);
    }
}

/// WHERE one row's grant lives, which is one of the two axes a row varies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Grant {
    /// A policy document embedded in the role itself (`PutRolePolicy`).
    Embedded,
    /// An account-managed policy attached to the role (`AttachRolePolicy`).
    AccountManaged,
    /// One of AWS's own managed policies attached to the role, whose document this emulator
    /// does not hold: a grant it cannot read, which is not evidence of no permission.
    AwsManaged,
    /// No grant at all.
    None,
}

/// WHICH resource the row's statement names — the other axis. A data-plane statement of
/// this service scopes itself to a path under the domain's ARN
/// (`arn:aws:es:<region>:<account>:domain/<name>/<path>`, "Actions defined by Amazon
/// OpenSearch Service" in the IAM Service Authorization Reference), which is what the
/// request's own path is matched against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Scope {
    /// Every path of the domain.
    Domain,
    /// Every path under the index the handler writes to — a least-privilege statement, which
    /// covers the document path the write addresses and nothing above it.
    Index,
    /// Every path under ANOTHER index of the same domain.
    OtherIndex,
}

impl Scope {
    /// The `Resource` element a statement of this scope carries.
    fn resource(self) -> String {
        match self {
            Self::Domain => format!("{}/*", domain_arn()),
            Self::Index => format!("{}/{INDEX}/*", domain_arn()),
            Self::OtherIndex => format!("{}/{OTHER_INDEX}/*", domain_arn()),
        }
    }
}

/// One row: where the grant lives, which resource it names, and whether a write is expected
/// to be admitted with the grant in place.
struct Row {
    case: &'static str,
    grant: Grant,
    scope: Scope,
    /// Whether the write must be admitted while the grant is in force.
    admitted: bool,
}

const ROWS: &[Row] = &[
    Row {
        case: "a document embedded in the execution role, over every path of the domain",
        grant: Grant::Embedded,
        scope: Scope::Domain,
        admitted: true,
    },
    Row {
        case: "the same document scoped to the index the handler writes to",
        grant: Grant::Embedded,
        scope: Scope::Index,
        admitted: true,
    },
    Row {
        case: "an account-managed policy attached to the role, scoped to that index",
        grant: Grant::AccountManaged,
        scope: Scope::Index,
        admitted: true,
    },
    Row {
        case: "an AWS-managed policy attached to the role, whose document AWS owns",
        grant: Grant::AwsManaged,
        scope: Scope::Domain,
        admitted: true,
    },
    Row {
        case: "a document scoped to ANOTHER index of the same domain",
        grant: Grant::Embedded,
        scope: Scope::OtherIndex,
        admitted: false,
    },
    Row {
        case: "an execution role carrying no grant of this service at all",
        grant: Grant::None,
        scope: Scope::Domain,
        admitted: false,
    },
];

/// Put one row's grant in force. `ordinal` names the row, so each account-managed policy is
/// the row's own record rather than a second version of one name.
async fn grant(router: &MultiServiceRouter, row: &Row, ordinal: usize) {
    let case = row.case;
    let document = write_grant(&row.scope.resource());
    match row.grant {
        Grant::Embedded => {
            let (status, body) = iam(
                router,
                "PutRolePolicy",
                &[
                    ("RoleName", EXECUTION_ROLE),
                    ("PolicyName", INLINE_POLICY),
                    ("PolicyDocument", &document),
                ],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: PutRolePolicy must succeed, got {status} with {body}"
            );
        }
        Grant::AccountManaged => {
            let name = managed_policy(ordinal);
            let (status, body) = iam(
                router,
                "CreatePolicy",
                &[("PolicyName", &name), ("PolicyDocument", &document)],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: CreatePolicy must succeed, got {status} with {body}"
            );
            let arn = opt_element(&body, "Arn").unwrap_or_default();
            assert_eq!(
                arn,
                policy_arn(&name),
                "case {case:?}: the ARN IAM minted for the policy is the one the attachment \
                 names, got {body}"
            );
            let (status, body) = iam(
                router,
                "AttachRolePolicy",
                &[("RoleName", EXECUTION_ROLE), ("PolicyArn", &arn)],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: AttachRolePolicy must succeed, got {status} with {body}"
            );
        }
        Grant::AwsManaged => {
            let (status, body) = iam(
                router,
                "AttachRolePolicy",
                &[
                    ("RoleName", EXECUTION_ROLE),
                    ("PolicyArn", AWS_MANAGED_POLICY_ARN),
                ],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: AttachRolePolicy must succeed, got {status} with {body}"
            );
        }
        Grant::None => {}
    }
}

/// Take one row's grant back out of force, the way that kind of grant is removed.
async fn revoke(router: &MultiServiceRouter, row: &Row, ordinal: usize) {
    let case = row.case;
    let (action, members): (&str, Vec<(&str, String)>) = match row.grant {
        Grant::Embedded => (
            "DeleteRolePolicy",
            vec![
                ("RoleName", EXECUTION_ROLE.to_string()),
                ("PolicyName", INLINE_POLICY.to_string()),
            ],
        ),
        Grant::AccountManaged => (
            "DetachRolePolicy",
            vec![
                ("RoleName", EXECUTION_ROLE.to_string()),
                ("PolicyArn", policy_arn(&managed_policy(ordinal))),
            ],
        ),
        Grant::AwsManaged => (
            "DetachRolePolicy",
            vec![
                ("RoleName", EXECUTION_ROLE.to_string()),
                ("PolicyArn", AWS_MANAGED_POLICY_ARN.to_string()),
            ],
        ),
        Grant::None => return,
    };
    let borrowed: Vec<(&str, &str)> = members
        .iter()
        .map(|(key, value)| (*key, value.as_str()))
        .collect();
    let (status, body) = iam(router, action, &borrowed).await;
    assert_eq!(
        status, 200,
        "case {case:?}: {action} must succeed, got {status} with {body}"
    );
}

/// Every row: the write is admitted while a grant that covers the path it addresses is in
/// force, whichever kind of policy carries it, and refused `403` once that grant is gone.
#[tokio::test]
async fn an_identity_grant_admits_the_handlers_signed_write_and_its_removal_refuses_it() {
    let router = build_router();
    let endpoint = created_domain(&router).await;
    deployed_function(&router, &endpoint).await;

    for (ordinal, row) in ROWS.iter().enumerate() {
        let case = row.case;
        grant(&router, row, ordinal).await;
        let (answered, payload) = probed_write(&router, case, &format!("granted-{ordinal}")).await;
        if row.admitted {
            assert!(
                (200..=201).contains(&answered),
                "case {case:?}: a write the role's own policy allows on the path it addresses \
                 must be admitted, got {answered} with {payload}"
            );
        } else {
            assert_eq!(
                answered, 403,
                "case {case:?}: a write nothing in force allows must be refused 403, got \
                 {answered} with {payload}"
            );
        }

        revoke(&router, row, ordinal).await;
        let (answered, payload) = probed_write(&router, case, &format!("revoked-{ordinal}")).await;
        assert_eq!(
            answered, 403,
            "case {case:?}: with the grant out of force the same write must be refused 403, \
             got {answered} with {payload}"
        );
    }
}
