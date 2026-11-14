// SPDX-License-Identifier: CC0-1.0

//! CloudFormation creates an `AWS::OpenSearchService::Domain` in Amazon OpenSearch Service,
//! so that service's own judgement of the declared cluster is what decides the stack.
//!
//! The rule under test: **a stack's resource is only real if the service that OWNS it
//! created it, and that service's refusal of the create FAILS the resource and rolls the
//! whole stack back.** A domain whose cluster names an instance type the requested engine
//! release does not offer is refused by Amazon OpenSearch Service with its own
//! `InvalidTypeException`, so the declaring resource fails carrying that refusal — as the
//! type's own resource handler words it — every resource the deployment had already made
//! (the topic the domain `DependsOn`) is deleted again, and the stack settles in
//! `ROLLBACK_COMPLETE`. A pairing that service DOES offer deploys, and the domain reads back
//! through `DescribeDomain` on the release and the type the template declared, with `Ref`
//! and `Fn::GetAtt` resolving to the identifiers that service minted.
//!
//! The rows span what the rule generalises over rather than one recorded template: both
//! engine lines, each instance family the published catalog gates on a release, the TIER of
//! the cluster that names the type (the data nodes, the dedicated masters, the warm tier),
//! and both directions of the rule — a release that does not offer the type and one that
//! does. Which pairings those are is the owning service's own catalogue, read per row from
//! `ListInstanceTypeDetails` rather than restated here, so a row's premise cannot drift from
//! the service it is asserted against.
//!
//! Its second rule is about what a created domain's attributes then CARRY:
//! **`Fn::GetAtt` on a resource the owning service created answers that service's OWN
//! report of the attribute the type publishes, including an attribute whose published value
//! is a MAP, whose entries are read one at a time through a dotted path — so every output of
//! a completed stack over an address the service published carries an `OutputValue`.** A
//! domain reached inside a VPC has no public `DomainStatus$Endpoint` at all and publishes
//! its hosts in `DomainStatus$Endpoints` instead, so the outputs a client addresses such a
//! domain through are exactly the entries of that map.
//!
//! This suite needs THREE services on one engine — CloudFormation reaches Amazon OpenSearch
//! Service and Amazon SNS through the in-process callback — which is why it lives here
//! rather than in any one service's crate.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::Value;
use smithy_server_runtime::MultiServiceRouter;

use crate::query_wire::{elements, form_encode, opt_element, outputs_of};
use service_com_amazonaws_cloudformation_cloudformation::state::{
    failed_action, resource_failure_summary,
};
use service_com_amazonaws_opensearch_amazonopensearchservice::state::{
    ENDPOINTS_KEY_VPC, ENDPOINTS_KEY_VPC_DUALSTACK, IP_ADDRESS_TYPE_DUALSTACK,
};

/// The region every request here is signed for, and so the region the stack's resources and
/// their ARNs belong to.
///
/// `pub(crate)` because the suites that reuse the wire helpers below assert on the ARNs and
/// hosts a resource of that Region carries
/// ([`crate::cloudformation_search_indexing_pipeline_outputs`]): one Region, named once, so a
/// suite's expectations cannot be signed for one Region and written for another.
pub(crate) const REGION: &str = "us-west-2";

/// How long one poll of a stack waits before asking again.
const POLL_INTERVAL: Duration = Duration::from_millis(250);

/// How many times a stack is polled before the suite gives up on it reaching a terminal
/// status. Generous next to the spans that have to fall due — the stack's own transition and
/// the domain's provisioning window, each half a second on this emulator's compressed clock
/// — so a slow machine does not fail the suite.
const POLL_LIMIT: usize = 40;

/// The HTTP status the owning service answers a read of a resource it does not hold on —
/// a domain here, an index in the sibling suite that deploys one.
///
/// That service's `ResourceNotFoundException` carries `@httpError(409)` in its model, and
/// the recorded wire answer of a missing-resource refusal is HTTP 404 — the status a client
/// needs to tell an absent resource from a conflict worth retrying. Stated here, and
/// independently of the constant that service decides it by, so this suite asserts the
/// recorded status rather than whatever the handler currently answers. `pub(crate)` because
/// the sibling suites read a rolled-back domain, and the indexes of a deployed one, back the
/// same way ([`crate::cloudformation_opensearch_domain_deployments`],
/// [`crate::cloudformation_deployed_domain_index_lifecycle`]).
pub(crate) const MISSING_RESOURCE_STATUS: u16 = 404;

/// The emulator with the three services this suite exercises, mounted on ONE engine exactly
/// as `core/router/src/lib.rs` mounts them.
///
/// This router and the readers below are `pub(crate)` because a sibling suite deploys the same
/// resource type and reads it back the same way
/// ([`crate::cloudformation_opensearch_domain_deployments`]): one copy, called twice, rather
/// than a second copy that can drift from this one.
pub(crate) fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_cloudformation_cloudformation::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_opensearch_amazonopensearchservice::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_sns_amazonsimplenotificationservice::EmulatedService::new(engine),
    );
    router
}

/// A SigV4 `Authorization` header whose credential scope names `service`, which is what
/// routes the request. The signature is never checked: request authentication is off.
///
/// `pub(crate)` because a sibling suite dials the DATA plane at the address this stack's
/// outputs published ([`crate::cloudformation_domain_endpoint_is_dialled_by_consumers`]),
/// which is a host no helper here knows: one credential scope, named once, so two suites of
/// the same estate cannot sign for two different Regions.
pub(crate) fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000"
    )
}

