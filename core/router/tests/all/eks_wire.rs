// SPDX-License-Identifier: CC0-1.0

//! The request harness every suite that drives Amazon EKS beside the services it resolves
//! its inputs against needs: the SigV4 header those requests carry, the answer a client
//! branches on, and one driver per wire protocol in play (EKS speaks restJson1, EC2
//! ec2Query, IAM awsQuery).
//!
//! It lives here, ONCE, for the reason `query_wire.rs` gives for the wire readers it owns:
//! a suite that writes its own drifts from the others, and two suites asserting the same
//! cross-service rule through two harnesses are no longer asserting the same thing. The
//! percent-encoder and the XML element readers come from `query_wire.rs` rather than being
//! written again here.
//!
//! Each suite keeps what is its own: its stack size, its tables, and the assertions it
//! makes about an answer.

use smithy_server_runtime::MultiServiceRouter;

use crate::query_wire::form_encode;

/// The region every request built here is signed for.
pub(crate) const REGION: &str = "us-west-2";

/// The EC2 API version a client sends in an ec2Query body.
const EC2_API_VERSION: &str = "2016-11-15";

/// The IAM API version a client sends in an awsQuery body.
const IAM_API_VERSION: &str = "2010-05-08";

/// The Auto Scaling API version a client sends in an awsQuery body.
const AUTOSCALING_API_VERSION: &str = "2011-01-01";

/// The STS API version a client sends in an awsQuery body.
const STS_API_VERSION: &str = "2011-06-15";

/// The error code EC2 answers for a subnet id no subnet of the account carries, which is
/// what an Amazon EKS refusal carries through.
pub(crate) const EC2_NOT_FOUND_CODE: &str = "InvalidSubnetID.NotFound";

/// The access key id a request carries unless a suite names another one — a key no
/// identity of the account holds, which is the ambient caller.
pub(crate) const DEFAULT_ACCESS_KEY_ID: &str = "AKIDEXAMPLE";

/// A SigV4 `Authorization` header whose credential scope names `signing_name` and
/// `access_key_id`. The signature is never checked: these suites run with authentication
/// off, and the access key id is what resolves the calling principal.
fn authorization(access_key_id: &str, signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential={access_key_id}/20260101/{REGION}/{signing_name}/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature=deadbeef"
    )
}

/// One answer, reduced to what a client branches on.
#[derive(Debug)]
pub(crate) struct Answer {
    pub(crate) status: u16,
    /// The error code, from the `x-amzn-errortype` header or the JSON envelope. A query
    /// protocol carries it in the XML body instead, which `query_wire::opt_element` reads.
    pub(crate) error_type: Option<String>,
    pub(crate) text: String,
    pub(crate) body: serde_json::Value,
}

