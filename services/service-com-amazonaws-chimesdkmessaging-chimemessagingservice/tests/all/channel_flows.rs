// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Chime SDK Messaging channel-FLOW slice.
//!
//! Every case drives real HTTP through the production router — restJson1 request on the
//! wire in, restJson1 response out — so nothing here can pass by reading a handler's Rust
//! types directly. Four rules are under test:
//!
//! * each operation computes its answer from the SAME persisted channel-flow store: what
//!   `CreateChannelFlow` wrote is what `DescribeChannelFlow` and `ListChannelFlows`
//!   report, and what `DeleteChannelFlow` removed is gone for both;
//! * a channel carries at most ONE channel flow, so `AssociateChannelFlow` REPLACES the
//!   association the channel held and every reader agrees about the one it carries now —
//!   `DescribeChannel`'s `ChannelFlowArn` and the channels
//!   `ListChannelsAssociatedWithChannelFlow` reports for each flow — until
//!   `DisassociateChannelFlow` removes it;
//! * a channel flow a channel is still associated with is not deleted at all; and
//! * a `ChannelFlowArn` that names no channel flow this account holds is answered with
//!   the modelled `ForbiddenException` — a TERMINAL HTTP 403 carrying `Code` `Forbidden`
//!   in the restJson1 error shape, `Content-Type: application/json` and an
//!   `x-amzn-errortype` naming that modelled shape — because `DescribeChannelFlow` models
//!   no `NotFoundException` (`smithy_query(query="operation",
//!   operation="DescribeChannelFlow")`: errors `BadRequestException` plus the service's
//!   common `ForbiddenException`, `ServiceFailureException`,
//!   `ServiceUnavailableException`, `ThrottledClientException`,
//!   `UnauthorizedClientException`).
//!
//! The tables span what those rules generalise over rather than re-running one recorded
//! request: processor counts at both ends of the modelled `ProcessorList len 1..3`, every
//! `FallbackAction` wire value, every `ChannelFlowExecutionOrder` inside its modelled
//! `range 1..3`, tags present and absent, page sizes at and past the modelled `MaxResults`
//! bounds, six shapes of ARN that name no stored flow, every `ChannelMode` and
//! `ChannelPrivacy` the associated channel can carry, one channel flow associated with one
//! through nine channels, and four shapes of unusable `NextToken`.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_chimesdkmessaging_chimemessagingservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

// The restJson1 label/query encoders this binary's other suite already states: one copy
// for the whole test target, so two suites cannot encode an ARN differently.
use super::channel_messaging::{encode, query};
// The `Message` every authorization refusal of this service carries, stated once for this
// test binary.
use super::channel_membership::INSUFFICIENT_ACCESS;

/// The `AppInstanceArn` most cases create their channel flows under.
const APP_INSTANCE: &str = "arn:aws:chime:us-east-1:111122223333:app-instance/ai1";
/// A second app instance, so the `ListChannelFlows` filter has something to exclude.
const OTHER_APP_INSTANCE: &str = "arn:aws:chime:us-east-1:111122223333:app-instance/ai2";
/// The `x-amz-chime-bearer` identity every request is made as.
const BEARER: &str = "arn:aws:chime:us-east-1:111122223333:app-instance/ai1/user/alice";
/// A processor Lambda ARN inside the modelled `LambdaFunctionArn` pattern
/// `^arn:aws:lambda:[a-z]{2}-[a-z]+-\d{1}:\d{12}:function:[a-zA-Z0-9\-_\.]+(:(\$LATEST|[a-zA-Z0-9\-_]+))?$`.
const PROCESSOR_LAMBDA: &str = "arn:aws:lambda:us-east-1:111122223333:function:redact-profanity";

/// The `FallbackAction` wire values, from
/// `smithy_query(query="shape", shape="FallbackAction")`.
const FALLBACK_ACTIONS: [&str; 2] = ["ABORT", "CONTINUE"];
/// The `InvocationType` wire values, from the same model
/// (`smithy_query(query="shape", shape="LambdaConfiguration")` -> `InvocationType`).
const INVOCATION_TYPES: [&str; 1] = ["ASYNC"];
/// `MaxResults`'s modelled bound on `ListChannelFlows` (`range 1..50`).
const MAX_RESULTS_MAX: i64 = 50;
/// A second identity under the same app instance, which moderates none of the channels
/// these cases create: the caller a ROLE refusal is exercised with.
const NON_MODERATOR: &str = "arn:aws:chime:us-east-1:111122223333:app-instance/ai1/user/bob";

/// The `Message` a `DeleteChannelFlow` naming a channel flow a channel is still associated
/// with is refused with, as AWS words that refusal.
const FLOW_STILL_ASSOCIATED: &str = "Channel flow associated with a channel cannot be deleted.";
/// The `Message` a `DisassociateChannelFlow` whose `ChannelFlowArn` is not the one the
/// channel carries is refused with, as AWS words that refusal.
const FLOW_NOT_ASSOCIATED: &str =
    "Channel flow provided in the request is not associated with the channel.";
/// The `Message` a `NextToken` this service cannot read a cursor out of is refused with,
/// naming the operation it was presented to, as AWS words that refusal.
const TOKEN_EXCEPTION: &str = "Pagination token exception in operation \
     'ListChannelsAssociatedWithChannelFlow': decryption error";
/// The `Message` an ARN this service holds no channel FLOW for is refused with.
const UNRESOLVABLE_APP_INSTANCE: &str =
    "An invalid App Instance ARN was supplied for the input parameter.";

/// Every error shape `DescribeChannelFlow` declares, from
/// `smithy_query(query="operation", operation="DescribeChannelFlow")` — `errors`:
/// `BadRequestException` (400) plus this service's common `ForbiddenException` (403),
/// `ServiceFailureException` (500), `ServiceUnavailableException` (503),
/// `ThrottledClientException` (429) and `UnauthorizedClientException` (401). A refusal of
/// this operation names one of these in `x-amzn-errortype`, never a code no model carries.
const DESCRIBE_CHANNEL_FLOW_ERRORS: [&str; 6] = [
    "BadRequestException",
    "ForbiddenException",
    "ServiceFailureException",
    "ServiceUnavailableException",
    "ThrottledClientException",
    "UnauthorizedClientException",
];

/// One response, reduced to the four things the wire contract pins: the HTTP status, the
/// `Content-Type`, the `x-amzn-errortype` a restJson1 client resolves the error shape
/// from, and the JSON body.
#[derive(Debug)]
struct Reply {
    status: u16,
    content_type: String,
    error_type: String,
    body: Value,
}

impl Reply {
    /// The `Message` the error shape carries.
    ///
    /// A modelled error shape of this service names the member `Message`; the framework's
    /// own `ValidationException` names it `message`, so both spellings are read here.
    fn message(&self) -> &str {
        self.body
            .get("Message")
            .or_else(|| self.body.get("message"))
            .and_then(Value::as_str)
            .unwrap_or("<no Message>")
    }

