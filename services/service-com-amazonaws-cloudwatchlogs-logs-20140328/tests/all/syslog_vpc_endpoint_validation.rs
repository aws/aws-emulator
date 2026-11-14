// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the resource a syslog configuration's `vpcEndpointId` names: the VPC
//! endpoint the log group ingests syslog data through.
//!
//! Every case drives the real wire path — `POST /` with
//! `X-Amz-Target: Logs_20140328.<Operation>` through the multi-service router — with Amazon
//! EC2 mounted beside CloudWatch Logs on the SAME engine, so every endpoint a case names is
//! created and removed through EC2's own API rather than written into a store by hand.
//!
//! The RULES under test, one level above any single recorded call:
//!
//! > 1. A member that identifies a resource owned by ANOTHER service is resolved against that
//! >    service before a put stores anything. `PutSyslogConfiguration` "enables ingestion of
//! >    syslog data through the specified VPC endpoint into the log group", and a VPC endpoint
//! >    is Amazon EC2's resource, so a `vpcEndpointId` naming an endpoint the account has is
//! >    stored and listed, and one naming no endpoint of the account is the operation's
//! >    `InvalidParameterException` (400) whose message is the fixed failed-validation
//! >    sentence around that id — with nothing stored, so the read path reports no such
//! >    configuration.
//! > 2. `DeleteSyslogConfiguration` takes away one ingestion path, so a request naming a pair
//! >    that ingests nothing is refused rather than silently accepted: 400
//! >    `InvalidParameterException` naming the endpoint and the log group it is not associated
//! >    with.
//! > 3. The presence of a required parameter is judged BEFORE the resource an identifier names
//! >    is resolved, so a request that omits `vpcEndpointId` is refused for that member
//! >    whatever its `logGroupIdentifier` names.
//!
//! The rows span what those rules generalise over: which endpoints the account holds (each
//! created through EC2, each binding its own configuration), the WIDTHS the `vpcEndpointId`
//! pattern admits at both published bounds for an id the account has no endpoint under, an id
//! one digit away from one it does hold, the id of an endpoint that was created and then
//! REMOVED (which is what shows the identifier is resolved live rather than remembered), the
//! identifier FORMS a log group is named by, whether the endpoint a delete names is held by
//! EC2, configured on another group, never configured or already deleted, and both operations
//! that take the member for the ordering rule.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::delivery_identity_authorization::xml_element;
use crate::harness::error_code;

/// The Region every request in this suite is signed for, so the CloudWatch Logs call and the
/// EC2 call that stood the endpoint up address the same Region.
const REGION: &str = "eu-west-2";

/// The code CloudWatch Logs answers for a parameter it will not accept.
const INVALID_PARAMETER: &str = "InvalidParameterException";

/// The code answered when the resource an operation names is not there.
const RESOURCE_NOT_FOUND: &str = "ResourceNotFoundException";

/// The Amazon EC2 API version an `ec2Query` body carries.
const EC2_API_VERSION: &str = "2016-11-15";

/// The published bounds on the hexadecimal part of a `vpcEndpointId`. Source: the CloudWatch
/// Logs model's `VpcEndpointId` shape, `@pattern ^vpce-[0-9a-f]{1,64}$`.
const VPC_ENDPOINT_ID_HEX_MIN: usize = 1;
const VPC_ENDPOINT_ID_HEX_MAX: usize = 64;

/// The refusal AWS answers a `vpcEndpointId` that does not resolve to a VPC endpoint of the
/// account with: fixed text around the one variable part, the id the request sent.
fn failed_validation(vpc_endpoint_id: &str) -> String {
    format!(
        "VPC endpoint {vpc_endpoint_id} failed validation. Verify the VPC endpoint ID is \
         correct, belongs to your account, and you have the required EC2 permissions."
    )
}

/// The refusal AWS answers a delete whose pair ingests nothing with: fixed text around the
/// endpoint the request named and the name of the log group it is not associated with.
fn not_associated(vpc_endpoint_id: &str, log_group_name: &str) -> String {
    format!("VPC endpoint {vpc_endpoint_id} is not associated with log group {log_group_name}")
}

/// The refusal a request that names no VPC endpoint carries.
const VPC_ENDPOINT_ID_REQUIRED: &str = "vpcEndpointId is required";