/// Call one awsQuery operation of `service`, answered with the HTTP status and the XML body.
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

/// Call one CloudFormation operation (API version 2010-05-15).
pub(crate) async fn cloudformation(
    router: &MultiServiceRouter,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    query(router, "cloudformation", "2010-05-15", action, members).await
}

/// Call one Amazon SNS operation (API version 2010-03-31).
async fn sns(
    router: &MultiServiceRouter,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    query(router, "sns", "2010-03-31", action, members).await
}

/// Call one Amazon OpenSearch Service operation over its `restJson1` wire (signing name
/// `es`), answered with the status and the parsed document.
pub(crate) async fn opensearch(
    router: &MultiServiceRouter,
    method: &'static str,
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

/// The `DomainStatus` Amazon OpenSearch Service reports for `name`, with the status it
/// answered: `DescribeDomain`, `GET /2021-01-01/opensearch/domain/{DomainName}`.
pub(crate) async fn describe_domain(router: &MultiServiceRouter, name: &str) -> (u16, Value) {
    let (status, body) = opensearch(
        router,
        "GET",
        &format!("/2021-01-01/opensearch/domain/{name}"),
        None,
    )
    .await;
    let reported = body
        .get("DomainStatus")
        .cloned()
        .unwrap_or_else(|| body.clone());
    (status, reported)
}

/// The tags Amazon OpenSearch Service holds for one resource, as the map a client reads them
/// as: `ListTags`, `GET /2021-01-01/tags?arn=<arn>`.
async fn domain_tags(
    router: &MultiServiceRouter,
    case: &str,
    arn: &str,
) -> BTreeMap<String, String> {
    let (status, body) = opensearch(
        router,
        "GET",
        &format!("/2021-01-01/tags?arn={}", form_encode(arn)),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from ListTags({arn}), got {status} with {body}"
    );
    body.get("TagList")
        .and_then(Value::as_array)
        .map(|tags| {
            tags.iter()
                .filter_map(|tag| {
                    Some((
                        tag.get("Key").and_then(Value::as_str)?.to_string(),
                        tag.get("Value")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                    ))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Every instance type the owning service publishes for one engine release, read to the end
/// of its pagination: `ListInstanceTypeDetails`. This is what each row's premise is checked
/// against, so the suite never asserts the gates against themselves.
async fn offered_instance_types(router: &MultiServiceRouter, engine_version: &str) -> Vec<String> {
    let mut types: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let path = match token.as_deref() {
            None => format!("/2021-01-01/opensearch/instanceTypeDetails/{engine_version}"),
            Some(next) => format!(
                "/2021-01-01/opensearch/instanceTypeDetails/{engine_version}?nextToken={next}"
            ),
        };
        let (status, body) = opensearch(router, "GET", &path, None).await;
        assert_eq!(
            status, 200,
            "case ListInstanceTypeDetails({engine_version}): expected 200, got {status} with \
             {body}"
        );
        types.extend(
            body.get("InstanceTypeDetails")
                .and_then(Value::as_array)
                .unwrap_or(&Vec::new())
                .iter()
                .filter_map(|entry| entry.get("InstanceType").and_then(Value::as_str))
                .map(str::to_string),
        );
        token = body
            .get("NextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        if token.is_none() {
            return types;
        }
    }
}

/// The text of the first `<tag>` of an XML answer, or the empty string.
pub(crate) fn xml_text(xml: &str, tag: &str) -> String {
    opt_element(xml, tag).unwrap_or_default()
}

/// The awsQuery error `<Code>` of a refusal, or `""` on a success envelope.
fn error_code(xml: &str) -> String {
    xml_text(xml, "Code")
}

/// Whether a stack status is one a deployment stops in, so polling it again tells a client
/// nothing new.
fn is_terminal(status: &str) -> bool {
    status.ends_with("_COMPLETE") || status.ends_with("_FAILED")
}

/// Poll `DescribeStacks` until the stack reaches a terminal status, and answer the last body
/// it was described with. Nothing sleeps inside a handler: what each poll collects is what
/// the clock already owed.
pub(crate) async fn settle_stack(
    router: &MultiServiceRouter,
    case: &str,
    stack_name: &str,
) -> String {
    let mut last = String::new();
    for _ in 0..POLL_LIMIT {
        tokio::time::sleep(POLL_INTERVAL).await;
        let (status, body) =
            cloudformation(router, "DescribeStacks", &[("StackName", stack_name)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from DescribeStacks, got {status} with {body}"
        );
        if is_terminal(&xml_text(&body, "StackStatus")) {
            return body;
        }
        last = body;
    }
    panic!("case {case:?}: the stack never reached a terminal status, last seen {last}");
}

/// The ARN a topic named `name` carries in the account this emulator mints ARNs for, read off
/// the topic SNS created rather than rebuilt here.
async fn topic_arn(router: &MultiServiceRouter, case: &str, name: &str) -> String {
    let (status, body) = sns(router, "ListTopics", &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from ListTopics, got {status} with {body}"
    );
    elements(&body, "TopicArn")
        .into_iter()
        .find(|arn| arn.ends_with(&format!(":{name}")))
        .unwrap_or_default()
}

/// One row: an engine release, the cluster a template declares on it, and what the owning
/// service's catalogue says about the pairing.
struct Row {
    case: &'static str,
    stack: &'static str,
    /// The domain the template names, inside `DomainName`'s own `^[a-z][a-z0-9\-]+$` and
    /// 3..28 characters.
    domain: &'static str,
    /// The topic the template declares FIRST, which the domain `DependsOn`: what a rollback
    /// has to take away again.
    topic: &'static str,
    engine_version: &'static str,
    /// The `ClusterConfig` the template declares, as the template spells it.
    cluster: &'static str,
    /// The instance type the row's cluster provisions a node on, which the engine release is
    /// judged against: the one the refusal must name, for a row the release does not offer.
    instance_type: &'static str,
    /// Whether the owning service offers the pairing — checked against its published
    /// catalogue before the row is deployed.
    offered: bool,
}

/// What the owning service answers a DIRECT create of the row's cluster with: the HTTP
/// status and the message, which is what the stack's reason has to be built from.
///
/// Asked under a name of its own, so the direct call neither collides with the domain the
/// stack declares nor leaves one behind.
async fn direct_refusal(router: &MultiServiceRouter, row: &Row) -> (u16, String) {
    let cluster: Value =
        serde_json::from_str(row.cluster).expect("the row's cluster is a JSON object");
    let (status, body) = opensearch(
        router,
        "POST",
        "/2021-01-01/opensearch/domain",
        Some(serde_json::json!({
            "DomainName": format!("direct-{}", row.domain),
            "EngineVersion": row.engine_version,
            "ClusterConfig": cluster,
        })),
    )
    .await;
    let message = body
        .get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    (status, message)
}

/// The message the resource's handler reports for a refusal the owning service answered
/// directly with `message` and `status`: that service's own sentence, behind the opener the
/// handler's invalid-request class carries and in front of the diagnostic the SDK call
/// raises. Restated here rather than imported, so a change to any part of it fails this
/// suite instead of being asserted against itself.
fn expected_refusal(status: u16, message: &str) -> String {
    format!(
        "Invalid request provided: {message} \
         (Service: OpenSearch, Status Code: {status}, Request ID: "
    )
}

/// The `HandlerErrorCode` class a provider's validation refusal is reported under, which is
/// what a client reads to decide whether a failed resource is worth retrying.
const INVALID_REQUEST: &str = "HandlerErrorCode: InvalidRequest";

/// The class a refusal of a service LIMIT is reported under instead, and what the SDK
/// diagnostic then reports: a condition that can clear on its own is retried to the SDK's
/// default maximum of attempts, where a refusal of the request itself is raised on the
/// first.
const SERVICE_LIMIT_EXCEEDED: &str = "HandlerErrorCode: ServiceLimitExceeded";

/// The attempt count the diagnostic carries for a refusal the SDK does not retry.
const SINGLE_ATTEMPT: &str = "(SDK Attempt Count: 1)";

/// And for one it does.
const RETRIED_ATTEMPTS: &str = "(SDK Attempt Count: 3)";

const ROWS: &[Row] = &[
    Row {
        // The OpenSearch-optimized family is offered from OpenSearch 2.11, so an earlier
        // 2.x release does not run it.
        case: "an OpenSearch-optimized data node on an earlier 2.x release",
        stack: "sim-osrb-or1-early",
        domain: "sim-osrb-or1-early",
        topic: "sim-osrb-or1-early-alerts",
        engine_version: "OpenSearch_2.5",
        cluster: r#"{"InstanceType":"or1.medium.search","InstanceCount":2}"#,
        instance_type: "or1.medium.search",
        offered: false,
    },
    Row {
        // The same instance type on the newest supported 2.x release: the refusal is
        // engine-scoped, not a blanket rejection of the type.
        case: "the same data node on the newest supported 2.x release",
        stack: "sim-osrb-or1-newest",
        domain: "sim-osrb-or1-newest",
        topic: "sim-osrb-or1-newest-alerts",
        engine_version: "OpenSearch_2.19",
        cluster: r#"{"InstanceType":"or1.medium.search","InstanceCount":2}"#,
        instance_type: "or1.medium.search",
        offered: true,
    },
    Row {
        // A different family with a different gate, on the OTHER engine line: the Graviton
        // families are offered from Elasticsearch 7.9.
        case: "a Graviton data node on an Elasticsearch release before that family",
        stack: "sim-osrb-graviton-early",
        domain: "sim-osrb-grav-early",
        topic: "sim-osrb-graviton-early-alerts",
        engine_version: "Elasticsearch_7.1",
        cluster: r#"{"InstanceType":"r6g.large.search","InstanceCount":2}"#,
        instance_type: "r6g.large.search",
        offered: false,
    },
    Row {
        case: "a Graviton data node on an OpenSearch release",
        stack: "sim-osrb-graviton-ok",
        domain: "sim-osrb-grav-ok",
        topic: "sim-osrb-graviton-ok-alerts",
        engine_version: "OpenSearch_1.0",
        cluster: r#"{"InstanceType":"r6g.large.search","InstanceCount":2}"#,
        instance_type: "r6g.large.search",
        offered: true,
    },
    Row {
        // The type is not the data nodes' here: every node of the cluster runs the domain's
        // one release, so the dedicated masters are scoped to it too.
        case: "a dedicated master type the release does not offer",
        stack: "sim-osrb-master",
        domain: "sim-osrb-master",
        topic: "sim-osrb-master-alerts",
        engine_version: "OpenSearch_2.9",
        cluster: r#"{"InstanceType":"m5.large.search","InstanceCount":2,
             "DedicatedMasterEnabled":true,"DedicatedMasterType":"or1.large.search",
             "DedicatedMasterCount":3}"#,
        instance_type: "or1.large.search",
        offered: false,
    },
    Row {
        // And the warm tier, whose member names a type of its own enum. UltraWarm is
        // offered from Elasticsearch 6.8, and rests on dedicated master nodes.
        case: "an UltraWarm tier on an Elasticsearch release before that family",
        stack: "sim-osrb-warm",
        domain: "sim-osrb-warm",
        topic: "sim-osrb-warm-alerts",
        engine_version: "Elasticsearch_6.3",
        cluster: r#"{"InstanceType":"m5.large.search","InstanceCount":2,
             "DedicatedMasterEnabled":true,"DedicatedMasterType":"m5.large.search",
             "DedicatedMasterCount":3,"WarmEnabled":true,
             "WarmType":"ultrawarm1.medium.search","WarmCount":2}"#,
        instance_type: "ultrawarm1.medium.search",
        offered: false,
    },
];

/// The template one row deploys: the topic the domain depends on, declared first, then the
/// domain, with the identifiers of the domain as outputs.
fn template(row: &Row) -> String {
    format!(
        r#"{{"Resources":{{
             "Alerts":{{"Type":"AWS::SNS::Topic","Properties":{{"TopicName":"{topic}"}}}},
             "Domain":{{"Type":"AWS::OpenSearchService::Domain","DependsOn":"Alerts",
               "Properties":{{"DomainName":"{domain}","EngineVersion":"{engine}",
                 "ClusterConfig":{cluster},
                 "Tags":[{{"Key":"tier","Value":"search"}}]}}}}}},
           "Outputs":{{
             "DomainRef":{{"Value":{{"Ref":"Domain"}}}},
             "DomainArn":{{"Value":{{"Fn::GetAtt":["Domain","Arn"]}}}},
             "DomainId":{{"Value":{{"Fn::GetAtt":["Domain","Id"]}}}},
             "DomainEndpoint":{{"Value":{{"Fn::GetAtt":["Domain","DomainEndpoint"]}}}}}}}}"#,
        topic = row.topic,
        domain = row.domain,
        engine = row.engine_version,
        cluster = row.cluster,
    )
}