impl Answer {
    /// The error message, whichever casing the envelope carries it under.
    pub(crate) fn message(&self) -> String {
        self.body
            .get("message")
            .or_else(|| self.body.get("Message"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string()
    }
}

/// Drive one request at the service `signing_name` claims.
pub(crate) fn send(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    signing_name: &str,
    method: &str,
    uri: &str,
    content_type: &str,
    body: String,
) -> Answer {
    send_as(
        router,
        runtime,
        DEFAULT_ACCESS_KEY_ID,
        signing_name,
        method,
        uri,
        content_type,
        body,
    )
}

/// [`send`], signed (in the routing sense) with `access_key_id` — the credential the
/// service resolves the calling principal from, which for a key a session was minted
/// under is that session.
#[allow(clippy::too_many_arguments)]
pub(crate) fn send_as(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    access_key_id: &str,
    signing_name: &str,
    method: &str,
    uri: &str,
    content_type: &str,
    body: String,
) -> Answer {
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://{signing_name}.{REGION}.amazonaws.com{uri}"))
        .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
        .header("authorization", authorization(access_key_id, signing_name))
        .header("content-type", content_type)
        .body(bytes::Bytes::from(body))
        .expect("a well-formed request");
    let response = runtime.block_on(router.dispatch(request));
    let status = response.status().as_u16();
    let header_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let text = String::from_utf8_lossy(response.body()).to_string();
    let body = serde_json::from_str(&text).unwrap_or(serde_json::Value::Null);
    let error_type = header_type.or_else(|| {
        body.get("__type")
            .and_then(serde_json::Value::as_str)
            .map(|value| value.rsplit(['#', ':']).next().unwrap_or("").to_string())
    });
    Answer {
        status,
        error_type,
        text,
        body,
    }
}

/// Drive one EKS request (restJson1).
pub(crate) fn eks(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    method: &str,
    uri: &str,
    body: Option<serde_json::Value>,
) -> Answer {
    let payload = body.map(|json| json.to_string()).unwrap_or_default();
    send(
        router,
        runtime,
        "eks",
        method,
        uri,
        "application/json",
        payload,
    )
}

/// One form body: `Action`, `Version` and the parameters, each component encoded by the
/// one encoder this test crate owns.
fn form_body(action: &str, version: &str, params: &[(&str, &str)]) -> String {
    let mut body = format!("Action={action}&Version={version}");
    for (name, value) in params {
        body.push_str(&format!("&{name}={}", form_encode(value)));
    }
    body
}

/// Drive one ec2Query action: `POST /` with the parameters in the form body.
pub(crate) fn ec2(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    action: &str,
    params: &[(&str, &str)],
) -> Answer {
    send(
        router,
        runtime,
        "ec2",
        "POST",
        "/",
        "application/x-www-form-urlencoded",
        form_body(action, EC2_API_VERSION, params),
    )
}

/// Drive one awsQuery IAM action.
pub(crate) fn iam(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    action: &str,
    params: &[(&str, &str)],
) -> Answer {
    iam_as(router, runtime, DEFAULT_ACCESS_KEY_ID, action, params)
}

/// Drive one awsQuery IAM action as the holder of `access_key_id`.
pub(crate) fn iam_as(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    access_key_id: &str,
    action: &str,
    params: &[(&str, &str)],
) -> Answer {
    send_as(
        router,
        runtime,
        access_key_id,
        "iam",
        "POST",
        "/",
        "application/x-www-form-urlencoded",
        form_body(action, IAM_API_VERSION, params),
    )
}

/// Drive one awsQuery Auto Scaling action — the service a managed node group's
/// capacity lives in.
pub(crate) fn autoscaling(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    action: &str,
    params: &[(&str, &str)],
) -> Answer {
    send(
        router,
        runtime,
        "autoscaling",
        "POST",
        "/",
        "application/x-www-form-urlencoded",
        form_body(action, AUTOSCALING_API_VERSION, params),
    )
}

/// A role ARN of the account this emulator serves, under the default path.
pub(crate) fn role_arn(name: &str) -> String {
    format!("arn:aws:iam::{}:role/{name}", engine::emulator_account_id())
}

/// Drive one awsQuery STS action — the service a projected service account token is
/// exchanged for a role session at.
pub(crate) fn sts(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    action: &str,
    params: &[(&str, &str)],
) -> Answer {
    sts_as(router, runtime, DEFAULT_ACCESS_KEY_ID, action, params)
}

/// [`sts`] as the holder of `access_key_id` — for a key a session was minted under, that
/// session.
pub(crate) fn sts_as(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    access_key_id: &str,
    action: &str,
    params: &[(&str, &str)],
) -> Answer {
    send_as(
        router,
        runtime,
        access_key_id,
        "sts",
        "POST",
        "/",
        "application/x-www-form-urlencoded",
        form_body(action, STS_API_VERSION, params),
    )
}

/// The base64url form of `text`: the two alphabet substitutions and the dropped
/// padding of RFC 7515 §2.
///
/// Written out here rather than taken from the workspace's one codec for the reason
/// `query_wire.rs` gives for its own encoder: `core/router/Cargo.toml` is a pinned
/// path in this repository, so this test crate can take no dependency at all.
pub(crate) fn base64url(text: &str) -> String {
    aws_smithy_types::base64::encode(text.as_bytes())
        .replace('+', "-")
        .replace('/', "_")
        .replace('=', "")
}

/// The bearer token a kubeconfig's credential plugin builds for the credential
/// `access_key_id`: `k8s-aws-v1.` followed by the base64url form of a presigned
/// `sts:GetCallerIdentity` request, whose SigV4 credential scope names that key.
///
/// A cluster's Kubernetes API server authenticates every request from the token it
/// carries, so every suite that dials a published cluster endpoint presents one.
pub(crate) fn kubernetes_bearer_token(access_key_id: &str) -> String {
    format!(
        "k8s-aws-v1.{}",
        base64url(&format!(
            "https://sts.{REGION}.amazonaws.com/?Action=GetCallerIdentity&Version=2011-06-15\
             &X-Amz-Algorithm=AWS4-HMAC-SHA256&X-Amz-Credential={access_key_id}%2F20260101%2F\
             {REGION}%2Fsts%2Faws4_request&X-Amz-Date=20260101T000000Z&X-Amz-Expires=60\
             &X-Amz-SignedHeaders=host%3Bx-k8s-aws-id&X-Amz-Signature=0000"
        ))
    )
}

/// The token a client built from the ACCOUNT's own published credential presents,
/// which is the caller a suite dials a cluster's Kubernetes API as unless it names
/// another.
pub(crate) fn account_kubernetes_bearer_token() -> String {
    kubernetes_bearer_token(DEFAULT_ACCESS_KEY_ID)
}

/// A `CreateCluster` body naming `subnets`, and nothing else beyond the required members.
pub(crate) fn cluster_body(name: &str, subnets: &[String]) -> serde_json::Value {
    serde_json::json!({
        "name": name,
        "roleArn": role_arn("eksClusterRole"),
        "resourcesVpcConfig": { "subnetIds": subnets },
    })
}
