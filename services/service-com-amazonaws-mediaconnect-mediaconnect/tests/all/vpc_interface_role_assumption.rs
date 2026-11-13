// SPDX-License-Identifier: CC0-1.0

//! The role a flow's VPC interface is created UNDER is resolved against IAM — the service
//! that owns it — before any state is written.
//!
//! THE RULE UNDER TEST. `VpcInterfaceRequest$RoleArn` is "the ARN of the role that you
//! created when you set up MediaConnect as a trusted service" and the stored
//! `VpcInterface$RoleArn` is "a role Arn MediaConnect can assume to create ENIs in your
//! account" (`smithy_query(query="shape",shape="VpcInterfaceRequest,VpcInterface")`), so a
//! request that adds a VPC interface to a flow asks this service to ACT UNDER that role: it
//! takes the role on first, and a role IAM answers it cannot assume — this account holds no
//! such role, or the trust policy the role carries does not admit
//! `mediaconnect.amazonaws.com` — refuses the WHOLE request with the modelled
//! `BadRequestException` (HTTP 400, error type `BadRequestException`) whose message echoes
//! the role ARN the request carried, and stores NOTHING.
//!
//! The other half of the rule: a refusal is GROUNDED in an answer. A value that names
//! another account's role is not this emulator's IAM to answer for, so nothing is
//! established about it and the request is served — the same reading every emulated service
//! takes of a caller-supplied role.
//!
//! IAM is mounted beside MediaConnect on ONE engine, and every role an accepted row names
//! exists only because a real `CreateRole` created it here, so the resolution under test runs
//! over the same in-process callback as in production and every refusal is IAM's own answer.
//! This suite builds its own router rather than using `crate::flow_lifecycle::Emulator`,
//! which mounts this service alone.
//!
//! The rows differ along what the rule generalises over: whether IAM holds the role at all,
//! what its trust policy says (this service, another service, an explicit deny of this
//! service), whether the role sits under a path, whose account it belongs to, WHICH entry of
//! a batch carries it, and which of the two operations that add VPC interfaces to a flow
//! carries the entry.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The Region every request below is signed for, and so the Region every ARN is minted in.
const REGION: &str = "us-east-1";

/// The service principal a role's trust policy has to admit for MediaConnect to create
/// network interfaces under it, as AWS names it — written out here rather than read from the
/// crate so a change to the handler cannot quietly move what these tests assert.
const SERVICE_PRINCIPAL: &str = "mediaconnect.amazonaws.com";

/// The action a trust policy grants a service principal so it can take a role on.
const ASSUME_ROLE: &str = "sts:AssumeRole";

/// AWS's whole `BadRequestException$Message` for a request naming a role MediaConnect cannot
/// assume: fixed text around the role ARN the REQUEST carried.
fn unassumable_refusal(role_arn: &str) -> String {
    format!(
        "Bad request: Unable to assume role: {role_arn} Add {SERVICE_PRINCIPAL} as a trusted \
         entity and try again."
    )
}

/// MediaConnect mounted beside the OWNER of the roles its VPC interfaces are created under,
/// on one shared engine, behind the production router.
struct Wired {
    router: MultiServiceRouter,
}