/// Every `ResourceStatusReason` the stack's events carry, which is where the owning
/// service's own refusal of one resource is reported.
pub(crate) async fn resource_reasons(
    router: &MultiServiceRouter,
    stack_name: &str,
) -> Vec<String> {
    let (_, events) =
        cloudformation(router, "DescribeStackEvents", &[("StackName", stack_name)]).await;
    elements(&events, "ResourceStatusReason")
}

#[tokio::test]
async fn the_published_catalog_agrees_about_every_row() {
    let router = build_router();
    for row in ROWS {
        let offered = offered_instance_types(&router, row.engine_version).await;
        let holds = offered.contains(&row.instance_type.to_string());
        assert_eq!(
            holds, row.offered,
            "case {:?}: the row's premise is that {} {} {}, but the published catalog says \
             otherwise: {offered:?}",
            row.case,
            row.engine_version,
            if row.offered { "offers" } else { "does not offer" },
            row.instance_type,
        );
    }
}

#[tokio::test]
async fn a_domain_the_owning_service_refuses_rolls_the_whole_stack_back() {
    let router = build_router();

    for row in ROWS.iter().filter(|row| !row.offered) {
        let case = row.case;
        let (status, body) = cloudformation(
            &router,
            "CreateStack",
            &[("StackName", row.stack), ("TemplateBody", &template(row))],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from CreateStack, got {status} with {body}"
        );

        let described = settle_stack(&router, case, row.stack).await;
        assert_eq!(
            xml_text(&described, "StackStatus"),
            "ROLLBACK_COMPLETE",
            "case {case:?}: a Domain that fails to create must roll the whole stack back, got \
             {described}"
        );
        // The STACK's reason summarises which resource failed; the service's own refusal is
        // reported on that resource's own event, with the class that refusal falls in.
        assert_eq!(
            xml_text(&described, "StackStatusReason"),
            resource_failure_summary(failed_action::CREATE, ["Domain"]),
            "case {case:?}: expected the stack reason to summarise what failed, got {described}"
        );
        let reasons = resource_reasons(&router, row.stack).await;
        // The refusal the resource reports is the one the owning service answers the same
        // cluster with when a client asks it directly — behind the handler's own opener and
        // in front of its SDK diagnostic, which names that service and the very status it
        // answered.
        let (direct_status, direct_message) = direct_refusal(&router, row).await;
        assert!(
            direct_message.contains(row.instance_type),
            "case {case:?}: expected the service's own refusal to name {}, got {direct_status} \
             with {direct_message:?}",
            row.instance_type
        );
        let expected = expected_refusal(direct_status, &direct_message);
        assert!(
            reasons
                .iter()
                .any(|reason| reason.contains(&expected) && reason.contains(INVALID_REQUEST)),
            "case {case:?}: expected a resource event carrying {expected:?} and \
             {INVALID_REQUEST:?}, got {reasons:?}"
        );
        // A refusal of the REQUEST is not retried, so the diagnostic reports the one attempt
        // the call was asked for.
        assert!(
            reasons
                .iter()
                .any(|reason| reason.contains(&expected) && reason.contains(SINGLE_ATTEMPT)),
            "case {case:?}: expected the diagnostic to report a single attempt \
             ({SINGLE_ATTEMPT:?}), got {reasons:?}"
        );

        // Nothing survives: the domain was never created, and the topic the domain
        // `DependsOn` — created first — is deleted again by the rollback.
        let (status, reported) = describe_domain(&router, row.domain).await;
        assert_eq!(
            status, MISSING_RESOURCE_STATUS,
            "case {case:?}: the refused domain must not exist, got {status} with {reported}"
        );
        assert_eq!(
            topic_arn(&router, case, row.topic).await,
            String::new(),
            "case {case:?}: the rollback must delete the topic the domain depended on"
        );
    }
}