    /// The modelled error code this service puts in the error shape's `Code` member.
    fn code(&self) -> &str {
        self.body
            .get("Code")
            .and_then(Value::as_str)
            .unwrap_or("<no Code>")
    }

    /// The error SHAPE `x-amzn-errortype` names, stripped of the optional `namespace#`
    /// prefix and `:sdk-url` suffix the restJson1 protocol allows around it.
    fn error_shape(&self) -> &str {
        let shape = self
            .error_type
            .split('#')
            .next_back()
            .unwrap_or(&self.error_type);
        shape.split(':').next().unwrap_or(shape)
    }
}

/// The emulator under test: this one service mounted in the production router.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        // ONE shared engine, exactly as `router/src/lib.rs` builds it, so every request
        // in a case observes the same store.
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Signature verification is the emulator's own concern and is exercised
        // elsewhere; these cases are about the service's behaviour.
        router
            .set_request_authentication(false)
            .register_service(EmulatedService::new(engine));
        Self { router }
    }

    async fn call(&self, method: &str, uri: &str, body: Option<&Value>) -> Reply {
        self.call_as(BEARER, method, uri, body).await
    }

    /// [`Self::call`] as an identity other than [`BEARER`].
    ///
    /// The `x-amz-chime-bearer` header is the caller every handler decides an action on,
    /// so a case about the ROLE an action needs has to make the request as someone else.
    async fn call_as(
        &self,
        bearer: &str,
        method: &str,
        uri: &str,
        body: Option<&Value>,
    ) -> Reply {
        let bytes = match body {
            Some(value) => Bytes::from(serde_json::to_vec(value).expect("serialize the request")),
            None => Bytes::new(),
        };
        let request = http::Request::builder()
            .method(method)
            .uri(uri)
            .header("host", "messaging-chime.us-east-1.amazonaws.com")
            // The SigV4 credential scope carries the signing name five sibling services
            // share, so this request only reaches this service because `can_handle` keys
            // on the endpoint and the path instead.
            .header(
                "authorization",
                "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/us-east-1/chime/aws4_request, \
                 SignedHeaders=host, Signature=0000",
            )
            .header("content-type", "application/json")
            .header("x-amz-chime-bearer", bearer)
            .body(bytes)
            .expect("build the request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let header = |name: &str| {
            response
                .headers()
                .get(name)
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default()
                .to_string()
        };
        let content_type = header("content-type");
        let error_type = header("x-amzn-errortype");
        let body = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        Reply {
            status,
            content_type,
            error_type,
            body,
        }
    }

    async fn create_channel_flow(&self, request: &Value) -> Reply {
        self.call("POST", "/channel-flows", Some(request)).await
    }

    async fn describe_channel_flow(&self, channel_flow_arn: &str) -> Reply {
        self.call(
            "GET",
            &format!("/channel-flows/{}", encode(channel_flow_arn)),
            None,
        )
        .await
    }

    async fn delete_channel_flow(&self, channel_flow_arn: &str) -> Reply {
        self.call(
            "DELETE",
            &format!("/channel-flows/{}", encode(channel_flow_arn)),
            None,
        )
        .await
    }

    async fn list_channel_flows(&self, params: &[(&str, String)]) -> Reply {
        self.call("GET", &format!("/channel-flows{}", query(params)), None)
            .await
    }

    /// Create a channel flow with one processor and return its ARN.
    async fn channel_flow(&self, app_instance_arn: &str, name: &str, token: &str) -> String {
        let reply = self
            .create_channel_flow(&json!({
                "AppInstanceArn": app_instance_arn,
                "Name": name,
                "ClientRequestToken": token,
                "Processors": [processor(name, 1, "ABORT", "ASYNC", PROCESSOR_LAMBDA)],
            }))
            .await;
        assert_eq!(
            reply.status, 201,
            "case {name:?}: CreateChannelFlow must answer 201, got {} {:?}",
            reply.status, reply.body
        );
        reply.body["ChannelFlowArn"]
            .as_str()
            .unwrap_or_else(|| {
                panic!("case {name:?}: CreateChannelFlow must return a ChannelFlowArn")
            })
            .to_string()
    }

    async fn describe_channel(&self, channel_arn: &str) -> Reply {
        self.call("GET", &format!("/channels/{}", encode(channel_arn)), None)
            .await
    }

    /// `AssociateChannelFlow`: the association is a sub-resource of the CHANNEL
    /// (`PUT /channels/{ChannelArn}/channel-flow`), with the flow in the body.
    async fn associate_channel_flow(&self, channel_arn: &str, channel_flow_arn: &str) -> Reply {
        self.associate_channel_flow_as(BEARER, channel_arn, channel_flow_arn)
            .await
    }

    async fn associate_channel_flow_as(
        &self,
        bearer: &str,
        channel_arn: &str,
        channel_flow_arn: &str,
    ) -> Reply {
        self.call_as(
            bearer,
            "PUT",
            &format!("/channels/{}/channel-flow", encode(channel_arn)),
            Some(&json!({ "ChannelFlowArn": channel_flow_arn })),
        )
        .await
    }

    async fn disassociate_channel_flow(&self, channel_arn: &str, channel_flow_arn: &str) -> Reply {
        self.call(
            "DELETE",
            &format!(
                "/channels/{}/channel-flow/{}",
                encode(channel_arn),
                encode(channel_flow_arn)
            ),
            None,
        )
        .await
    }

    /// `ListChannelsAssociatedWithChannelFlow`, whose route is `GET /channels` with the
    /// literal `scope=channel-flow-associations` the model pins on it.
    async fn list_associated_channels(
        &self,
        channel_flow_arn: &str,
        extra: &[(&str, String)],
    ) -> Reply {
        let mut params = vec![
            ("scope", "channel-flow-associations".to_string()),
            ("channel-flow-arn", channel_flow_arn.to_string()),
        ];
        params.extend(extra.iter().map(|(key, value)| (*key, value.clone())));
        self.call("GET", &format!("/channels{}", query(&params)), None)
            .await
    }

    /// Create a channel from `request` and return its ARN.
    async fn channel(&self, request: &Value) -> String {
        let reply = self.call("POST", "/channels", Some(request)).await;
        assert_eq!(
            reply.status, 201,
            "case {:?}: CreateChannel must answer 201, got {} {:?}",
            request["Name"], reply.status, reply.body
        );
        reply.body["ChannelArn"]
            .as_str()
            .unwrap_or_else(|| panic!("CreateChannel must return a ChannelArn: {:?}", reply.body))
            .to_string()
    }

    /// Create a channel with only the required members and return its ARN.
    async fn plain_channel(&self, name: &str, token: &str) -> String {
        self.channel(&json!({
            "AppInstanceArn": APP_INSTANCE,
            "Name": name,
            "ClientRequestToken": token,
        }))
        .await
    }
}