/// A `vpcEndpointId` whose hexadecimal part is `hex_len` characters long, so a row can vary
/// the id's WIDTH along the pattern that bounds it. The characters cycle through the whole
/// `[0-9a-f]` class the pattern admits.
fn endpoint_id(hex_len: usize) -> String {
    let digits: String = "0123456789abcdef".chars().cycle().take(hex_len).collect();
    format!("vpce-{digits}")
}

/// The body `PutSyslogConfiguration` and `DeleteSyslogConfiguration` take. The member is
/// optional in the model on both, so `None` sends no such member at all.
fn syslog_body(identifier: &str, vpc_endpoint_id: Option<&str>) -> Value {
    let mut body = json!({"logGroupIdentifier": identifier});
    if let Some(id) = vpc_endpoint_id {
        body["vpcEndpointId"] = json!(id);
    }
    body
}

/// CloudWatch Logs and Amazon EC2 on one shared engine, reached over the wire.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Signing is not the subject here, so requests are unauthenticated; the credential
        // scope is still carried, because it is what names the Region.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_cloudwatchlogs_logs_20140328::EmulatedService::new(Arc::clone(
                &engine,
            )),
        );
        router.register_service(service_com_amazonaws_ec2_amazonec2::EmulatedService::new(
            engine,
        ));
        Self { router }
    }

    /// One awsJson1_1 CloudWatch Logs call: its status and parsed body.
    async fn logs(&self, operation: &str, body: &Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("Logs_20140328.{operation}"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/logs/\
                     aws4_request, SignedHeaders=host;x-amz-target, Signature=0123456789abcdef"
                ),
            )
            .body(bytes::Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("a valid request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// One CloudWatch Logs call that must succeed, answering its body.
    async fn logs_ok(&self, case: &str, operation: &str, body: &Value) -> Value {
        let (status, parsed) = self.logs(operation, body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: {operation} {body} must succeed, got {status} {parsed}"
        );
        parsed
    }

    /// One CloudWatch Logs call that must be refused as a client error: the status, the wire
    /// error code and the message it reports.
    async fn logs_refusal(
        &self,
        case: &str,
        operation: &str,
        body: &Value,
    ) -> (u16, String, String) {
        let (status, parsed) = self.logs(operation, body).await;
        let message = parsed
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let code = if status == 200 {
            String::new()
        } else {
            error_code(&parsed)
        };
        assert_eq!(
            status, 400,
            "case {case:?}: {operation} {body} must be refused with 400, got {status} {parsed}"
        );
        (status, code, message)
    }

    /// One `ec2Query` call to the Amazon EC2 service mounted beside CloudWatch Logs,
    /// answering the XML body it answered with.
    ///
    /// EC2 speaks `ec2Query`: an `application/x-www-form-urlencoded` POST carrying
    /// `Action=<Operation>`, answered with XML. The request goes through the SAME router the
    /// CloudWatch Logs calls go through, so both services read one engine.
    async fn ec2(&self, action: &str, params: &[(&str, &str)], case: &str) -> String {
        let mut body = format!("Action={action}&Version={EC2_API_VERSION}");
        for (name, value) in params {
            body.push('&');
            body.push_str(name);
            body.push('=');
            body.push_str(value);
        }
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("https://ec2.{REGION}.amazonaws.com/"))
            .header("content-type", "application/x-www-form-urlencoded")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/ec2/\
                     aws4_request, SignedHeaders=host, Signature=0123456789abcdef"
                ),
            )
            .body(bytes::Bytes::from(body))
            .expect("a valid request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let payload = String::from_utf8_lossy(response.body()).to_string();
        assert_eq!(
            status, 200,
            "case {case:?}: EC2 {action} must answer 200, got {status} with {payload}"
        );
        payload
    }

    /// A created log group.
    async fn create_group(&self, name: &str) {
        self.logs_ok(name, "CreateLogGroup", &json!({"logGroupName": name}))
            .await;
    }

    /// The `logGroupArn` CloudWatch Logs stores for the group called `name`.
    async fn group_arn(&self, name: &str) -> String {
        let page = self
            .logs_ok(
                name,
                "DescribeLogGroups",
                &json!({"logGroupIdentifiers": [name]}),
            )
            .await;
        page["logGroups"][0]["logGroupArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {name:?}: a described group carries an ARN, got {page}"))
            .to_string()
    }

    /// The `vpcEndpointId` of every syslog configuration the group `identifier` names carries,
    /// in page order — read back through the implemented reader.
    async fn configured_endpoints(&self, case: &str, identifier: &str) -> Vec<String> {
        let page = self
            .logs_ok(
                case,
                "ListSyslogConfigurations",
                &json!({"logGroupIdentifier": identifier}),
            )
            .await;
        page["syslogConfigurations"]
            .as_array()
            .unwrap_or_else(|| {
                panic!("case {case:?}: a page carries syslogConfigurations, got {page}")
            })
            .iter()
            .map(|item| {
                item["vpcEndpointId"]
                    .as_str()
                    .unwrap_or_else(|| {
                        panic!("case {case:?}: a configuration carries a vpcEndpointId, got {item}")
                    })
                    .to_string()
            })
            .collect()
    }
}