#[tokio::test]
async fn a_domain_the_owning_service_offers_is_created_and_reads_back() {
    let router = build_router();

    for row in ROWS.iter().filter(|row| row.offered) {
        let case = row.case;
        let (status, body) = cloudformation(
            &router,
            "CreateStack",
            &[("StackName", row.stack), ("TemplateBody", &template(row))],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from CreateStack, got {status} with {body}"
        );

        let described = settle_stack(&router, case, row.stack).await;
        assert_eq!(
            xml_text(&described, "StackStatus"),
            "CREATE_COMPLETE",
            "case {case:?}: a pairing the service offers must deploy, got {described}"
        );

        // Round trip: the domain the stack reports is the one that service holds, on the
        // release and the type the template declared.
        let (status, reported) = describe_domain(&router, row.domain).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the created domain to read back, got {status} with \
             {reported}"
        );
        assert_eq!(
            reported.get("EngineVersion").and_then(Value::as_str),
            Some(row.engine_version),
            "case {case:?}: expected the stored domain to run the declared release, got \
             {reported}"
        );
        assert_eq!(
            reported
                .get("ClusterConfig")
                .and_then(|cluster| cluster.get("InstanceType"))
                .and_then(Value::as_str),
            Some(row.instance_type),
            "case {case:?}: expected the stored cluster to run the declared type, got {reported}"
        );
        assert_eq!(
            reported.get("Processing").and_then(Value::as_bool),
            Some(false),
            "case {case:?}: a stack reported complete must not still be provisioning its \
             domain, got {reported}"
        );

        // `Ref` returns the domain name, and each `Fn::GetAtt` the identifier the OWNING
        // service minted — never one CloudFormation's naming rules computed.
        let outputs: BTreeMap<String, String> = outputs_of(&described);
        for (attribute, member) in [
            ("DomainRef", "DomainName"),
            ("DomainArn", "ARN"),
            ("DomainId", "DomainId"),
            ("DomainEndpoint", "Endpoint"),
        ] {
            let minted = reported
                .get(member)
                .and_then(Value::as_str)
                .unwrap_or_default();
            assert!(
                !minted.is_empty(),
                "case {case:?}: the owning service must report {member}, got {reported}"
            );
            assert_eq!(
                outputs.get(attribute).map(String::as_str),
                Some(minted),
                "case {case:?}: expected {attribute} to be the service's own {member}, got \
                 {outputs:?}"
            );
        }
        assert_eq!(
            outputs.get("DomainRef").map(String::as_str),
            Some(row.domain),
            "case {case:?}: Ref returns the domain name, got {outputs:?}"
        );

        // The domain carries the tags the declaration asked for beside the ones
        // CloudFormation records on every resource it provisions, which only a create that
        // sent them as the owning service's own tag list could leave.
        let arn = outputs
            .get("DomainArn")
            .map(String::as_str)
            .unwrap_or_default();
        let tags = domain_tags(&router, case, arn).await;
        assert_eq!(
            tags.get("tier").map(String::as_str),
            Some("search"),
            "case {case:?}: expected the declared tag on the created domain, got {tags:?}"
        );
        assert_eq!(
            tags.get("aws:cloudformation:stack-name").map(String::as_str),
            Some(row.stack),
            "case {case:?}: expected CloudFormation's own tags on the created domain, got \
             {tags:?}"
        );

        // The topic the domain depended on is there too, and deleting the stack takes both
        // resources away again.
        assert_ne!(
            topic_arn(&router, case, row.topic).await,
            String::new(),
            "case {case:?}: the topic the domain depended on must exist"
        );
        let (status, body) =
            cloudformation(&router, "DeleteStack", &[("StackName", row.stack)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from DeleteStack, got {status} with {body}"
        );
        for _ in 0..POLL_LIMIT {
            tokio::time::sleep(POLL_INTERVAL).await;
            let (_, reported) = describe_domain(&router, row.domain).await;
            if reported.get("DomainName").is_none() {
                break;
            }
        }
        let (status, reported) = describe_domain(&router, row.domain).await;
        assert_eq!(
            status, MISSING_RESOURCE_STATUS,
            "case {case:?}: deleting the stack must delete the domain, got {status} with \
             {reported}"
        );
        let (status, answer) = sns(
            &router,
            "GetTopicAttributes",
            &[("TopicArn", &format!("arn:aws:sns:{REGION}:000000000000:{}", row.topic))],
        )
        .await;
        assert_eq!(
            (status, error_code(&answer).as_str()),
            (404, "NotFound"),
            "case {case:?}: deleting the stack must delete the topic, got {status} with {answer}"
        );
    }
}