/// One modelled `Processor` as the wire carries it.
fn processor(
    name: &str,
    execution_order: i64,
    fallback_action: &str,
    invocation_type: &str,
    resource_arn: &str,
) -> Value {
    json!({
        "Name": name,
        "ExecutionOrder": execution_order,
        "FallbackAction": fallback_action,
        "Configuration": {
            "Lambda": {
                "ResourceArn": resource_arn,
                "InvocationType": invocation_type,
            }
        }
    })
}

#[tokio::test]
async fn a_channel_flow_reads_back_as_it_was_created() {
    let emulator = Emulator::new();

    // (case, the processors the request declares)
    //
    // The rows span the modelled `ProcessorList len 1..3` at both ends, every
    // `FallbackAction` wire value, every `ChannelFlowExecutionOrder` inside its modelled
    // `range 1..3`, and every `InvocationType` value.
    let mut rows: Vec<(String, Vec<Value>)> = vec![
        (
            "one processor, the shortest list the model allows".to_string(),
            vec![processor("only", 1, "ABORT", "ASYNC", PROCESSOR_LAMBDA)],
        ),
        (
            "two processors in execution order".to_string(),
            vec![
                processor("first", 1, "CONTINUE", "ASYNC", PROCESSOR_LAMBDA),
                processor("second", 2, "ABORT", "ASYNC", PROCESSOR_LAMBDA),
            ],
        ),
        (
            "three processors, the longest list the model allows".to_string(),
            vec![
                processor("first", 1, "ABORT", "ASYNC", PROCESSOR_LAMBDA),
                processor("second", 2, "CONTINUE", "ASYNC", PROCESSOR_LAMBDA),
                processor(
                    "third",
                    3,
                    "ABORT",
                    "ASYNC",
                    "arn:aws:lambda:eu-west-1:210987654321:function:another-processor:$LATEST",
                ),
            ],
        ),
    ];
    for action in FALLBACK_ACTIONS {
        for invocation in INVOCATION_TYPES {
            rows.push((
                format!("a single processor with FallbackAction {action} / {invocation}"),
                vec![processor(
                    "lone",
                    1,
                    action,
                    invocation,
                    PROCESSOR_LAMBDA,
                )],
            ));
        }
    }

    for (index, (case, processors)) in rows.into_iter().enumerate() {
        let name = format!("flow-{index}");
        let mut request = json!({
            "AppInstanceArn": APP_INSTANCE,
            "Name": name,
            "ClientRequestToken": format!("token-{index}"),
            "Processors": processors,
        });
        // The optional `Tags` member is present on the even rows and ABSENT (not null:
        // the modelled `TagList` is `len 1..50`) on the odd ones, so it is exercised both
        // ways.
        if index % 2 == 0 {
            request["Tags"] = json!([{"Key": "team", "Value": "chat"}]);
        }
        let created = emulator.create_channel_flow(&request).await;
        assert_eq!(
            created.status, 201,
            "case {case:?}: expected HTTP 201 from CreateChannelFlow, got {} ({:?})",
            created.status, created.body
        );
        let arn = created.body["ChannelFlowArn"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            arn.starts_with(&format!("{APP_INSTANCE}/channel-flow/")),
            "case {case:?}: expected a ChannelFlowArn under the app instance, got {arn:?}"
        );

        let described = emulator.describe_channel_flow(&arn).await;
        assert_eq!(
            described.status, 200,
            "case {case:?}: expected HTTP 200 from DescribeChannelFlow, got {} ({:?})",
            described.status, described.body
        );
        let flow = &described.body["ChannelFlow"];
        assert_eq!(
            flow["ChannelFlowArn"].as_str(),
            Some(arn.as_str()),
            "case {case:?}: expected the created ARN back, got {:?}",
            flow["ChannelFlowArn"]
        );
        assert_eq!(
            flow["Name"].as_str(),
            Some(name.as_str()),
            "case {case:?}: expected the Name the request carried, got {:?}",
            flow["Name"]
        );
        assert_eq!(
            flow["Processors"], request["Processors"],
            "case {case:?}: expected the processors the request declared, in order, got {:?}",
            flow["Processors"]
        );
        for member in ["CreatedTimestamp", "LastUpdatedTimestamp"] {
            assert!(
                flow[member].is_number(),
                "case {case:?}: expected {member} as an epoch-seconds timestamp, got {:?}",
                flow[member]
            );
        }
    }
}

#[tokio::test]
async fn an_arn_that_names_no_stored_channel_flow_is_forbidden() {
    let emulator = Emulator::new();
    let live = emulator.channel_flow(APP_INSTANCE, "live-flow", "token-live").await;
    let deleted = emulator
        .channel_flow(APP_INSTANCE, "doomed-flow", "token-doomed")
        .await;
    let gone = emulator.delete_channel_flow(&deleted).await;
    assert_eq!(
        gone.status, 204,
        "case \"DeleteChannelFlow\": expected HTTP 204, got {} ({:?})",
        gone.status, gone.body
    );

    // (case, the ChannelFlowArn to describe)
    //
    // Six shapes of ARN that name no channel flow this store holds: a flow id nothing
    // created, a flow under another app instance, a flow that was deleted, a channel's
    // ARN rather than a flow's, and the same flow id under another account and another
    // region. Each must be the SAME terminal refusal; the recorded AWS response this rule
    // was filed from is one row of this table (a channel flow whose create was rejected).
    let rows: Vec<(&str, String)> = vec![
        (
            "a channel flow id nothing created",
            format!("{APP_INSTANCE}/channel-flow/never-created"),
        ),
        (
            "a channel flow under another app instance",
            format!("{OTHER_APP_INSTANCE}/channel-flow/never-created"),
        ),
        ("a channel flow that was deleted", deleted.clone()),
        (
            "a channel's ARN rather than a channel flow's",
            format!("{APP_INSTANCE}/channel/abc123"),
        ),
        (
            "the live flow's id under another account",
            live.replace("111122223333", "444455556666"),
        ),
        (
            "the live flow's id in another region",
            live.replace("us-east-1", "eu-west-1"),
        ),
    ];

    for (case, arn) in rows {
        let reply = emulator.describe_channel_flow(&arn).await;
        assert_eq!(
            (reply.status, reply.code()),
            (403, "Forbidden"),
            "case {case:?} ({arn}): expected the modelled ForbiddenException — HTTP 403 with \
             Code \"Forbidden\" — got {} {:?} ({:?})",
            reply.status,
            reply.code(),
            reply.body
        );
        assert_eq!(
            reply.content_type, "application/json",
            "case {case:?}: a restJson1 error is framed as application/json, got {:?}",
            reply.content_type
        );
        assert!(
            reply.body.get("Message").and_then(Value::as_str).is_some(),
            "case {case:?}: the modelled ForbiddenException carries a Message, got {:?}",
            reply.body
        );
        // restJson1 resolves the error shape from this header, so it must name a shape
        // this operation models — never a code no model declares (the router's own
        // `AmbiguousService`, say).
        assert!(
            DESCRIBE_CHANNEL_FLOW_ERRORS.contains(&reply.error_shape()),
            "case {case:?}: expected x-amzn-errortype to name one of this operation's \
             modelled errors {DESCRIBE_CHANNEL_FLOW_ERRORS:?}, got {:?}",
            reply.error_type
        );
        assert_eq!(
            reply.error_shape(),
            "ForbiddenException",
            "case {case:?}: expected x-amzn-errortype to name ForbiddenException, got {:?}",
            reply.error_type
        );
    }

    // The flow that is still there is still readable: the refusal above is about the ARN,
    // not about the operation.
    let described = emulator.describe_channel_flow(&live).await;
    assert_eq!(
        described.status, 200,
        "case \"the live channel flow\": expected HTTP 200, got {} ({:?})",
        described.status, described.body
    );
}