impl Wired {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Signatures are not the subject here; the requests below carry a scope, not a real
        // signature.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_mediaconnect_mediaconnect::EmulatedService::new(Arc::clone(
                &engine,
            )),
        );
        router.register_service(
            service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(engine),
        );
        Self { router }
    }

    /// One MediaConnect request through the front door, answered as `(status, error type,
    /// body)`.
    async fn send(&self, method: &str, uri: &str, body: Option<Value>) -> (u16, Option<String>, Value) {
        let payload = body
            .map(|value| bytes::Bytes::from(serde_json::to_vec(&value).expect("serialize body")))
            .unwrap_or_default();
        let request = http::Request::builder()
            .method(method)
            .uri(uri)
            .header("content-type", "application/json")
            .header("host", format!("mediaconnect.{REGION}.amazonaws.com"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 \
                     Credential=AKIDEXAMPLEEXAMPLE/20260101/{REGION}/mediaconnect/aws4_request, \
                     SignedHeaders=host, Signature=deadbeef"
                ),
            )
            .body(payload)
            .expect("build request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let code = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(ToString::to_string);
        let parsed = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, code, parsed)
    }

    /// Create an IAM role carrying `document` as its trust policy under `path`, and answer
    /// the ARN IAM minted for it — read off IAM's own response rather than constructed, so a
    /// row names the ARN IAM actually holds.
    ///
    /// IAM's protocol is `awsQuery`, so the body is form-encoded; `form_urlencoded` is the
    /// one encoder used for it rather than a hand-rolled one.
    async fn create_role(&self, name: &str, path: &str, document: &str) -> String {
        let body = ::form_urlencoded::Serializer::new(String::new())
            .append_pair("Action", "CreateRole")
            .append_pair("Version", "2010-05-08")
            .append_pair("RoleName", name)
            .append_pair("Path", path)
            .append_pair("AssumeRolePolicyDocument", document)
            .finish();
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", "iam.amazonaws.com")
            .header("content-type", "application/x-www-form-urlencoded")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 \
                     Credential=AKIDEXAMPLEEXAMPLE/20260101/{REGION}/iam/aws4_request, \
                     SignedHeaders=host, Signature=deadbeef"
                ),
            )
            .body(bytes::Bytes::from(body))
            .expect("build request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let xml = String::from_utf8(response.body().to_vec()).expect("a UTF-8 response body");
        assert_eq!(
            status, 200,
            "CreateRole {name:?} expected 200, got {status}: {xml}"
        );
        let opened = xml
            .find("<Arn>")
            .unwrap_or_else(|| panic!("CreateRole {name:?} returned no <Arn>: {xml}"))
            + "<Arn>".len();
        let closed = xml[opened..]
            .find("</Arn>")
            .unwrap_or_else(|| panic!("CreateRole {name:?} left <Arn> unclosed: {xml}"));
        xml[opened..opened + closed].to_string()
    }

    /// Create a flow named `name` carrying `vpc_interfaces` (omitted when null), answered as
    /// the wire gives it so a row can assert a refused create as well as a good one.
    async fn create_flow(&self, name: &str, vpc_interfaces: Value) -> (u16, Option<String>, Value) {
        let mut body = json!({
            "name": name,
            "source": { "name": "src", "protocol": "rtp", "whitelistCidr": "10.60.0.0/16" },
        });
        if !vpc_interfaces.is_null() {
            body["vpcInterfaces"] = vpc_interfaces;
        }
        self.send("POST", "/v1/flows", Some(body)).await
    }

    /// `POST /v1/flows/{FlowArn}/vpcInterfaces` — the AddFlowVpcInterfaces binding
    /// (`smithy_query(query="operation",operation="AddFlowVpcInterfaces")`).
    async fn add_vpc_interfaces(
        &self,
        flow_arn: &str,
        vpc_interfaces: Value,
    ) -> (u16, Option<String>, Value) {
        self.send(
            "POST",
            &format!("/v1/flows/{flow_arn}/vpcInterfaces"),
            Some(json!({ "vpcInterfaces": vpc_interfaces })),
        )
        .await
    }

    /// The `vpcInterfaces` DescribeFlow reports for `flow_arn`, which is where a stored VPC
    /// interface is read back from.
    async fn described_vpc_interfaces(&self, case: &str, flow_arn: &str) -> Vec<Value> {
        let (status, code, described) = self.send("GET", &format!("/v1/flows/{flow_arn}"), None).await;
        assert_eq!(
            (status, code.as_deref()),
            (200, None),
            "case {case:?}: DescribeFlow must answer 200, got {status} {code:?}: {described}"
        );
        described["flow"]["vpcInterfaces"]
            .as_array()
            .cloned()
            .unwrap_or_default()
    }

    /// The names ListFlows reports, so a row can assert a refused create stored no flow.
    async fn listed_flow_names(&self) -> Vec<String> {
        let (status, code, listed) = self.send("GET", "/v1/flows", None).await;
        assert_eq!(
            (status, code.as_deref()),
            (200, None),
            "ListFlows must answer 200, got {status} {code:?}: {listed}"
        );
        listed["flows"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|flow| flow["name"].as_str().unwrap_or_default().to_string())
            .collect()
    }
}

/// A trust policy with one statement, written as a client would send it.
fn policy(effect: &str, action: &str, service: &str) -> String {
    format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Effect\":\"{effect}\",\
         \"Principal\":{{\"Service\":\"{service}\"}},\"Action\":\"{action}\"}}]}}"
    )
}

/// One VPC interface request, in the shape `VpcInterfaceRequest` takes (`Name`, `RoleArn`,
/// `SecurityGroupIds` and `SubnetId` required).
///
/// The subnet and security group are synthetic: no EC2 is mounted here, so nothing is
/// established about the network and the placement grounds no refusal — which is what leaves
/// the ROLE as the only thing these rows vary.
fn vpc_interface(name: &str, role_arn: &str) -> Value {
    json!({
        "name": name,
        "roleArn": role_arn,
        "securityGroupIds": ["sg-0123456789abcdef0"],
        "subnetId": "subnet-0123456789abcdef0",
    })
}

