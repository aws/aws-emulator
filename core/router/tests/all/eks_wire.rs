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

/// The error code EC2 answers for a subnet id no subnet of the account carries, which is
/// what an Amazon EKS refusal carries through.
pub(crate) const EC2_NOT_FOUND_CODE: &str = "InvalidSubnetID.NotFound";

/// A SigV4 `Authorization` header whose credential scope names `signing_name`.
/// The signature is never checked: these suites run with authentication off.
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{signing_name}/aws4_request, \
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
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://{signing_name}.{REGION}.amazonaws.com{uri}"))
        .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
        .header("authorization", authorization(signing_name))
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
    send(
        router,
        runtime,
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

/// A `CreateCluster` body naming `subnets`, and nothing else beyond the required members.
pub(crate) fn cluster_body(name: &str, subnets: &[String]) -> serde_json::Value {
    serde_json::json!({
        "name": name,
        "roleArn": role_arn("eksClusterRole"),
        "resourcesVpcConfig": { "subnetIds": subnets },
    })
}