#[tokio::test]
async fn deleting_a_channel_flow_removes_it_from_every_reader() {
    let emulator = Emulator::new();
    let arn = emulator.channel_flow(APP_INSTANCE, "flow", "token-1").await;

    let listed = emulator
        .list_channel_flows(&[("app-instance-arn", APP_INSTANCE.to_string())])
        .await;
    assert_eq!(
        listed.body["ChannelFlows"]
            .as_array()
            .map(|flows| flows.len()),
        Some(1),
        "case \"before the delete\": expected the one created flow in ListChannelFlows, got {:?}",
        listed.body
    );

    let deleted = emulator.delete_channel_flow(&arn).await;
    assert_eq!(
        deleted.status, 204,
        "case \"DeleteChannelFlow\": expected HTTP 204, got {} ({:?})",
        deleted.status, deleted.body
    );

    let described = emulator.describe_channel_flow(&arn).await;
    assert_eq!(
        (described.status, described.code()),
        (403, "Forbidden"),
        "case \"DescribeChannelFlow after the delete\": expected HTTP 403 Forbidden, got {} {:?}",
        described.status,
        described.code()
    );
    let listed = emulator
        .list_channel_flows(&[("app-instance-arn", APP_INSTANCE.to_string())])
        .await;
    assert_eq!(
        listed.body["ChannelFlows"]
            .as_array()
            .map(|flows| flows.len()),
        Some(0),
        "case \"after the delete\": expected ListChannelFlows to report none, got {:?}",
        listed.body
    );
    let twice = emulator.delete_channel_flow(&arn).await;
    assert_eq!(
        (twice.status, twice.code()),
        (403, "Forbidden"),
        "case \"DeleteChannelFlow twice\": expected HTTP 403 Forbidden the second time, got {} \
         {:?}",
        twice.status,
        twice.code()
    );
}

#[tokio::test]
async fn list_channel_flows_pages_one_app_instances_flows_in_arn_order() {
    let emulator = Emulator::new();

    // Nine flows under the app instance under test and two under another one, so the
    // filter has something to exclude at every page size.
    let mut expected: Vec<String> = Vec::new();
    for index in 0..9 {
        expected.push(
            emulator
                .channel_flow(APP_INSTANCE, &format!("flow-{index}"), &format!("token-{index}"))
                .await,
        );
    }
    for index in 0..2 {
        emulator
            .channel_flow(
                OTHER_APP_INSTANCE,
                &format!("other-{index}"),
                &format!("other-token-{index}"),
            )
            .await;
    }
    expected.sort();

    // Page sizes spanning the modelled bound: one at a time, an uneven split, exactly the
    // page count, more than there are, and the modelled maximum.
    for page_size in [1_i64, 2, 4, 9, 20, MAX_RESULTS_MAX] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut params = vec![
                ("app-instance-arn", APP_INSTANCE.to_string()),
                ("max-results", page_size.to_string()),
            ];
            if let Some(token) = &token {
                params.push(("next-token", token.clone()));
            }
            let reply = emulator.list_channel_flows(&params).await;
            assert_eq!(
                reply.status, 200,
                "case \"page size {page_size}\": expected HTTP 200 from ListChannelFlows, got {} \
                 ({:?})",
                reply.status, reply.body
            );
            let page = reply.body["ChannelFlows"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            assert!(
                i64::try_from(page.len()).unwrap_or(i64::MAX) <= page_size,
                "case \"page size {page_size}\": a page must hold at most MaxResults entries, got \
                 {}",
                page.len()
            );
            for summary in page {
                seen.push(
                    summary["ChannelFlowArn"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                );
            }
            match reply.body.get("NextToken").and_then(Value::as_str) {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            seen, expected,
            "case \"page size {page_size}\": expected this app instance's nine flows in ARN \
             order, got {seen:?}"
        );
    }

    // A page size outside the modelled `range 1..50` is a client error, whichever side of
    // the bound it falls on.
    for page_size in ["0", "51"] {
        let reply = emulator
            .list_channel_flows(&[
                ("app-instance-arn", APP_INSTANCE.to_string()),
                ("max-results", page_size.to_string()),
            ])
            .await;
        assert_eq!(
            reply.status, 400,
            "case \"max-results {page_size}\": expected HTTP 400 for a page size outside the \
             modelled bound, got {} ({:?})",
            reply.status, reply.body
        );
    }

    // A token this service never issued is refused rather than silently restarting the
    // listing.
    let reply = emulator
        .list_channel_flows(&[
            ("app-instance-arn", APP_INSTANCE.to_string()),
            ("next-token", "not-a-token".to_string()),
        ])
        .await;
    assert_eq!(
        reply.status, 400,
        "case \"a foreign NextToken\": expected HTTP 400, got {} ({:?})",
        reply.status, reply.body
    );
}

#[tokio::test]
async fn a_replayed_client_request_token_answers_with_the_first_channel_flow() {
    let emulator = Emulator::new();
    let request = json!({
        "AppInstanceArn": APP_INSTANCE,
        "Name": "idempotent",
        "ClientRequestToken": "token-replayed",
        "Processors": [processor("only", 1, "ABORT", "ASYNC", PROCESSOR_LAMBDA)],
    });

    let first = emulator.create_channel_flow(&request).await;
    let replay = emulator.create_channel_flow(&request).await;
    assert_eq!(
        (first.status, replay.status),
        (201, 201),
        "case \"a replayed ClientRequestToken\": both answers are 201, got {} and {}",
        first.status,
        replay.status
    );
    assert_eq!(
        first.body["ChannelFlowArn"], replay.body["ChannelFlowArn"],
        "case \"a replayed ClientRequestToken\": expected the first flow's ARN again, got {:?} \
         after {:?}",
        replay.body["ChannelFlowArn"], first.body["ChannelFlowArn"]
    );

    // A DIFFERENT token under the same app instance mints a second flow, so the ledger
    // keys on the token rather than on the request shape.
    let mut other = request.clone();
    other["ClientRequestToken"] = json!("token-other");
    let second = emulator.create_channel_flow(&other).await;
    assert_ne!(
        first.body["ChannelFlowArn"], second.body["ChannelFlowArn"],
        "case \"a second ClientRequestToken\": expected a new flow, got the first one's ARN {:?}",
        second.body["ChannelFlowArn"]
    );

    let listed = emulator
        .list_channel_flows(&[("app-instance-arn", APP_INSTANCE.to_string())])
        .await;
    assert_eq!(
        listed.body["ChannelFlows"]
            .as_array()
            .map(|flows| flows.len()),
        Some(2),
        "case \"a replayed ClientRequestToken\": the store holds two flows, not three, got {:?}",
        listed.body
    );
}

#[tokio::test]
async fn a_processor_outside_the_modelled_value_sets_is_refused() {
    let emulator = Emulator::new();

    // (case, the processor the request declares) — one row per member whose value set the
    // model declares, each with a value outside it.
    let rows: Vec<(&str, Value)> = vec![
        (
            "a FallbackAction the model does not declare",
            processor("p", 1, "MAYBE", "ASYNC", PROCESSOR_LAMBDA),
        ),
        (
            "an InvocationType the model does not declare",
            processor("p", 1, "ABORT", "SYNC", PROCESSOR_LAMBDA),
        ),
        (
            "an ExecutionOrder past the modelled range",
            processor("p", 4, "ABORT", "ASYNC", PROCESSOR_LAMBDA),
        ),
        (
            "a ResourceArn that is not a Lambda function ARN",
            processor(
                "p",
                1,
                "ABORT",
                "ASYNC",
                "arn:aws:sqs:us-east-1:111122223333:queue/not-a-function",
            ),
        ),
    ];

    for (index, (case, processor)) in rows.into_iter().enumerate() {
        let reply = emulator
            .create_channel_flow(&json!({
                "AppInstanceArn": APP_INSTANCE,
                "Name": "refused",
                "ClientRequestToken": format!("refused-{index}"),
                "Processors": [processor],
            }))
            .await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: expected HTTP 400 and no channel flow written, got {} ({:?})",
            reply.status, reply.body
        );
    }

    // Nothing above was stored: a refused create writes no flow.
    let listed = emulator
        .list_channel_flows(&[("app-instance-arn", APP_INSTANCE.to_string())])
        .await;
    assert_eq!(
        listed.body["ChannelFlows"]
            .as_array()
            .map(|flows| flows.len()),
        Some(0),
        "case \"after four refused creates\": expected an empty store, got {:?}",
        listed.body
    );
}