/// The storage declarations the suite deploys: a volume the owning service does not publish
/// for the instance type the domain runs on, at each end of the published range, and the
/// sub-type whose maximum is a flat cap. Each row is a `VolumeSize` the owning service's own
/// `DescribeInstanceTypeLimits` reports as outside its `MinimumVolumeSize` ..
/// `MaximumVolumeSize` pair for that instance type, which the direct call below re-checks
/// per row rather than this suite restating the bounds.
const STORAGE_ROWS: &[(&str, &str, &str, &str, &str, &str)] = &[
    (
        "a gp3 volume below the published floor",
        "sim-osrb-ebs-low",
        "sim-osrb-ebs-low",
        "OpenSearch_2.11",
        r#"{"InstanceType":"t3.small.search","InstanceCount":1}"#,
        r#"{"EBSEnabled":true,"VolumeType":"gp3","VolumeSize":5}"#,
    ),
    (
        "a gp3 volume over the maximum the instance type publishes",
        "sim-osrb-ebs-high",
        "sim-osrb-ebs-high",
        "Elasticsearch_7.10",
        r#"{"InstanceType":"m5.large.search","InstanceCount":1}"#,
        r#"{"EBSEnabled":true,"VolumeType":"gp3","VolumeSize":4096}"#,
    ),
    (
        "a magnetic volume over the flat cap its sub-type publishes",
        "sim-osrb-ebs-mag",
        "sim-osrb-ebs-mag",
        "OpenSearch_2.19",
        r#"{"InstanceType":"m5.2xlarge.search","InstanceCount":1}"#,
        r#"{"EBSEnabled":true,"VolumeType":"standard","VolumeSize":400}"#,
    ),
];

