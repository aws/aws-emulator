// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what a FRONT api's PRIVATE proxy integration serves: an `HTTP_PROXY`
//! integration whose `connectionType` is `VPC_LINK`, carried over a provisioned VPC link to a
//! PRIVATE api's deployed stage, which is the pattern a client stands up to reach a private
//! REST API through an interface VPC endpoint.
//!
//! The RULE under test, one level above any one recorded call:
//!
//! > An `HTTP_PROXY` integration — "for integrating the API method request with an HTTP
//! > endpoint … with the client request passed through as-is" (`IntegrationType`) — answers
//! > with the BACKEND'S OWN response, its status and its body mapped by nothing, whatever
//! > that status is; and the backend's own resource policy decides the request that was
//! > passed on, so a `StringEquals` `aws:SourceVpce` condition is evaluated against the
//! > interface VPC endpoint that onward request arrives through: a match is served the
//! > backend method's own answer, and a mismatch is `403` carrying the `execute-api:Invoke`
//! > authorization sentence about the BACKEND method the onward request addressed — never
//! > the front method — and none of the backend's body.
//!
//! The rows span what that rule generalises over: the STATUS the backend method answers —
//! two success classes, a client-error class and a server-error class, one deployed resource
//! each, so a status passed through is told apart from a `200` invented by the front api —
//! and the endpoint the backend's policy is pinned to as the call is repeated: the one the
//! request arrives through, another endpoint of the account, and the first one again, which
//! is what shows the gate is read out of the document rather than decided once.
//!
//! Every case drives the real wire path — an HTTP request through the multi-service router
//! into the mounted service — and every record is written through the control plane
//! (`CreateRestApi`, `PutMethod`, `PutIntegration`, `PutMethodResponse`,
//! `PutIntegrationResponse`, `CreateVpcLink`, `UpdateRestApi`, `CreateDeployment`), so
//! nothing here reaches into state.
//!
//! [`Gateway`], [`create_api`], [`string_member`] and the `MOCK`/invoke helpers are this
//! crate's one wire-test harness, reused rather than stood up a second way.

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_apigateway_backplanecontrolservice::state::VPC_LINK_AVAILABLE;

use crate::execute_api_data_plane::{
    deploy, invoke_on_host, json_template, mock_resource, put_integration, put_method,
};
use crate::rest_api_lifecycle::{create_api, string_member, Gateway};

/// The Region every host in this suite is addressed in, which is the one the harness's
/// control-plane calls are signed for — so the api is created in the Region its ARNs name.
const REGION: &str = "us-east-1";

/// The account id this emulator mints every ARN under, which is therefore the account an
/// authorization refusal names the method ARN in.
const ACCOUNT: &str = "000000000000";

/// The stage the PRIVATE backend api is deployed to.
const PRIVATE_STAGE: &str = "s";

/// The stage each front api is deployed to.
const FRONT_STAGE: &str = "live";

/// The one value every backend method answers with — what proves a request reached the
/// backend's integration, so a refusal must never carry it.
const NONCE: &str = "9d42be";

/// The interface VPC endpoint the backend is reached through, in EC2's own id form (`vpce-`
/// and 17 lowercase hexadecimal digits).
const ENDPOINT: &str = "vpce-0123456789abcdef0";

/// Another endpoint of the same account: the one the policy is re-pinned to so the SAME
/// request becomes a refusal, because it is not the endpoint the request arrives through.
const OTHER_ENDPOINT: &str = "vpce-00000000000000000";

/// A network load balancer of this account, as Elastic Load Balancing writes its ARN
/// (`loadbalancer/net/{name}/{id}`), which is what a VPC link is provisioned against.
const LOAD_BALANCER_ARN: &str =
    "arn:aws:elasticloadbalancing:us-east-1:000000000000:loadbalancer/net/private-proxy/0a1b2c3d";

/// How long the VPC link's provisioning is given before it is called a non-convergence.
///
/// `CreateVpcLink` is "an asynchronous operation", so the link settles from the CLOCK alone:
/// this is a deliberately generous ceiling on a loaded machine, not the span itself, which
/// the loop below never waits out — it polls for the status and stops as soon as it holds.
const LINK_DEADLINE: Duration = Duration::from_secs(30);

/// How long the poll below waits between reads of the link's status.
const LINK_POLL_INTERVAL: Duration = Duration::from_millis(25);

/// The principal a refusal names for a request to a method that authorizes nobody, which is
/// what `authorizationType` `NONE` makes every caller.
const ANONYMOUS: &str = "anonymous";

/// The statuses a backend method answers, one deployed resource each: the success an api
/// answers normally, another success class, a client error and a server error. A proxy
/// integration maps nothing, so each must reach the front api's caller as the backend
/// computed it.
const BACKEND_ANSWERS: &[(&str, u16)] = &[
    ("whoami", 200),
    ("created", 201),
    ("missing", 404),
    ("broken", 500),
];