// ---------------------------------------------------------------------------
// AssociateChannelFlow / DisassociateChannelFlow /
// ListChannelsAssociatedWithChannelFlow
// ---------------------------------------------------------------------------

/// The `ChannelArn`s a `ListChannelsAssociatedWithChannelFlow` reply reports, sorted.
fn listed_channels(reply: &Reply) -> Vec<String> {
    let mut arns: Vec<String> = reply.body["Channels"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|summary| {
            summary["ChannelArn"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    arns.sort();
    arns
}

/// RULE: a channel carries at most ONE channel flow. `AssociateChannelFlow` writes that
/// association, REPLACING the one the channel held, and every reader reports the one it
/// carries now — `DescribeChannel`'s `ChannelFlowArn`, and the channels each flow's
/// `ListChannelsAssociatedWithChannelFlow` reports — until `DisassociateChannelFlow`
/// removes it.
///
/// The rows span the channel the association is written on: both `ChannelPrivacy` values,
/// both `ChannelMode` values, `Metadata` present and absent, and a caller-supplied
/// `ChannelId`. The flows span the modelled `ProcessorList len 1..3` at both ends.
#[tokio::test]
async fn associating_a_channel_flow_replaces_the_one_the_channel_carried() {
    // (case, the CreateChannel request, the Mode and Privacy the channel ends up with)
    let rows: Vec<(&str, Value, &str, &str)> = vec![
        (
            "a channel taking both defaults",
            json!({"AppInstanceArn": APP_INSTANCE, "Name": "defaults"}),
            "UNRESTRICTED",
            "PUBLIC",
        ),
        (
            "a PRIVATE channel",
            json!({"AppInstanceArn": APP_INSTANCE, "Name": "private", "Privacy": "PRIVATE"}),
            "UNRESTRICTED",
            "PRIVATE",
        ),
        (
            "a RESTRICTED channel carrying Metadata",
            json!({
                "AppInstanceArn": APP_INSTANCE,
                "Name": "restricted",
                "Mode": "RESTRICTED",
                "Metadata": "{\"tier\":\"gold\"}",
            }),
            "RESTRICTED",
            "PUBLIC",
        ),
        (
            "a channel created with its own ChannelId",
            json!({
                "AppInstanceArn": APP_INSTANCE,
                "Name": "explicit-id",
                "ChannelId": "support-desk",
                "Privacy": "PRIVATE",
                "Mode": "RESTRICTED",
            }),
            "RESTRICTED",
            "PRIVATE",
        ),
    ];

    for (index, (case, mut request, mode, privacy)) in rows.into_iter().enumerate() {
        let emulator = Emulator::new();
        request["ClientRequestToken"] = json!(format!("channel-token-{index}"));
        let channel = emulator.channel(&request).await;
        // One flow with the shortest processor list the model allows and one with the
        // longest, so the association is exercised against both ends of it.
        let first = emulator
            .channel_flow(APP_INSTANCE, &format!("first-{index}"), &format!("f-{index}"))
            .await;
        let second = emulator
            .create_channel_flow(&json!({
                "AppInstanceArn": APP_INSTANCE,
                "Name": format!("second-{index}"),
                "ClientRequestToken": format!("s-{index}"),
                "Processors": [
                    processor("one", 1, "ABORT", "ASYNC", PROCESSOR_LAMBDA),
                    processor("two", 2, "CONTINUE", "ASYNC", PROCESSOR_LAMBDA),
                    processor("three", 3, "ABORT", "ASYNC", PROCESSOR_LAMBDA),
                ],
            }))
            .await
            .body["ChannelFlowArn"]
            .as_str()
            .unwrap_or_default()
            .to_string();

        // Before any association: the channel carries no flow and neither flow reports it.
        let described = emulator.describe_channel(&channel).await;
        assert!(
            described.body["Channel"]["ChannelFlowArn"].is_null(),
            "case {case:?}: a channel carries no ChannelFlowArn until one is associated, \
             got {:?}",
            described.body["Channel"]["ChannelFlowArn"]
        );
        for (label, flow) in [("the first flow", &first), ("the second flow", &second)] {
            let listed = emulator.list_associated_channels(flow, &[]).await;
            assert_eq!(
                (listed.status, listed_channels(&listed)),
                (200, Vec::new()),
                "case {case:?}: {label} is associated with nothing yet, got {} {:?}",
                listed.status,
                listed.body
            );
        }

        // Associating the first flow.
        let associated = emulator.associate_channel_flow(&channel, &first).await;
        assert_eq!(
            associated.status, 200,
            "case {case:?}: expected HTTP 200 from AssociateChannelFlow, got {} ({:?})",
            associated.status, associated.body
        );
        let described = emulator.describe_channel(&channel).await;
        assert_eq!(
            described.body["Channel"]["ChannelFlowArn"].as_str(),
            Some(first.as_str()),
            "case {case:?}: expected DescribeChannel to report the associated flow, got {:?}",
            described.body["Channel"]["ChannelFlowArn"]
        );
        let listed = emulator.list_associated_channels(&first, &[]).await;
        assert_eq!(
            listed_channels(&listed),
            vec![channel.clone()],
            "case {case:?}: expected the first flow to report this one channel, got {:?}",
            listed.body
        );
        // The summary describes the CHANNEL the flow is associated with, with every member
        // `ChannelAssociatedWithFlowSummary` carries
        // (`smithy_query(query="shape", shape="ChannelAssociatedWithFlowSummary")`:
        // `Name`, `ChannelArn`, `Mode`, `Privacy`, `Metadata`).
        let summary = &listed.body["Channels"][0];
        assert_eq!(
            (
                summary["Name"].as_str(),
                summary["Mode"].as_str(),
                summary["Privacy"].as_str(),
                summary["Metadata"].as_str(),
            ),
            (
                request["Name"].as_str(),
                Some(mode),
                Some(privacy),
                request.get("Metadata").and_then(Value::as_str),
            ),
            "case {case:?}: expected the channel's own name, mode, privacy and metadata in \
             the summary, got {summary:?}"
        );

        // Associating the SAME flow again changes nothing: the channel is associated with
        // it once, not twice.
        let again = emulator.associate_channel_flow(&channel, &first).await;
        assert_eq!(
            again.status, 200,
            "case {case:?}: re-associating the same flow is accepted, got {} ({:?})",
            again.status, again.body
        );
        let listed = emulator.list_associated_channels(&first, &[]).await;
        assert_eq!(
            listed_channels(&listed),
            vec![channel.clone()],
            "case {case:?}: expected the channel listed once after re-associating, got {:?}",
            listed.body
        );

        // Associating a SECOND flow replaces the first.
        let replaced = emulator.associate_channel_flow(&channel, &second).await;
        assert_eq!(
            replaced.status, 200,
            "case {case:?}: expected HTTP 200 associating a second flow, got {} ({:?})",
            replaced.status, replaced.body
        );
        let described = emulator.describe_channel(&channel).await;
        assert_eq!(
            described.body["Channel"]["ChannelFlowArn"].as_str(),
            Some(second.as_str()),
            "case {case:?}: expected the channel to carry the second flow, got {:?}",
            described.body["Channel"]["ChannelFlowArn"]
        );
        let listed = emulator.list_associated_channels(&first, &[]).await;
        assert_eq!(
            listed_channels(&listed),
            Vec::<String>::new(),
            "case {case:?}: expected the replaced flow to report no channels, got {:?}",
            listed.body
        );
        let listed = emulator.list_associated_channels(&second, &[]).await;
        assert_eq!(
            listed_channels(&listed),
            vec![channel.clone()],
            "case {case:?}: expected the second flow to report the channel, got {:?}",
            listed.body
        );

        // The REPLACED flow is no longer this channel's, so disassociating it removes
        // nothing and is refused.
        let refused = emulator.disassociate_channel_flow(&channel, &first).await;
        assert_eq!(
            (refused.status, refused.code(), refused.message()),
            (400, "BadRequest", FLOW_NOT_ASSOCIATED),
            "case {case:?}: expected HTTP 400 BadRequest {FLOW_NOT_ASSOCIATED:?} \
             disassociating the replaced flow, got {} {:?}",
            refused.status,
            refused.body
        );

        // Disassociating the flow the channel does carry leaves it with none.
        let removed = emulator.disassociate_channel_flow(&channel, &second).await;
        assert_eq!(
            removed.status, 204,
            "case {case:?}: expected HTTP 204 from DisassociateChannelFlow, got {} ({:?})",
            removed.status, removed.body
        );
        let described = emulator.describe_channel(&channel).await;
        assert!(
            described.body["Channel"]["ChannelFlowArn"].is_null(),
            "case {case:?}: expected no ChannelFlowArn after the disassociation, got {:?}",
            described.body["Channel"]["ChannelFlowArn"]
        );
        let listed = emulator.list_associated_channels(&second, &[]).await;
        assert_eq!(
            listed_channels(&listed),
            Vec::<String>::new(),
            "case {case:?}: expected the flow to report no channels after the \
             disassociation, got {:?}",
            listed.body
        );
    }
}

/// RULE: a channel flow any channel is still associated with is not deleted: the request
/// is refused and the flow stays readable, until every channel that carries it has been
/// disassociated.
///
/// The rows span the number of channels one flow is associated with, because the rule is
/// about ANY channel carrying it rather than about a particular one.
#[tokio::test]
async fn a_channel_flow_a_channel_still_carries_is_not_deleted() {
    for channel_count in [1_usize, 2, 3] {
        let case = format!("a flow associated with {channel_count} channel(s)");
        let emulator = Emulator::new();
        let flow = emulator.channel_flow(APP_INSTANCE, "flow", "flow-token").await;
        let mut channels: Vec<String> = Vec::new();
        for index in 0..channel_count {
            let channel = emulator
                .plain_channel(&format!("ch-{index}"), &format!("ch-token-{index}"))
                .await;
            let associated = emulator.associate_channel_flow(&channel, &flow).await;
            assert_eq!(
                associated.status, 200,
                "case {case:?}: expected HTTP 200 associating channel {index}, got {} ({:?})",
                associated.status, associated.body
            );
            channels.push(channel);
        }

        // One channel is freed per round, and while any still carries the flow the delete
        // is refused and the flow is still there.
        for (round, channel) in channels.iter().enumerate() {
            let refused = emulator.delete_channel_flow(&flow).await;
            assert_eq!(
                (refused.status, refused.code(), refused.message()),
                (403, "Forbidden", FLOW_STILL_ASSOCIATED),
                "case {case:?}, round {round}: expected HTTP 403 Forbidden \
                 {FLOW_STILL_ASSOCIATED:?}, got {} {:?}",
                refused.status,
                refused.body
            );
            let described = emulator.describe_channel_flow(&flow).await;
            assert_eq!(
                described.status, 200,
                "case {case:?}, round {round}: the refused delete removed nothing, so the \
                 flow is still readable, got {} ({:?})",
                described.status, described.body
            );
            let listed = emulator.list_associated_channels(&flow, &[]).await;
            assert_eq!(
                listed_channels(&listed).len(),
                channel_count - round,
                "case {case:?}, round {round}: expected the channels still carrying the \
                 flow, got {:?}",
                listed.body
            );
            let removed = emulator.disassociate_channel_flow(channel, &flow).await;
            assert_eq!(
                removed.status, 204,
                "case {case:?}, round {round}: expected HTTP 204 from \
                 DisassociateChannelFlow, got {} ({:?})",
                removed.status, removed.body
            );
        }

        // With no channel carrying it, the flow deletes.
        let deleted = emulator.delete_channel_flow(&flow).await;
        assert_eq!(
            deleted.status, 204,
            "case {case:?}: expected HTTP 204 once no channel carries the flow, got {} ({:?})",
            deleted.status, deleted.body
        );
        let described = emulator.describe_channel_flow(&flow).await;
        assert_eq!(
            described.status, 403,
            "case {case:?}: expected the deleted flow to be gone, got {} ({:?})",
            described.status, described.body
        );
    }
}

/// RULE: `ListChannelsAssociatedWithChannelFlow` reports every channel associated with the
/// flow it names exactly once across its pages, in `ChannelArn` order, and no channel that
/// carries another flow or none.
///
/// The page sizes span the modelled `MaxResults range 1..50`: one at a time, uneven
/// splits, exactly the page count, more than there are, and the bound itself — plus one
/// value either side of it, which is a client error.
#[tokio::test]
async fn the_associated_channel_listing_pages_every_channel_once() {
    let emulator = Emulator::new();
    let flow = emulator.channel_flow(APP_INSTANCE, "flow", "flow-token").await;
    let other_flow = emulator
        .channel_flow(APP_INSTANCE, "other-flow", "other-flow-token")
        .await;

    let mut expected: Vec<String> = Vec::new();
    for index in 0..9 {
        let channel = emulator
            .plain_channel(&format!("ch-{index}"), &format!("ch-token-{index}"))
            .await;
        emulator.associate_channel_flow(&channel, &flow).await;
        expected.push(channel);
    }
    // Two channels carry the OTHER flow and one carries none, so the listing has something
    // to exclude at every page size.
    for index in 0..2 {
        let channel = emulator
            .plain_channel(&format!("other-{index}"), &format!("other-token-{index}"))
            .await;
        emulator.associate_channel_flow(&channel, &other_flow).await;
    }
    emulator.plain_channel("lonely", "lonely-token").await;
    expected.sort();

    for page_size in [1_i64, 2, 4, 9, 20, MAX_RESULTS_MAX] {
        let case = format!("page size {page_size}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut params = vec![("max-results", page_size.to_string())];
            if let Some(token) = &token {
                params.push(("next-token", token.clone()));
            }
            let reply = emulator.list_associated_channels(&flow, &params).await;
            assert_eq!(
                reply.status, 200,
                "case {case:?}: expected HTTP 200 from the listing, got {} ({:?})",
                reply.status, reply.body
            );
            let page = reply.body["Channels"].as_array().cloned().unwrap_or_default();
            assert!(
                i64::try_from(page.len()).unwrap_or(i64::MAX) <= page_size,
                "case {case:?}: a page holds at most MaxResults entries, got {}",
                page.len()
            );
            seen.extend(
                page.iter()
                    .map(|summary| summary["ChannelArn"].as_str().unwrap_or_default().to_string()),
            );
            match reply.body.get("NextToken").and_then(Value::as_str) {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            seen, expected,
            "case {case:?}: expected the nine associated channels in ARN order, each once, \
             got {seen:?}"
        );
    }

    // A page size outside the modelled `range 1..50` is a client error, whichever side of
    // the bound it falls on.
    for page_size in ["0", "51"] {
        let reply = emulator
            .list_associated_channels(&flow, &[("max-results", page_size.to_string())])
            .await;
        assert_eq!(
            reply.status, 400,
            "case \"max-results {page_size}\": expected HTTP 400 for a page size outside the \
             modelled bound, got {} ({:?})",
            reply.status, reply.body
        );
    }
}

/// RULE: a `NextToken` this service cannot read a cursor out of — one it did not issue, or
/// one that no longer names a position in the view it was issued for — is refused with
/// HTTP 400 `ValidationException` naming the operation it was presented to; a token the
/// service DID issue resumes the listing.
///
/// The rows span the shapes such a token arrives in: text that is not this service's
/// encoding at all, an odd-length one, a well-formed cursor naming something outside this
/// view, and a real token cut short.
#[tokio::test]
async fn a_next_token_this_service_did_not_issue_is_refused() {
    let emulator = Emulator::new();
    let flow = emulator.channel_flow(APP_INSTANCE, "flow", "flow-token").await;
    let mut associated: Vec<String> = Vec::new();
    for index in 0..3 {
        let channel = emulator
            .plain_channel(&format!("ch-{index}"), &format!("ch-token-{index}"))
            .await;
        emulator.associate_channel_flow(&channel, &flow).await;
        associated.push(channel);
    }
    let outside = emulator.plain_channel("outside", "outside-token").await;
    associated.sort();

    // A token this service issued resumes the listing where the page before it ended.
    let first = emulator
        .list_associated_channels(&flow, &[("max-results", "1".to_string())])
        .await;
    let token = first
        .body
        .get("NextToken")
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            panic!("case \"a page with more to come\": expected a NextToken, got {first:?}")
        })
        .to_string();
    let second = emulator
        .list_associated_channels(
            &flow,
            &[
                ("max-results", "1".to_string()),
                ("next-token", token.clone()),
            ],
        )
        .await;
    assert_eq!(
        (second.status, listed_channels(&second)),
        (200, vec![associated[1].clone()]),
        "case \"a token this service issued\": expected the second channel in ARN order, \
         got {} {:?}",
        second.status,
        second.body
    );

    // (case, the token the request carries)
    let rows: Vec<(&str, String)> = vec![
        (
            "text that is not this service's token encoding",
            "not-a-token".to_string(),
        ),
        ("an odd-length token", "abc".to_string()),
        (
            "a well-formed cursor naming a channel outside this view",
            hex::encode(outside.as_bytes()),
        ),
        (
            "a token this service issued, cut short",
            token[..token.len() - 2].to_string(),
        ),
    ];

    for (case, token) in rows {
        let reply = emulator
            .list_associated_channels(&flow, &[("next-token", token.clone())])
            .await;
        assert_eq!(
            (reply.status, reply.error_shape(), reply.message()),
            (400, "ValidationException", TOKEN_EXCEPTION),
            "case {case:?} ({token}): expected HTTP 400 ValidationException \
             {TOKEN_EXCEPTION:?}, got {} {:?} ({:?})",
            reply.status,
            reply.error_type,
            reply.body
        );
    }
}

/// RULE: an association is written, removed and reported only for resources this service
/// holds under the channel's app instance, and only for a caller that moderates the
/// channel or administers its app instance; every other request is refused and the stored
/// association is left exactly as it was.
///
/// The rows span the ways a request fails that gate: an unresolvable channel, an
/// unresolvable channel flow, a channel flow held under ANOTHER app instance, a caller
/// without the role, and a `ChannelFlowArn` that is not the one the channel carries.
#[tokio::test]
async fn an_association_is_refused_without_the_resources_and_the_role() {
    let emulator = Emulator::new();
    let channel = emulator.plain_channel("channel", "channel-token").await;
    let flow = emulator.channel_flow(APP_INSTANCE, "flow", "flow-token").await;
    let other_flow = emulator
        .channel_flow(APP_INSTANCE, "other-flow", "other-flow-token")
        .await;
    // A channel flow this account holds, but under a DIFFERENT app instance from the
    // channel's: a flow processes the messages of channels in its own app instance.
    let foreign_flow = emulator
        .channel_flow(OTHER_APP_INSTANCE, "foreign-flow", "foreign-token")
        .await;
    let unknown_channel = format!("{APP_INSTANCE}/channel/never-created");
    let unknown_flow = format!("{APP_INSTANCE}/channel-flow/never-created");

    // (case, the ChannelArn, the ChannelFlowArn, the expected Message)
    let rows: Vec<(&str, String, String, &str)> = vec![
        (
            "a channel ARN this service holds no channel for",
            unknown_channel.clone(),
            flow.clone(),
            INSUFFICIENT_ACCESS,
        ),
        (
            "a channel under another app instance",
            format!("{OTHER_APP_INSTANCE}/channel/never-created"),
            flow.clone(),
            INSUFFICIENT_ACCESS,
        ),
        (
            "a channel flow ARN this service holds no flow for",
            channel.clone(),
            unknown_flow.clone(),
            UNRESOLVABLE_APP_INSTANCE,
        ),
        (
            "a channel flow held under another app instance",
            channel.clone(),
            foreign_flow.clone(),
            UNRESOLVABLE_APP_INSTANCE,
        ),
        (
            "a channel's own ARN in place of a channel flow's",
            channel.clone(),
            channel.clone(),
            UNRESOLVABLE_APP_INSTANCE,
        ),
    ];

    for (case, channel_arn, channel_flow_arn, message) in rows {
        let reply = emulator
            .associate_channel_flow(&channel_arn, &channel_flow_arn)
            .await;
        assert_eq!(
            (reply.status, reply.code(), reply.message()),
            (403, "Forbidden", message),
            "case {case:?}: expected HTTP 403 Forbidden {message:?} from \
             AssociateChannelFlow, got {} {:?}",
            reply.status,
            reply.body
        );
    }

    // A caller that neither moderates the channel nor administers its app instance cannot
    // associate a flow, whatever the channel's privacy: the flow rewrites how every
    // message to the channel is processed.
    let refused = emulator
        .associate_channel_flow_as(NON_MODERATOR, &channel, &flow)
        .await;
    assert_eq!(
        (refused.status, refused.code(), refused.message()),
        (403, "Forbidden", INSUFFICIENT_ACCESS),
        "case \"a caller that does not moderate the channel\": expected HTTP 403 Forbidden, \
         got {} {:?}",
        refused.status,
        refused.body
    );

    // Nothing above was written: the channel still carries no flow.
    let described = emulator.describe_channel(&channel).await;
    assert!(
        described.body["Channel"]["ChannelFlowArn"].is_null(),
        "case \"after five refused associations\": expected no association, got {:?}",
        described.body["Channel"]["ChannelFlowArn"]
    );

    // Disassociation answers the same gate, and refuses a flow that is not this channel's.
    let associated = emulator.associate_channel_flow(&channel, &flow).await;
    assert_eq!(
        associated.status, 200,
        "case \"the moderator associates the flow\": expected HTTP 200, got {} ({:?})",
        associated.status, associated.body
    );
    let refused = emulator
        .disassociate_channel_flow(&unknown_channel, &flow)
        .await;
    assert_eq!(
        (refused.status, refused.code(), refused.message()),
        (403, "Forbidden", INSUFFICIENT_ACCESS),
        "case \"disassociating on an unresolvable channel\": expected HTTP 403 Forbidden, \
         got {} {:?}",
        refused.status,
        refused.body
    );
    for (case, channel_flow_arn) in [
        ("another flow this account holds", other_flow.clone()),
        ("a flow ARN nothing created", unknown_flow.clone()),
    ] {
        let refused = emulator
            .disassociate_channel_flow(&channel, &channel_flow_arn)
            .await;
        assert_eq!(
            (refused.status, refused.code(), refused.message()),
            (400, "BadRequest", FLOW_NOT_ASSOCIATED),
            "case {case:?}: expected HTTP 400 BadRequest {FLOW_NOT_ASSOCIATED:?} from \
             DisassociateChannelFlow, got {} {:?}",
            refused.status,
            refused.body
        );
    }
    let described = emulator.describe_channel(&channel).await;
    assert_eq!(
        described.body["Channel"]["ChannelFlowArn"].as_str(),
        Some(flow.as_str()),
        "case \"after two refused disassociations\": expected the association untouched, \
         got {:?}",
        described.body["Channel"]["ChannelFlowArn"]
    );

    // The listing answers for a channel flow this service holds, and refuses an ARN it
    // cannot resolve one for.
    let listed = emulator.list_associated_channels(&flow, &[]).await;
    assert_eq!(
        (listed.status, listed_channels(&listed)),
        (200, vec![channel.clone()]),
        "case \"the listing for a stored flow\": expected the associated channel, got {} {:?}",
        listed.status,
        listed.body
    );
    let refused = emulator.list_associated_channels(&unknown_flow, &[]).await;
    assert_eq!(
        (refused.status, refused.code(), refused.message()),
        (403, "Forbidden", UNRESOLVABLE_APP_INSTANCE),
        "case \"the listing for a flow nothing created\": expected HTTP 403 Forbidden \
         {UNRESOLVABLE_APP_INSTANCE:?}, got {} {:?}",
        refused.status,
        refused.body
    );
}