/// What the owning service answers a DIRECT create of one row's cluster AND storage with:
/// the status and the message the stack's reason has to be built from.
async fn direct_storage_refusal(
    router: &MultiServiceRouter,
    domain: &str,
    engine_version: &str,
    cluster: &str,
    ebs: &str,
) -> (u16, String) {
    let (status, body) = opensearch(
        router,
        "POST",
        "/2021-01-01/opensearch/domain",
        Some(serde_json::json!({
            "DomainName": format!("direct-{domain}"),
            "EngineVersion": engine_version,
            "ClusterConfig": serde_json::from_str::<Value>(cluster).expect("a JSON object"),
            "EBSOptions": serde_json::from_str::<Value>(ebs).expect("a JSON object"),
        })),
    )
    .await;
    let message = body
        .get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    (status, message)
}

/// A domain declaring storage the owning service will not provision fails the resource under
/// the SERVICE-LIMIT class, not the invalid-request one.
///
/// This is the class half of the rule this suite is about: what a resource's reason reports
/// is decided by the error the owning service answered, so a refusal of a published LIMIT
/// reaches the reason as `ServiceLimitExceeded`, quoting that service's own sentence and the
/// 409 it answered, and carrying the attempts an SDK makes at a condition it retries — where
/// the instance-type rows above, refusals of the request itself, reach it as `InvalidRequest`
/// on a single attempt.
#[tokio::test]
async fn a_domain_whose_storage_exceeds_a_published_limit_reports_the_service_limit_class() {
    let router = build_router();

    for (case, stack, domain, engine_version, cluster, ebs) in STORAGE_ROWS {
        let topic = format!("{stack}-alerts");
        let body = format!(
            r#"{{"Resources":{{
                 "Alerts":{{"Type":"AWS::SNS::Topic","Properties":{{"TopicName":"{topic}"}}}},
                 "Domain":{{"Type":"AWS::OpenSearchService::Domain","DependsOn":"Alerts",
                   "Properties":{{"DomainName":"{domain}","EngineVersion":"{engine_version}",
                     "ClusterConfig":{cluster},"EBSOptions":{ebs}}}}}}}}}"#
        );
        let (status, answer) = cloudformation(
            &router,
            "CreateStack",
            &[("StackName", stack), ("TemplateBody", &body)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from CreateStack, got {status} with {answer}"
        );

        let described = settle_stack(&router, case, stack).await;
        assert_eq!(
            xml_text(&described, "StackStatus"),
            "ROLLBACK_COMPLETE",
            "case {case:?}: a Domain that fails to create must roll the whole stack back, got \
             {described}"
        );

        // The refusal the resource reports is the one the owning service answers the same
        // declaration with when a client asks it directly, and that service answers a
        // published-limit refusal with its modelled 409.
        let (direct_status, direct_message) =
            direct_storage_refusal(&router, domain, engine_version, cluster, ebs).await;
        assert_eq!(
            direct_status, 409,
            "case {case:?}: expected the owning service to refuse the storage with its \
             service-limit status, got {direct_status} with {direct_message:?}"
        );
        let reasons = resource_reasons(&router, stack).await;
        let quoted = format!(
            "{direct_message} (Service: OpenSearch, Status Code: {direct_status}, Request ID: "
        );
        assert!(
            reasons.iter().any(|reason| reason.contains(&quoted)
                && reason.contains(RETRIED_ATTEMPTS)
                && reason.contains(SERVICE_LIMIT_EXCEEDED)),
            "case {case:?}: expected a resource event carrying {quoted:?}, \
             {RETRIED_ATTEMPTS:?} and {SERVICE_LIMIT_EXCEEDED:?}, got {reasons:?}"
        );
        // And NOT under the invalid-request class, whose own wording the provider puts
        // inside the quoted message.
        assert!(
            !reasons
                .iter()
                .any(|reason| reason.contains(INVALID_REQUEST)
                    || reason.contains("Invalid request provided:")),
            "case {case:?}: expected no invalid-request classification of a limit refusal, \
             got {reasons:?}"
        );

        // Nothing survives: the domain was never created, and the topic it depended on is
        // deleted again by the rollback.
        let (status, reported) = describe_domain(&router, domain).await;
        assert_eq!(
            status, MISSING_RESOURCE_STATUS,
            "case {case:?}: the refused domain must not exist, got {status} with {reported}"
        );
        assert_eq!(
            topic_arn(&router, case, &topic).await,
            String::new(),
            "case {case:?}: the rollback must delete the topic the domain depended on"
        );
    }
}