/// The text of the first `<name>` element of an `ec2Query` response — the minted id a create
/// answers with. The reading itself is this target's one XML-element scraper
/// ([`xml_element`]); the case name is what makes its absence attributable to one row.
fn ec2_element(xml: &str, name: &str, case: &str) -> String {
    xml_element(xml, name)
        .unwrap_or_else(|| panic!("case {case:?}: no <{name}> element in {xml}"))
}

/// The network an interface endpoint is created in: a VPC and one subnet of it.
struct Network {
    vpc: String,
    subnet: String,
}

/// One VPC with one subnet, created through EC2's own API.
async fn network(emulator: &Emulator) -> Network {
    let case = "the network the endpoints live in";
    let vpc = ec2_element(
        &emulator
            .ec2("CreateVpc", &[("CidrBlock", "10.0.0.0/16")], case)
            .await,
        "vpcId",
        case,
    );
    let subnet = ec2_element(
        &emulator
            .ec2(
                "CreateSubnet",
                &[
                    ("VpcId", &vpc),
                    ("CidrBlock", "10.0.0.0/24"),
                    ("AvailabilityZone", &format!("{REGION}a")),
                ],
                case,
            )
            .await,
        "subnetId",
        case,
    );
    Network { vpc, subnet }
}

/// One INTERFACE endpoint to the Region's CloudWatch Logs service — the endpoint syslog data
/// is ingested through — answering the id EC2 minted for it.
async fn logs_endpoint(emulator: &Emulator, network: &Network, case: &str) -> String {
    let answered = emulator
        .ec2(
            "CreateVpcEndpoint",
            &[
                ("VpcId", &network.vpc),
                ("ServiceName", &format!("com.amazonaws.{REGION}.logs")),
                ("VpcEndpointType", "Interface"),
                ("SubnetId.1", &network.subnet),
                // A VPC you create has DNS hostnames off, which a private hosted zone needs;
                // this endpoint is only the one syslog data is ingested through, so it asks
                // for no private DNS.
                ("PrivateDnsEnabled", "false"),
            ],
            case,
        )
        .await;
    let id = ec2_element(&answered, "vpcEndpointId", case);
    let hex = id.strip_prefix("vpce-").unwrap_or_else(|| {
        panic!("case {case:?}: EC2 mints an id under the vpce- prefix, got {id}")
    });
    assert!(
        (VPC_ENDPOINT_ID_HEX_MIN..=VPC_ENDPOINT_ID_HEX_MAX).contains(&hex.len())
            && hex.chars().all(|character| character.is_ascii_hexdigit()),
        "case {case:?}: the minted id must satisfy the VpcEndpointId pattern, got {id}"
    );
    id
}