/// One row: a role a request names, and whether the rule says the request is served.
struct Row {
    case: &'static str,
    role_arn: String,
    served: bool,
}

/// The roles the rows below name, created in IAM where the row is about a role IAM HOLDS.
async fn rows(wired: &Wired) -> Vec<Row> {
    let account = engine::emulator_account_id();
    vec![
        Row {
            case: "a role IAM holds whose trust policy admits this service",
            role_arn: wired
                .create_role(
                    "mediaconnect-eni",
                    "/",
                    &policy("Allow", ASSUME_ROLE, SERVICE_PRINCIPAL),
                )
                .await,
            served: true,
        },
        Row {
            case: "the same grant on a role IAM holds under a path",
            role_arn: wired
                .create_role(
                    "mediaconnect-eni-pathed",
                    "/service-role/",
                    &policy("Allow", ASSUME_ROLE, SERVICE_PRINCIPAL),
                )
                .await,
            served: true,
        },
        Row {
            case: "a role IAM holds whose trust policy admits another service",
            role_arn: wired
                .create_role(
                    "medialive-eni",
                    "/",
                    &policy("Allow", ASSUME_ROLE, "medialive.amazonaws.com"),
                )
                .await,
            served: false,
        },
        Row {
            case: "a role IAM holds whose trust policy denies this service explicitly",
            role_arn: wired
                .create_role(
                    "mediaconnect-eni-denied",
                    "/",
                    &policy("Deny", ASSUME_ROLE, SERVICE_PRINCIPAL),
                )
                .await,
            served: false,
        },
        Row {
            case: "a role ARN of this account IAM holds no role for",
            role_arn: format!("arn:aws:iam::{account}:role/mediaconnect-eni-never-created"),
            served: false,
        },
        Row {
            // Another account's roles are not this emulator's IAM to answer for, so nothing
            // is established about the value and nothing is refused on it.
            case: "a role of another account",
            role_arn: "arn:aws:iam::123456789012:role/mediaconnect-eni".to_string(),
            served: true,
        },
    ]
}

/// AddFlowVpcInterfaces resolves the role every requested entry names: a role this service
/// may act under is stored and read back through DescribeFlow, and a role IAM answered it
/// cannot assume refuses the call with AWS's message and leaves the flow holding nothing.
#[tokio::test]
async fn add_flow_vpc_interfaces_resolves_the_role_each_entry_names() {
    let wired = Wired::new();
    let rows = rows(&wired).await;

    for (index, Row { case, role_arn, served }) in rows.iter().enumerate() {
        let (status, code, created) = wired
            .create_flow(&format!("add-role-flow-{index}"), Value::Null)
            .await;
        assert_eq!(
            (status, code.as_deref()),
            (201, None),
            "case {case:?}: the fixture flow must be created, got {status} {code:?}: {created}"
        );
        let flow_arn = created["flow"]["flowArn"]
            .as_str()
            .expect("a created flow carries its ARN")
            .to_string();

        let requested = json!([vpc_interface("vpc-1", role_arn)]);
        let (status, code, body) = wired.add_vpc_interfaces(&flow_arn, requested).await;
        let stored = wired.described_vpc_interfaces(case, &flow_arn).await;
        if *served {
            assert_eq!(
                (status, code.as_deref()),
                (201, None),
                "case {case:?}: expected 201 with no error, got {status} {code:?}: {body}"
            );
            // Read back through DescribeFlow: the interface is stored, under the role the
            // request named.
            let names: Vec<&str> = stored
                .iter()
                .map(|held| held["name"].as_str().unwrap_or_default())
                .collect();
            assert_eq!(
                names,
                vec!["vpc-1"],
                "case {case:?}: the flow holds the added interface, got {stored:?}"
            );
            assert_eq!(
                stored[0]["roleArn"].as_str(),
                Some(role_arn.as_str()),
                "case {case:?}: the stored interface carries the role the request named, \
                 got {stored:?}"
            );
        } else {
            assert_eq!(
                (status, code.as_deref()),
                (400, Some("BadRequestException")),
                "case {case:?}: expected 400 BadRequestException, got {status} {code:?}: {body}"
            );
            let expected = unassumable_refusal(role_arn);
            assert_eq!(
                body["message"].as_str(),
                Some(expected.as_str()),
                "case {case:?}: expected the refusal {expected:?}, got {body}"
            );
            assert!(
                stored.is_empty(),
                "case {case:?}: a refused add stores no VPC interface, got {stored:?}"
            );
        }
    }
}