/// The body every backend method answers, as its response template writes it.
fn served_body() -> String {
    format!("{{\"m\":\"{NONCE}\"}}")
}

/// An api's own invoke host, which names the api.
fn api_host(api: &str) -> String {
    format!("{api}.execute-api.{REGION}.amazonaws.com")
}

/// The endpoint-specific DNS name EC2 reports in an interface endpoint's `DnsEntries`: it
/// names the ENDPOINT rather than an api, so the api travels in the `x-apigw-api-id` header
/// AWS documents for this addressing.
fn endpoint_host(endpoint: &str) -> String {
    format!("{endpoint}.execute-api.{REGION}.vpce.amazonaws.com")
}

/// The method ARN of the deployed backend `GET` on `path_part`, which is what an
/// authorization refusal of that method reports.
fn method_arn(api: &str, path_part: &str) -> String {
    format!("arn:aws:execute-api:{REGION}:{ACCOUNT}:{api}/{PRIVATE_STAGE}/GET/{path_part}")
}

/// The PRIVATE backend api, carrying one deployed `MOCK` `GET` per row of
/// [`BACKEND_ANSWERS`]; answers its id.
async fn backend(gateway: &Gateway) -> String {
    let created = gateway
        .call(
            "POST",
            "/restapis",
            Some(json!({
                "name": "private-proxy-backend",
                "endpointConfiguration": { "types": ["PRIVATE"] },
            })),
        )
        .await;
    assert_eq!(
        created.status, 201,
        "case CreateRestApi(PRIVATE): expected 201, got {} with {:?}",
        created.status, created.body
    );
    let api = string_member(&created.body, "id", "CreateRestApi");
    let root = string_member(&created.body, "rootResourceId", "CreateRestApi");
    for (path_part, status) in BACKEND_ANSWERS {
        // The integration response carries no `selectionPattern`, so it is the branch every
        // result of the `MOCK` maps to and its own status is what the backend answers.
        mock_resource(
            gateway,
            &api,
            &root,
            path_part,
            "GET",
            &status.to_string(),
            json_template(&served_body()),
        )
        .await;
    }
    deploy(gateway, &api, PRIVATE_STAGE).await;
    api
}

/// `CreateVpcLink` over [`LOAD_BALANCER_ARN`], as its id.
async fn vpc_link(gateway: &Gateway, name: &str) -> String {
    let answer = gateway
        .call(
            "POST",
            "/vpclinks",
            Some(json!({ "name": name, "targetArns": [LOAD_BALANCER_ARN] })),
        )
        .await;
    assert_eq!(
        answer.status, 202,
        "case CreateVpcLink({name:?}): expected 202, got {} with {:?}",
        answer.status, answer.body
    );
    string_member(&answer.body, "id", "CreateVpcLink")
}

/// Read `GetVpcLink` until the link reports [`VPC_LINK_AVAILABLE`], which a private
/// integration is carried only over.
///
/// Nothing is nudged between reads: the status is polled for, never waited out, so a link
/// that only ever settled because somebody called again would never satisfy this either.
async fn await_available(gateway: &Gateway, link: &str) {
    let deadline = Instant::now() + LINK_DEADLINE;
    loop {
        let answer = gateway.call("GET", &format!("/vpclinks/{link}"), None).await;
        assert_eq!(
            answer.status, 200,
            "case GetVpcLink({link}): expected 200, got {} with {:?}",
            answer.status, answer.body
        );
        let status = string_member(&answer.body, "status", "GetVpcLink");
        if status == VPC_LINK_AVAILABLE {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "case GetVpcLink({link}): expected the clock to settle the link to \
             {VPC_LINK_AVAILABLE}, last read {status:?}"
        );
        tokio::time::sleep(LINK_POLL_INTERVAL).await;
    }
}

/// A front api whose root `GET` is an `HTTP_PROXY` integration over `link` to the backend's
/// `GET /{stage}/{path_part}`, deployed to [`FRONT_STAGE`]; answers its id.
///
/// The `uri` names the interface ENDPOINT's own DNS name, which names no api, so the
/// backend's api id is mapped onto the integration request in the header AWS documents for
/// that addressing. The method authorizes nobody, so the call under test is the anonymous
/// one the backend's resource policy is the only grant for.
async fn front(gateway: &Gateway, backend: &str, path_part: &str, link: &str) -> String {
    let (api, root) = create_api(gateway, &format!("private-proxy-front-{path_part}")).await;
    put_method(
        gateway,
        &api,
        &root,
        "GET",
        json!({ "authorizationType": "NONE" }),
    )
    .await;
    put_integration(
        gateway,
        &api,
        &root,
        "GET",
        json!({
            "type": "HTTP_PROXY",
            "httpMethod": "GET",
            "uri": format!(
                "https://{host}/{PRIVATE_STAGE}/{path_part}",
                host = endpoint_host(ENDPOINT)
            ),
            "connectionType": "VPC_LINK",
            "connectionId": link,
            "requestParameters": {
                "integration.request.header.x-apigw-api-id": format!("'{backend}'"),
            },
        }),
    )
    .await;
    deploy(gateway, &api, FRONT_STAGE).await;
    api
}