#[tokio::test]
async fn a_put_is_accepted_only_for_a_vpc_endpoint_the_account_has() {
    let emulator = Emulator::new();
    let network = network(&emulator).await;
    let group = "/syslog/endpoint-resolution";
    emulator.create_group(group).await;

    // The positive control, twice over: each endpoint the account HAS is admitted, and each
    // is its own configuration on the group.
    let held: Vec<String> = vec![
        logs_endpoint(&emulator, &network, "the first endpoint of the account").await,
        logs_endpoint(&emulator, &network, "the second endpoint of the account").await,
    ];
    for (index, id) in held.iter().enumerate() {
        let case = format!("an endpoint the account has (number {})", index + 1);
        emulator
            .logs_ok(&case, "PutSyslogConfiguration", &syslog_body(group, Some(id)))
            .await;
    }
    let mut admitted = held.clone();
    admitted.sort();
    let configured = emulator
        .configured_endpoints("after the admitted puts", group)
        .await;
    assert_eq!(
        configured, admitted,
        "case \"after the admitted puts\": expected exactly the endpoints the puts named"
    );

    // An endpoint the account HAD and no longer has: the identifier is resolved against what
    // EC2 holds NOW, so a put naming it is refused once the endpoint is gone.
    let removed = logs_endpoint(&emulator, &network, "an endpoint that is then removed").await;
    emulator
        .ec2(
            "DeleteVpcEndpoints",
            &[("VpcEndpointId.1", &removed)],
            "an endpoint that is then removed",
        )
        .await;

    // Rows differ along WHY the id names no endpoint of the account: each published width of
    // the pattern, an id one digit away from one the account does hold, and an endpoint that
    // was created and then removed.
    let one_digit_off = {
        let first = &held[0];
        let (body, last) = first.split_at(first.len() - 1);
        let shifted = if last == "0" { "1" } else { "0" };
        format!("{body}{shifted}")
    };
    let refused: Vec<(String, String)> = vec![
        (
            "an id at the pattern's minimum width the account has no endpoint under".to_string(),
            endpoint_id(VPC_ENDPOINT_ID_HEX_MIN),
        ),
        (
            "an id at the pattern's maximum width the account has no endpoint under".to_string(),
            endpoint_id(VPC_ENDPOINT_ID_HEX_MAX),
        ),
        (
            "a 17-digit id the account has no endpoint under".to_string(),
            endpoint_id(17),
        ),
        (
            "an all-zero id the account has no endpoint under".to_string(),
            "vpce-0000000000000000".to_string(),
        ),
        (
            "an id one digit away from one the account does hold".to_string(),
            one_digit_off,
        ),
        (
            "the id of an endpoint that was created and then removed".to_string(),
            removed.clone(),
        ),
    ];
    for (case, id) in &refused {
        let (status, code, message) = emulator
            .logs_refusal(case, "PutSyslogConfiguration", &syslog_body(group, Some(id)))
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, INVALID_PARAMETER),
            "case {case:?}: expected 400 {INVALID_PARAMETER}, got {status} {code:?}"
        );
        assert_eq!(
            message,
            failed_validation(id),
            "case {case:?}: expected the failed-validation sentence naming the id sent, got \
             {message:?}"
        );
    }

    // NOTHING a refusal named was stored: the group still carries exactly the admitted
    // configurations, so the read path agrees with the refusals.
    let configured = emulator
        .configured_endpoints("after every refusal", group)
        .await;
    assert_eq!(
        configured, admitted,
        "case \"after every refusal\": a refused put stores nothing, got {configured:?}"
    );
    for (case, id) in &refused {
        assert!(
            !configured.contains(id),
            "case {case:?}: expected no configuration stored for it, got {configured:?}"
        );
    }

    // And the refusal reserved nothing: the same group takes the endpoint that does exist.
    let retry = "a put retried with an endpoint that exists";
    let retried = logs_endpoint(&emulator, &network, retry).await;
    emulator
        .logs_ok(
            retry,
            "PutSyslogConfiguration",
            &syslog_body(group, Some(&retried)),
        )
        .await;
    let configured = emulator
        .configured_endpoints("after the retry", group)
        .await;
    assert!(
        configured.contains(&retried),
        "case \"after the retry\": expected {retried:?} among {configured:?}"
    );
}