// ---------------------------------------------------------------------------
// What a created domain's published attributes carry: the addresses a client
// reaches it on, read back out of the stack's Outputs
// ---------------------------------------------------------------------------

/// The resource type the rows below declare.
const OPENSEARCH_DOMAIN_TYPE: &str = "AWS::OpenSearchService::Domain";

/// The attribute `AWS::OpenSearchService::Domain` publishes the hosts of a domain reached
/// inside a VPC under, from that type's "Return values" section: a MAP, keyed as the owning
/// service keys `DomainStatus$Endpoints`.
const DOMAIN_ENDPOINTS: &str = "DomainEndpoints";

/// The engine release and cluster every endpoint row declares: the pairing
/// [`the_published_catalog_agrees_about_every_row`] already asserts the owning service
/// offers, so no row here can fail on the cluster instead of on the addresses it is about.
const ENDPOINT_ENGINE: &str = "OpenSearch_2.19";
const ENDPOINT_CLUSTER: &str = r#"{"InstanceType":"or1.medium.search","InstanceCount":2}"#;

/// The `VPCOptions` a row declares to place the domain inside a VPC — the property whose
/// PRESENCE is what makes a domain VPC-access, so the ids themselves are only well-formed
/// Amazon EC2 ones.
const VPC_OPTIONS: &str = r#""VPCOptions":{"SubnetIds":["subnet-0fff0000000000001"],"SecurityGroupIds":["sg-0fff0000000000001"]}"#;

/// Where a row's domain is reachable, which is what decides the addresses the owning service
/// publishes for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Reach {
    /// No `VPCOptions`: the domain has a public endpoint and no endpoint map.
    Public,
    /// Inside a VPC, over IPv4 only.
    Vpc,
    /// Inside a VPC, with `IPAddressType` `dualstack`, which gives it a second host.
    VpcDualStack,
}

/// Both keys the owning service publishes a VPC-access domain's hosts under, from its own
/// constants rather than spelled again here.
fn vpc_keys() -> [&'static str; 2] {
    [ENDPOINTS_KEY_VPC, ENDPOINTS_KEY_VPC_DUALSTACK]
}

/// The keys a domain of this reachability is published under.
fn published_keys(reach: Reach) -> Vec<&'static str> {
    match reach {
        Reach::Public => Vec::new(),
        Reach::Vpc => vec![ENDPOINTS_KEY_VPC],
        Reach::VpcDualStack => vec![ENDPOINTS_KEY_VPC, ENDPOINTS_KEY_VPC_DUALSTACK],
    }
}

/// The output key one entry of the endpoint map is published under.
fn output_key(endpoints_key: &str) -> String {
    format!("Host{endpoints_key}")
}

/// One row: how the template declares the domain's reachability, and how its outputs spell
/// the path into the endpoint map.
struct EndpointRow {
    case: &'static str,
    stack: &'static str,
    /// Inside `DomainName`'s own `^[a-z][a-z0-9\-]+$` and 3..28 characters.
    domain: &'static str,
    reach: Reach,
    /// Whether the outputs name the map entry as ONE dotted attribute
    /// (`["Domain","DomainEndpoints.vpc"]`) or as the attribute and its key in separate
    /// `Fn::GetAtt` arguments — the two spellings of the same attribute path.
    dotted_path: bool,
}

/// The rows differ along what the rule generalises over: the reachability the declaration
/// asks for (and so WHICH addresses that service publishes, from none through one to two),
/// and the spelling the output reaches a map entry through. Every row declares an output for
/// every key, so each also asserts the other half of the rule — an entry the service
/// publishes NO host under resolves to nothing.
fn endpoint_rows() -> Vec<EndpointRow> {
    vec![
        EndpointRow {
            case: "a domain on its public endpoint",
            stack: "sim-osep-public",
            domain: "sim-osep-public",
            reach: Reach::Public,
            dotted_path: true,
        },
        EndpointRow {
            case: "a domain reached inside a VPC over IPv4",
            stack: "sim-osep-vpc",
            domain: "sim-osep-vpc",
            reach: Reach::Vpc,
            dotted_path: true,
        },
        EndpointRow {
            case: "a dual-stack domain reached inside a VPC, read through separate arguments",
            stack: "sim-osep-vpc-dual",
            domain: "sim-osep-vpc-dual",
            reach: Reach::VpcDualStack,
            dotted_path: false,
        },
    ]
}