/// CreateFlow's own `VpcInterfaces` are resolved the same way, so the two operations that add
/// VPC interfaces to a flow admit and refuse exactly the same requests — and a refused create
/// stores no flow at all.
#[tokio::test]
async fn create_flow_resolves_the_role_each_vpc_interface_names() {
    let wired = Wired::new();
    let rows = rows(&wired).await;

    for (index, Row { case, role_arn, served }) in rows.iter().enumerate() {
        let name = format!("create-role-flow-{index}");
        let requested = json!([vpc_interface("vpc-1", role_arn)]);
        let (status, code, body) = wired.create_flow(&name, requested).await;
        if *served {
            assert_eq!(
                (status, code.as_deref()),
                (201, None),
                "case {case:?}: expected 201 with no error, got {status} {code:?}: {body}"
            );
            let flow_arn = body["flow"]["flowArn"]
                .as_str()
                .expect("a created flow carries its ARN")
                .to_string();
            let stored = wired.described_vpc_interfaces(case, &flow_arn).await;
            assert_eq!(
                stored.len(),
                1,
                "case {case:?}: the created flow holds its VPC interface, got {stored:?}"
            );
            assert_eq!(
                stored[0]["roleArn"].as_str(),
                Some(role_arn.as_str()),
                "case {case:?}: the stored interface carries the role the request named, \
                 got {stored:?}"
            );
        } else {
            assert_eq!(
                (status, code.as_deref()),
                (400, Some("BadRequestException")),
                "case {case:?}: expected 400 BadRequestException, got {status} {code:?}: {body}"
            );
            let expected = unassumable_refusal(role_arn);
            assert_eq!(
                body["message"].as_str(),
                Some(expected.as_str()),
                "case {case:?}: expected the refusal {expected:?}, got {body}"
            );
            let listed = wired.listed_flow_names().await;
            assert!(
                !listed.contains(&name),
                "case {case:?}: a refused create stores no flow, got {listed:?}"
            );
        }
    }
}

/// The WHOLE request is refused: a batch whose second entry names a role IAM answered cannot
/// be assumed adds none of its entries, and the refusal names that second role — then the
/// same flow takes a batch whose entries all name an assumable role.
#[tokio::test]
async fn one_unassumable_entry_refuses_the_whole_batch() {
    let wired = Wired::new();
    let case = "a batch whose second entry cannot be assumed";
    let assumable = wired
        .create_role(
            "mediaconnect-eni-batch",
            "/",
            &policy("Allow", ASSUME_ROLE, SERVICE_PRINCIPAL),
        )
        .await;
    let unassumable = format!(
        "arn:aws:iam::{}:role/mediaconnect-eni-batch-absent",
        engine::emulator_account_id()
    );

    let (status, code, created) = wired.create_flow("batch-role-flow", Value::Null).await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "case {case:?}: the fixture flow must be created, got {status} {code:?}: {created}"
    );
    let flow_arn = created["flow"]["flowArn"]
        .as_str()
        .expect("a created flow carries its ARN")
        .to_string();

    let (status, code, body) = wired
        .add_vpc_interfaces(
            &flow_arn,
            json!([
                vpc_interface("vpc-good", &assumable),
                vpc_interface("vpc-bad", &unassumable),
            ]),
        )
        .await;
    assert_eq!(
        (status, code.as_deref()),
        (400, Some("BadRequestException")),
        "case {case:?}: expected 400 BadRequestException, got {status} {code:?}: {body}"
    );
    let expected = unassumable_refusal(&unassumable);
    assert_eq!(
        body["message"].as_str(),
        Some(expected.as_str()),
        "case {case:?}: the refusal names the entry's own role, expected {expected:?}, got \
         {body}"
    );
    let stored = wired.described_vpc_interfaces(case, &flow_arn).await;
    assert!(
        stored.is_empty(),
        "case {case:?}: one unassumable entry stores none of the batch, got {stored:?}"
    );

    let (status, code, body) = wired
        .add_vpc_interfaces(
            &flow_arn,
            json!([
                vpc_interface("vpc-good", &assumable),
                vpc_interface("vpc-also-good", &assumable),
            ]),
        )
        .await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "case {case:?}: a batch of assumable roles is stored, got {status} {code:?}: {body}"
    );
    let names: Vec<String> = wired
        .described_vpc_interfaces(case, &flow_arn)
        .await
        .iter()
        .map(|held| held["name"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        names,
        vec!["vpc-good".to_string(), "vpc-also-good".to_string()],
        "case {case:?}: both entries of the accepted batch are stored, got {names:?}"
    );
}