#[tokio::test]
async fn a_delete_naming_a_pair_that_ingests_nothing_is_refused() {
    let emulator = Emulator::new();
    let network = network(&emulator).await;
    let alpha = "/syslog/alpha";
    let beta = "/syslog/beta";
    emulator.create_group(alpha).await;
    emulator.create_group(beta).await;
    let alpha_arn = emulator.group_arn(alpha).await;

    let configured = logs_endpoint(&emulator, &network, "the endpoint configured on alpha").await;
    let unconfigured = logs_endpoint(&emulator, &network, "an endpoint configured nowhere").await;
    emulator
        .logs_ok(
            "the configuration under test",
            "PutSyslogConfiguration",
            &syslog_body(alpha, Some(&configured)),
        )
        .await;

    // (case, the log group identifier the delete names, the endpoint it names, the group name
    // the refusal must report). The rows differ along whether the endpoint is one EC2 holds,
    // whether it is configured on ANOTHER group or on none, and the form the group is named by.
    let refused: Vec<(&str, &str, &str, &str)> = vec![
        (
            "an endpoint configured on another group",
            beta,
            configured.as_str(),
            beta,
        ),
        (
            "an endpoint of the account that is configured nowhere",
            alpha,
            unconfigured.as_str(),
            alpha,
        ),
        (
            "the group named by its ARN rather than its name",
            alpha_arn.as_str(),
            unconfigured.as_str(),
            alpha,
        ),
        (
            "an id the account has no endpoint under at all",
            alpha,
            "vpce-0000000000000000",
            alpha,
        ),
        (
            "an id at the pattern's minimum width",
            beta,
            "vpce-0",
            beta,
        ),
    ];
    for (case, identifier, id, group_name) in &refused {
        let (status, code, message) = emulator
            .logs_refusal(
                case,
                "DeleteSyslogConfiguration",
                &syslog_body(identifier, Some(id)),
            )
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, INVALID_PARAMETER),
            "case {case:?}: expected 400 {INVALID_PARAMETER}, got {status} {code:?}"
        );
        assert_eq!(
            message,
            not_associated(id, group_name),
            "case {case:?}: expected the not-associated sentence naming the endpoint and the \
             group, got {message:?}"
        );
    }

    // The one configuration that DOES ingest is deleted once — and only once: deleting it
    // again names a pair that ingests nothing, which is the same refusal.
    emulator
        .logs_ok(
            "the configuration that does ingest",
            "DeleteSyslogConfiguration",
            &syslog_body(alpha, Some(&configured)),
        )
        .await;
    assert!(
        emulator
            .configured_endpoints("after the delete", alpha)
            .await
            .is_empty(),
        "case \"after the delete\": expected the group to carry no configuration"
    );
    let (status, code, message) = emulator
        .logs_refusal(
            "a second delete of the configuration already gone",
            "DeleteSyslogConfiguration",
            &syslog_body(alpha, Some(&configured)),
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (400, INVALID_PARAMETER),
        "case \"a second delete of the configuration already gone\": expected 400 \
         {INVALID_PARAMETER}, got {status} {code:?}"
    );
    assert_eq!(
        message,
        not_associated(&configured, alpha),
        "case \"a second delete of the configuration already gone\": expected the \
         not-associated sentence, got {message:?}"
    );
}

#[tokio::test]
async fn a_missing_vpc_endpoint_id_is_refused_before_the_log_group_is_resolved() {
    let emulator = Emulator::new();
    let network = network(&emulator).await;
    let existing = "/syslog/ordering";
    emulator.create_group(existing).await;
    let existing_arn = emulator.group_arn(existing).await;
    let held = logs_endpoint(&emulator, &network, "an endpoint the account has").await;

    // (case, the logGroupIdentifier the request names) — the rows differ along whether the
    // identifier resolves at all and the form it takes, because the member's absence is
    // judged before any of that.
    let identifiers: Vec<(&str, String)> = vec![
        ("a group that exists", existing.to_string()),
        ("a group that exists, named by its ARN", existing_arn.clone()),
        (
            "a name no group was created under",
            "/syslog/never-created".to_string(),
        ),
        (
            "an ARN no group was created under",
            existing_arn.replace("ordering", "never-created"),
        ),
    ];
    for (case, identifier) in &identifiers {
        for operation in ["PutSyslogConfiguration", "DeleteSyslogConfiguration"] {
            let case = format!("{case} on {operation}");
            let (status, code, message) = emulator
                .logs_refusal(&case, operation, &syslog_body(identifier, None))
                .await;
            assert_eq!(
                (status, code.as_str()),
                (400, INVALID_PARAMETER),
                "case {case:?}: expected 400 {INVALID_PARAMETER}, got {status} {code:?}"
            );
            assert_eq!(
                message, VPC_ENDPOINT_ID_REQUIRED,
                "case {case:?}: expected {VPC_ENDPOINT_ID_REQUIRED:?}, got {message:?}"
            );
        }
    }

    // The other side of the ordering: with the member present, an identifier that names no
    // log group is the missing RESOURCE — so the refusals above are about the parameter and
    // not about the group.
    for operation in ["PutSyslogConfiguration", "DeleteSyslogConfiguration"] {
        let case =
            format!("a group that does not exist, with an endpoint that does, on {operation}");
        let (_, code, _) = emulator
            .logs_refusal(
                &case,
                operation,
                &syslog_body("/syslog/never-created", Some(&held)),
            )
            .await;
        assert_eq!(
            code, RESOURCE_NOT_FOUND,
            "case {case:?}: expected {RESOURCE_NOT_FOUND}, got {code:?}"
        );
    }
}