/// The template one endpoint row deploys: the domain, and an output for `Ref`, for the
/// public endpoint and for each key of the endpoint map.
fn endpoint_template(row: &EndpointRow) -> String {
    let mut properties = format!(
        r#""DomainName":"{domain}","EngineVersion":"{ENDPOINT_ENGINE}","ClusterConfig":{ENDPOINT_CLUSTER}"#,
        domain = row.domain
    );
    if row.reach != Reach::Public {
        properties.push(',');
        properties.push_str(VPC_OPTIONS);
    }
    if row.reach == Reach::VpcDualStack {
        properties.push_str(&format!(
            r#","IPAddressType":"{IP_ADDRESS_TYPE_DUALSTACK}""#
        ));
    }
    let mut outputs = String::from(
        r#""DomainRef":{"Value":{"Ref":"Domain"}},"DomainEndpoint":{"Value":{"Fn::GetAtt":["Domain","DomainEndpoint"]}}"#,
    );
    for key in vpc_keys() {
        let path = if row.dotted_path {
            format!(r#""{DOMAIN_ENDPOINTS}.{key}""#)
        } else {
            format!(r#""{DOMAIN_ENDPOINTS}","{key}""#)
        };
        outputs.push_str(&format!(
            r#","{published}":{{"Value":{{"Fn::GetAtt":["Domain",{path}]}}}}"#,
            published = output_key(key)
        ));
    }
    format!(
        r#"{{"Resources":{{"Domain":{{"Type":"{OPENSEARCH_DOMAIN_TYPE}","Properties":{{{properties}}}}}}},"Outputs":{{{outputs}}}}}"#
    )
}

/// The `DomainStatus$Endpoints` the owning service reports for a domain, as the map a client
/// reads it as.
fn reported_endpoints(reported: &Value) -> BTreeMap<String, String> {
    reported
        .get("Endpoints")
        .and_then(Value::as_object)
        .map(|map| {
            map.iter()
                .filter_map(|(key, host)| Some((key.clone(), host.as_str()?.to_string())))
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test]
async fn a_domains_published_addresses_resolve_in_its_stacks_outputs() {
    let router = build_router();

    for row in endpoint_rows() {
        let case = row.case;
        let (status, body) = cloudformation(
            &router,
            "CreateStack",
            &[
                ("StackName", row.stack),
                ("TemplateBody", &endpoint_template(&row)),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from CreateStack, got {status} with {body}"
        );

        let described = settle_stack(&router, case, row.stack).await;
        assert_eq!(
            xml_text(&described, "StackStatus"),
            "CREATE_COMPLETE",
            "case {case:?}: the declared domain must deploy, got {described}"
        );
        let outputs: BTreeMap<String, String> = outputs_of(&described);

        // What the OWNING service publishes for the domain, which every expected value
        // below is read from rather than spelled here.
        let (status, reported) = describe_domain(&router, row.domain).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the created domain to read back, got {status} with \
             {reported}"
        );
        let published = reported_endpoints(&reported);
        let expected_keys: Vec<String> = published_keys(row.reach)
            .into_iter()
            .map(str::to_string)
            .collect();
        assert_eq!(
            published.keys().cloned().collect::<Vec<String>>(),
            expected_keys,
            "case {case:?}: the row's premise is that the service publishes {expected_keys:?}, \
             got {published:?}"
        );

        // Each entry of the published map is readable through its own dotted path, and an
        // entry the service publishes nothing under resolves to nothing.
        for key in vpc_keys() {
            let name = output_key(key);
            let reported_output = outputs.get(&name).cloned();
            match published.get(key) {
                Some(host) => {
                    assert!(
                        !host.is_empty(),
                        "case {case:?}: the service must publish a host under {key:?}, got \
                         {published:?}"
                    );
                    assert_eq!(
                        reported_output.as_deref(),
                        Some(host.as_str()),
                        "case {case:?}: expected the output {name:?} to be the service's own \
                         {key:?} host {host:?}, got {reported_output:?}"
                    );
                }
                None => assert_eq!(
                    reported_output, None,
                    "case {case:?}: the service publishes no {key:?} host, so {name:?} must \
                     carry no OutputValue, got {reported_output:?}"
                ),
            }
        }
        // The two hosts of a dual-stack domain are different addresses, so neither output
        // can be the other's.
        if row.reach == Reach::VpcDualStack {
            assert_ne!(
                published.get(ENDPOINTS_KEY_VPC),
                published.get(ENDPOINTS_KEY_VPC_DUALSTACK),
                "case {case:?}: expected two distinct hosts, got {published:?}"
            );
        }

        // The public endpoint half: present for the domain that has one, absent — rather
        // than guessed — for a domain the service reports none for.
        let public = reported.get("Endpoint").and_then(Value::as_str);
        assert_eq!(
            public.is_some(),
            row.reach == Reach::Public,
            "case {case:?}: expected a public endpoint only for a domain outside a VPC, got \
             {reported}"
        );
        // `DomainEndpoint` is the address a client SUBMITS its requests to, so it is the
        // address the owning service published for the domain however the domain is reached:
        // the public host where it has one, and the host it is reached on inside a VPC — the
        // `vpc` entry of the map above — where it has no public host at all.
        let addressed_at = public
            .map(str::to_string)
            .or_else(|| published.get(ENDPOINTS_KEY_VPC).cloned());
        assert_eq!(
            outputs.get("DomainEndpoint").cloned(),
            addressed_at,
            "case {case:?}: expected DomainEndpoint to be the address the owning service \
             publishes for the domain ({addressed_at:?}), got {outputs:?}"
        );

        // "Ref returns the domain name", whatever the domain is reached on.
        assert_eq!(
            outputs.get("DomainRef").map(String::as_str),
            Some(row.domain),
            "case {case:?}: Ref returns the domain name, got {outputs:?}"
        );

        // And the symptom itself, over the whole answer: the stack publishes an address for
        // every output whose expression names one the service reported — the entries of the
        // endpoint map, and the endpoint attribute, which every deployed domain is addressed
        // at one way or the other.
        let addressable: Vec<String> = published_keys(row.reach)
            .into_iter()
            .map(output_key)
            .chain(addressed_at.map(|_| "DomainEndpoint".to_string()))
            .collect();
        for name in &addressable {
            assert!(
                outputs.get(name).is_some_and(|value| !value.is_empty()),
                "case {case:?}: every Output naming an address the service published must \
                 carry it; {name:?} carries {:?}",
                outputs.get(name)
            );
        }
    }
}