/// The document that admits `execute-api:Invoke` only from `endpoint`.
fn pinned_policy(endpoint: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": "*",
            "Action": "execute-api:Invoke",
            "Resource": "execute-api:/*",
            "Condition": { "StringEquals": { "aws:SourceVpce": endpoint } },
        }],
    })
    .to_string()
}

/// `UpdateRestApi` re-pinning the backend's `/policy` to `endpoint`, then `CreateDeployment`
/// so the stage serves the api as it now stands — the two calls a developer makes to change
/// what the endpoint admits. The document is read back through `GetRestApi`, so each case
/// proves the round trip it then invokes against.
async fn repin_policy(gateway: &Gateway, api: &str, case: &str, endpoint: &str) {
    let document = pinned_policy(endpoint);
    let patched = gateway
        .call(
            "PATCH",
            &format!("/restapis/{api}"),
            Some(json!({
                "patchOperations": [
                    { "op": "replace", "path": "/policy", "value": document }
                ]
            })),
        )
        .await;
    assert_eq!(
        patched.status, 200,
        "case {case:?}: expected UpdateRestApi to answer 200, got {} with {:?}",
        patched.status, patched.body
    );
    let read = gateway.call("GET", &format!("/restapis/{api}"), None).await;
    assert_eq!(
        read.status, 200,
        "case {case:?}: expected GetRestApi to answer 200, got {} with {:?}",
        read.status, read.body
    );
    let reported = string_member(&read.body, "policy", case);
    assert_eq!(
        reported, document,
        "case {case:?}: expected GetRestApi to report the policy that was set, got \
         {reported:?}"
    );
    deploy(gateway, api, PRIVATE_STAGE).await;
}

/// The `Message` member of a refusal body — the member API Gateway answers an authorization
/// refusal with.
fn authorization_message(body: &str, case: &str) -> String {
    let parsed: Value = serde_json::from_str(body)
        .unwrap_or_else(|_| panic!("case {case:?}: expected a JSON body, got {body:?}"));
    parsed["Message"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a Message member, got {body:?}"))
        .to_owned()
}

#[tokio::test]
async fn a_private_proxy_serves_the_backends_own_answer_behind_its_endpoint_gate() {
    let gateway = Gateway::new();
    let backend = backend(&gateway).await;
    let link = vpc_link(&gateway, "private-proxy-link").await;
    await_available(&gateway, &link).await;

    // One front api per backend method, each proxying over the SAME link to its own backend
    // resource — so the only thing a row varies is the answer the backend computes.
    let mut fronts: Vec<(&str, u16, String)> = Vec::new();
    for (path_part, status) in BACKEND_ANSWERS {
        fronts.push((
            path_part,
            *status,
            front(&gateway, &backend, path_part, &link).await,
        ));
    }

    // (the pinning, the endpoint the backend's policy admits, whether the call is served)
    let pinnings = [
        (
            "pinned to the endpoint the request arrives through",
            ENDPOINT,
            true,
        ),
        (
            "re-pinned to another endpoint of the account",
            OTHER_ENDPOINT,
            false,
        ),
        ("re-pinned to the arrival endpoint again", ENDPOINT, true),
    ];
    for (pinning, pinned, admitted) in pinnings {
        repin_policy(&gateway, &backend, pinning, pinned).await;
        for (path_part, status, front) in &fronts {
            let case = format!("{pinning}, the backend answering {status} on /{path_part}");
            let answered = invoke_on_host(
                &gateway,
                &api_host(front),
                "GET",
                &format!("/{FRONT_STAGE}/"),
                &[],
            )
            .await;
            if admitted {
                assert_eq!(
                    answered.status, *status,
                    "case {case:?}: expected the backend's own {status}, got {} with {:?}",
                    answered.status, answered.body
                );
                assert_eq!(
                    answered.body,
                    served_body(),
                    "case {case:?}: expected the backend's own body, got {:?}",
                    answered.body
                );
            } else {
                assert_eq!(
                    answered.status, 403,
                    "case {case:?}: expected 403, got {} with {:?}",
                    answered.status, answered.body
                );
                let message = authorization_message(&answered.body, &case);
                let expected = format!(
                    "User: {ANONYMOUS} is not authorized to perform: execute-api:Invoke on \
                     resource: {arn}",
                    arn = method_arn(&backend, path_part)
                );
                assert_eq!(
                    message, expected,
                    "case {case:?}: expected the execute-api:Invoke authorization sentence \
                     about the backend method, got {message:?}"
                );
                assert!(
                    !answered.body.contains(NONCE),
                    "case {case:?}: a refused request must carry nothing of the backend's \
                     body, got {:?}",
                    answered.body
                );
            }
        }
    }
}
