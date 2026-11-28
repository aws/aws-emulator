// SPDX-License-Identifier: CC0-1.0

//! `ListResources` end to end: the resources a Region's index holds, what a filter
//! prefix and a view's own filter string select out of them, how the match pages, and
//! the refusals a query answers.
//!
//! The rule under test: a Resource Explorer query reports the resources the index of the
//! view's Region holds — read from the records of the services that OWN them, so an
//! index is a live view over those services rather than a second copy of their
//! resources — each one projected with the ARN, account, Region, service,
//! `service:resourceType` key, CloudFormation type and last-reported instant that owner
//! holds for it; the result is the intersection of the caller's filter string with the
//! view's own; the filter string carries only filter prefixes, never free-form text; and
//! a query that cannot resolve a view is refused with 401 `UnauthorizedException`.
//!
//! The tables span what the rule generalises over: WHICH service owns the resource (an
//! SQS queue, a FIFO queue, an SNS topic, this service's own index and views), the
//! filter NAME (`service`, `resourcetype`, `region`, `arn`, `id`, `accountid`, `tag`),
//! the operator (negation, a `*` suffix wildcard), the Region the resource lives in, the
//! page size, and the form of a term that is not a filter prefix. The recorded case — a
//! `resourcetype:sqs:queue` view returning the queue and never the topic — is one row.
//!
//! Every call goes over the wire through the router the emulator serves in production,
//! with the owning services mounted on the SAME engine: an SQS queue is created by
//! `AmazonSQS.CreateQueue` and an SNS topic by the `CreateTopic` query action, so
//! nothing but the owning service's own persisted state puts a resource in the index.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use bytes::Bytes;
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_resourceexplorer2_resourceexplorer as explorer;
use service_com_amazonaws_sns_amazonsimplenotificationservice as owner_sns;
use service_com_amazonaws_sqs_amazonsqs as owner_sqs;

/// The Region the fixture's index, views and most resources live in.
const REGION: &str = "us-east-1";

/// A second Region with an index and a view of its own: what proves a Region's index
/// holds only "the resources that are located in the same Region as the index".
const OTHER_REGION: &str = "eu-west-1";

/// The account segment every emulated ARN carries.
const ACCOUNT_ID: &str = engine::EMULATOR_ACCOUNT_ID;

/// The one property name `IncludedProperties` documents.
const TAGS_PROPERTY: &str = "tags";

/// What one Resource Explorer call answered.
struct Answer {
    status: u16,
    error: Option<String>,
    body: Value,
}

impl Answer {
    /// The body of a call that had to succeed.
    fn ok(&self, case: &str) -> &Value {
        assert_eq!(
            self.status, 200,
            "case {case:?}: expected HTTP 200, got {} ({:?}) with body {}",
            self.status, self.error, self.body
        );
        &self.body
    }

    /// Assert this call failed with `status` and the error code `code`.
    fn failed(&self, case: &str, status: u16, code: &str) {
        assert_eq!(
            (self.status, self.error.as_deref()),
            (status, Some(code)),
            "case {case:?}: expected HTTP {status} {code}, got {} ({:?}) with body {}",
            self.status,
            self.error,
            self.body
        );
    }

    /// The error's `Message` member, however the envelope cases it.
    fn error_message(&self) -> String {
        self.body
            .get("Message")
            .or_else(|| self.body.get("message"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    /// The `Resources` list of a successful answer.
    fn resources(&self, case: &str) -> Vec<Value> {
        let resources = self.ok(case).get("Resources").cloned().unwrap_or(Value::Null);
        resources
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected a Resources list, got {resources}"))
            .clone()
    }

    /// The ARNs of a successful answer, as a set — the order a page reports them in is
    /// not what any of these rows is about.
    fn arns(&self, case: &str) -> BTreeSet<String> {
        self.resources(case)
            .iter()
            .map(|resource| {
                resource["Arn"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {case:?}: a Resource with no Arn: {resource}"))
                    .to_string()
            })
            .collect()
    }
}

/// One emulator: Resource Explorer and the services that own the resources it indexes,
/// mounted on ONE shared engine — the arrangement `core/router` builds.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // These suites exercise handler behaviour, not SigV4: the `Authorization`
        // header below is present because it is what names the Region a call is made
        // in, not to be verified.
        router.set_request_authentication(false);
        router.register_service(explorer::EmulatedService::new(Arc::clone(&engine)));
        router.register_service(owner_sqs::EmulatedService::new(Arc::clone(&engine)));
        router.register_service(owner_sns::EmulatedService::new(engine));
        Self { router }
    }

    /// The SigV4 `Authorization` header of a call made in `region` for `signing_name`.
    fn authorization(region: &str, signing_name: &str) -> String {
        format!(
            "AWS4-HMAC-SHA256 Credential=AKIDTESTTESTTESTTEST/20260101/{region}/{signing_name}\
             /aws4_request, SignedHeaders=host, Signature=00"
        )
    }

    /// `POST /{operation}` with a JSON body — how this service's RPC-shaped routes are
    /// addressed.
    async fn explorer(&self, region: &str, operation: &str, body: Value) -> Answer {
        let request = http::Request::builder()
            .method("POST")
            .uri(format!(
                "https://resource-explorer-2.{region}.amazonaws.com/{operation}"
            ))
            .header("content-type", "application/json")
            .header(
                "authorization",
                Self::authorization(region, "resource-explorer-2"),
            )
            .body(Bytes::from(
                serde_json::to_vec(&body).expect("the request body serializes"),
            ))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        Answer {
            status,
            error,
            body,
        }
    }

    /// One Amazon SQS operation, which must answer 200: `POST /` with an
    /// `X-Amz-Target` and an awsJson body.
    async fn sqs(&self, region: &str, operation: &str, body: Value) -> Value {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", format!("sqs.{region}.amazonaws.com"))
            .header("content-type", "application/x-amz-json-1.0")
            .header("x-amz-target", format!("AmazonSQS.{operation}"))
            .header("authorization", Self::authorization(region, "sqs"))
            .body(Bytes::from(body.to_string()))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
        assert_eq!(
            status, 200,
            "SQS {operation} must answer 200, got {status} with body {parsed}"
        );
        parsed
    }

    /// One Amazon SNS query action, which must answer 200, with its XML body.
    async fn sns(&self, region: &str, action: &str, params: &[(&str, &str)]) -> String {
        let mut body = format!("Action={action}&Version=2010-03-31");
        for (name, value) in params {
            body.push('&');
            body.push_str(name);
            body.push('=');
            body.push_str(&utf8_percent_encode(value, NON_ALPHANUMERIC).to_string());
        }
        let request = http::Request::builder()
            .method("POST")
            .uri("https://sns.amazonaws.com/")
            .header("host", format!("sns.{region}.amazonaws.com"))
            .header("content-type", "application/x-www-form-urlencoded")
            .header("authorization", Self::authorization(region, "sns"))
            .body(Bytes::from(body))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let text = String::from_utf8_lossy(response.body()).to_string();
        assert_eq!(
            status, 200,
            "SNS {action} must answer 200, got {status} with body {text}"
        );
        text
    }

    /// Create an SQS queue through the owning service and read back the ARN it minted,
    /// with the queue's URL.
    async fn create_queue(
        &self,
        region: &str,
        name: &str,
        attributes: Value,
        tags: Value,
    ) -> (String, String) {
        let mut request = json!({ "QueueName": name });
        if attributes != Value::Null {
            request["Attributes"] = attributes;
        }
        if tags != Value::Null {
            request["tags"] = tags;
        }
        let created = self.sqs(region, "CreateQueue", request).await;
        let url = created["QueueUrl"]
            .as_str()
            .unwrap_or_else(|| panic!("CreateQueue {name}: no QueueUrl in {created}"))
            .to_string();
        let read = self
            .sqs(
                region,
                "GetQueueAttributes",
                json!({ "QueueUrl": &url, "AttributeNames": ["QueueArn"] }),
            )
            .await;
        let arn = read["Attributes"]["QueueArn"]
            .as_str()
            .unwrap_or_else(|| panic!("GetQueueAttributes {name}: no QueueArn in {read}"))
            .to_string();
        (url, arn)
    }

    /// Create an SNS topic through the owning service and read back the ARN it minted.
    async fn create_topic(&self, region: &str, name: &str) -> String {
        let body = self.sns(region, "CreateTopic", &[("Name", name)]).await;
        xml_element(&body, "TopicArn")
            .unwrap_or_else(|| panic!("CreateTopic {name}: no TopicArn in {body}"))
    }

    /// `CreateIndex` in `region`, with the index ARN it answered with.
    async fn create_index(&self, region: &str) -> String {
        let answer = self
            .explorer(region, "CreateIndex", json!({ "ClientToken": region }))
            .await;
        answer.ok(&format!("CreateIndex in {region}"))["Arn"]
            .as_str()
            .expect("CreateIndex answers an Arn")
            .to_string()
    }

    /// `CreateView` in `region`, with the view ARN it answered with.
    async fn create_view(&self, region: &str, body: Value) -> String {
        let case = format!("CreateView in {region}: {body}");
        let answer = self.explorer(region, "CreateView", body).await;
        answer.ok(&case)["View"]["ViewArn"]
            .as_str()
            .expect("CreateView answers a View.ViewArn")
            .to_string()
    }

    /// `AssociateDefaultView`, which must succeed.
    async fn set_default_view(&self, region: &str, view_arn: &str) {
        self.explorer(
            region,
            "AssociateDefaultView",
            json!({ "ViewArn": view_arn }),
        )
        .await
        .ok(&format!("AssociateDefaultView in {region}"));
    }

    /// The ARN `GetDefaultView` reports for `region`, which must have one.
    async fn reported_default_view(&self, region: &str) -> String {
        let answer = self.explorer(region, "GetDefaultView", json!({})).await;
        answer.ok(&format!("GetDefaultView in {region}"))["ViewArn"]
            .as_str()
            .expect("a Region that was turned on has a default view")
            .to_string()
    }

    /// `ListResources` with the Region's default view and one filter string.
    async fn list(&self, region: &str, filter_string: &str) -> Answer {
        self.explorer(
            region,
            "ListResources",
            json!({ "Filters": { "FilterString": filter_string } }),
        )
        .await
    }

    /// `ListResources` through one named view and one filter string.
    async fn list_through(&self, region: &str, view_arn: &str, filter_string: &str) -> Answer {
        self.explorer(
            region,
            "ListResources",
            json!({ "ViewArn": view_arn, "Filters": { "FilterString": filter_string } }),
        )
        .await
    }
}

/// The text of the first `<tag>…</tag>` element of an awsQuery response.
fn xml_element(body: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = body.find(&open)? + open.len();
    let rest = &body[start..];
    let end = rest.find(&close)?;
    Some(rest[..end].to_string())
}

/// A set of ARNs, written as the rows write them.
fn set(arns: &[&String]) -> BTreeSet<String> {
    arns.iter().map(|arn| (*arn).clone()).collect()
}

/// One resource of the fixture, with everything the index must report about it.
struct Indexed {
    case: &'static str,
    arn: String,
    /// `Resource$ResourceType`: the `service:resourceType` key.
    resource_type: &'static str,
    /// `Resource$Service`.
    service: &'static str,
    /// `Resource$CfnResourceType`.
    cfn_resource_type: &'static str,
}

/// The fixture: one index and one default view per Region, plus resources of every
/// owning service the index holds.
struct Fixture {
    emulator: Emulator,
    index: String,
    /// The view turning Resource Explorer on in the Region left there, which is one of
    /// the resources that Region's index holds.
    turn_on_view: String,
    /// The view the fixture created and made the Region's default: it includes the
    /// `tags` property, so the rows that filter on tags can be served.
    default_view: String,
    /// A standard queue carrying tags.
    queue: String,
    queue_url: String,
    /// A FIFO queue, whose name carries the `.fifo` suffix: the same resource type under
    /// a different name shape.
    fifo_queue: String,
    /// An SNS topic indexed in the same index as the queues.
    topic: String,
    /// A queue of the OTHER Region, which this Region's index must never report.
    foreign_queue: String,
    /// The other Region's index and views.
    foreign_index: String,
    foreign_turn_on_view: String,
    foreign_view: String,
}

impl Fixture {
    async fn new() -> Self {
        let emulator = Emulator::new();
        let index = emulator.create_index(REGION).await;
        let turn_on_view = emulator.reported_default_view(REGION).await;
        let foreign_index = emulator.create_index(OTHER_REGION).await;
        let foreign_turn_on_view = emulator.reported_default_view(OTHER_REGION).await;
        let default_view = emulator
            .create_view(
                REGION,
                json!({
                    "ViewName": "everything",
                    "IncludedProperties": [{ "Name": TAGS_PROPERTY }],
                }),
            )
            .await;
        emulator.set_default_view(REGION, &default_view).await;
        let foreign_view = emulator
            .create_view(OTHER_REGION, json!({ "ViewName": "everything-abroad" }))
            .await;
        emulator
            .set_default_view(OTHER_REGION, &foreign_view)
            .await;

        let (queue_url, queue) = emulator
            .create_queue(
                REGION,
                "orders",
                Value::Null,
                json!({ "stage": "prod", "team": "payments" }),
            )
            .await;
        let (_, fifo_queue) = emulator
            .create_queue(
                REGION,
                "payments.fifo",
                json!({ "FifoQueue": "true" }),
                Value::Null,
            )
            .await;
        let topic = emulator.create_topic(REGION, "alerts").await;
        let (_, foreign_queue) = emulator
            .create_queue(OTHER_REGION, "orders-abroad", Value::Null, Value::Null)
            .await;

        Self {
            emulator,
            index,
            turn_on_view,
            foreign_turn_on_view,
            default_view,
            queue,
            queue_url,
            fifo_queue,
            topic,
            foreign_queue,
            foreign_index,
            foreign_view,
        }
    }

    /// Every resource the fixture's Region indexes, with the projection the index must
    /// report for it.
    ///
    /// The `service:resourceType` keys and CloudFormation type names are those of the
    /// Resource Explorer supported-resource-type list and the CloudFormation registry,
    /// and are the ones `crate::state::owners` records.
    fn indexed(&self) -> Vec<Indexed> {
        vec![
            Indexed {
                case: "a standard SQS queue",
                arn: self.queue.clone(),
                resource_type: "sqs:queue",
                service: "sqs",
                cfn_resource_type: "AWS::SQS::Queue",
            },
            Indexed {
                case: "a FIFO SQS queue",
                arn: self.fifo_queue.clone(),
                resource_type: "sqs:queue",
                service: "sqs",
                cfn_resource_type: "AWS::SQS::Queue",
            },
            Indexed {
                case: "an SNS topic",
                arn: self.topic.clone(),
                resource_type: "sns:topic",
                service: "sns",
                cfn_resource_type: "AWS::SNS::Topic",
            },
            Indexed {
                case: "the Region's own index",
                arn: self.index.clone(),
                resource_type: "resource-explorer-2:index",
                service: "resource-explorer-2",
                cfn_resource_type: "AWS::ResourceExplorer2::Index",
            },
            Indexed {
                case: "the Region's own view",
                arn: self.default_view.clone(),
                resource_type: "resource-explorer-2:view",
                service: "resource-explorer-2",
                cfn_resource_type: "AWS::ResourceExplorer2::View",
            },
            Indexed {
                case: "the view the Region was turned on with",
                arn: self.turn_on_view.clone(),
                resource_type: "resource-explorer-2:view",
                service: "resource-explorer-2",
                cfn_resource_type: "AWS::ResourceExplorer2::View",
            },
        ]
    }

    /// Every ARN the fixture's Region indexes.
    fn every_arn(&self) -> BTreeSet<String> {
        self.indexed()
            .into_iter()
            .map(|resource| resource.arn)
            .collect()
    }
}

/// The index reports every resource of the Region, whichever service owns it, projected
/// from that owner's own record: the ARN it minted, the account and Region that ARN
/// carries, the `service:resourceType` key and CloudFormation type of its kind, and a
/// `LastReportedAt` instant. A resource of ANOTHER Region is never in it.
#[tokio::test]
async fn the_index_reports_every_owning_services_resources_in_its_region() {
    let fixture = Fixture::new().await;
    let answer = fixture.emulator.list(REGION, "").await;
    let case = "the default view, no filter";
    assert_eq!(
        answer.ok(case)["ViewArn"].as_str(),
        Some(fixture.default_view.as_str()),
        "case {case:?}: the answer names the view it used, got {}",
        answer.body
    );
    assert_eq!(
        answer.arns(case),
        fixture.every_arn(),
        "case {case:?}: expected every resource of {REGION}, got {}",
        answer.body
    );

    let reported = answer.resources(case);
    for resource in fixture.indexed() {
        let found = reported
            .iter()
            .find(|candidate| candidate["Arn"] == json!(resource.arn))
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: {} is not in the answer: {}",
                    resource.case, resource.arn, answer.body
                )
            });
        for (member, want) in [
            ("ResourceType", resource.resource_type),
            ("Service", resource.service),
            ("CfnResourceType", resource.cfn_resource_type),
            ("Region", REGION),
            ("OwningAccountId", ACCOUNT_ID),
        ] {
            assert_eq!(
                found[member],
                json!(want),
                "case {:?}: expected {member} {want:?}, got {found}",
                resource.case
            );
        }
        assert!(
            is_date_time(&found["LastReportedAt"]),
            "case {:?}: expected a LastReportedAt date-time string, got {found}",
            resource.case
        );
    }

    // The other Region's index holds its own resources and nothing of this one's.
    let case = "the other Region's default view";
    let abroad = fixture.emulator.list(OTHER_REGION, "").await;
    assert_eq!(
        abroad.arns(case),
        set(&[
            &fixture.foreign_queue,
            &fixture.foreign_index,
            &fixture.foreign_turn_on_view,
            &fixture.foreign_view
        ]),
        "case {case:?}: expected only {OTHER_REGION} resources, got {}",
        abroad.body
    );
}

/// A filter prefix selects out of the index by the field it names, with `-` negating a
/// term, `*` matching by prefix and several terms AND-ed. The rows differ along the
/// filter NAME, the operator and the number of resources selected.
#[tokio::test]
async fn filter_prefixes_select_resources_out_of_the_index() {
    let fixture = Fixture::new().await;
    let queue_id = "orders";

    let cases: Vec<(&str, String, BTreeSet<String>)> = vec![
        (
            "one service",
            "service:sqs".to_string(),
            set(&[&fixture.queue, &fixture.fifo_queue]),
        ),
        (
            "the other service",
            "service:sns".to_string(),
            set(&[&fixture.topic]),
        ),
        (
            "a qualified resource type",
            "resourcetype:sqs:queue".to_string(),
            set(&[&fixture.queue, &fixture.fifo_queue]),
        ),
        (
            "the other qualified resource type",
            "resourcetype:sns:topic".to_string(),
            set(&[&fixture.topic]),
        ),
        (
            "this service's own views",
            "resourcetype:resource-explorer-2:view".to_string(),
            set(&[&fixture.default_view, &fixture.turn_on_view]),
        ),
        (
            "the serving Region",
            format!("region:{REGION}"),
            fixture.every_arn(),
        ),
        (
            "a Region prefix wildcard",
            "region:us*".to_string(),
            fixture.every_arn(),
        ),
        (
            "a Region prefix nothing is in",
            "region:ap*".to_string(),
            BTreeSet::new(),
        ),
        (
            "one ARN",
            format!("arn:{}", fixture.queue),
            set(&[&fixture.queue]),
        ),
        (
            "one resource id",
            format!("id:{queue_id}"),
            set(&[&fixture.queue]),
        ),
        (
            "the owning account",
            format!("accountid:{ACCOUNT_ID}"),
            fixture.every_arn(),
        ),
        (
            "another account",
            "accountid:999999999999".to_string(),
            BTreeSet::new(),
        ),
        (
            "a negated service",
            "resourcetype:sqs:queue -id:orders".to_string(),
            set(&[&fixture.fifo_queue]),
        ),
        (
            "two filters AND-ed to nothing",
            "service:sqs service:sns".to_string(),
            BTreeSet::new(),
        ),
        (
            "a tag pair one queue carries",
            "tag:stage=prod".to_string(),
            set(&[&fixture.queue]),
        ),
        (
            "a tag key no resource carries",
            "tag.key:absent".to_string(),
            BTreeSet::new(),
        ),
        (
            "a global resource type, which no regional resource is",
            "globalresourcetype:sqs:queue".to_string(),
            BTreeSet::new(),
        ),
    ];

    for (case, filter_string, want) in cases {
        let answer = fixture.emulator.list(REGION, &filter_string).await;
        assert_eq!(
            answer.arns(case),
            want,
            "case {case:?}: expected {filter_string:?} to select {want:?}, got {}",
            answer.body
        );
    }
}

/// A view's own filter string SCOPES what the view can return, and the result is the
/// intersection of it with the caller's filter string: a view cut to one resource type
/// never reports a resource of another type indexed in the same index, and every
/// resource it does report carries that type's own `ResourceType` and `Service`.
///
/// The recorded case is the first row.
#[tokio::test]
async fn a_views_filter_string_scopes_what_it_can_return() {
    let fixture = Fixture::new().await;

    /// What a view reports with no filter string of the caller's own.
    enum Want {
        /// Exactly these ARNs.
        Exactly(BTreeSet<String>),
        /// Everything the Region indexes WHEN THE ROW RUNS, except these ARNs: each row
        /// creates a view, and a view is itself a resource of the index, so a row whose
        /// filter admits this service's own resources cannot name them up front.
        EverythingExcept(BTreeSet<String>),
    }

    struct Row {
        case: &'static str,
        view_name: &'static str,
        view_filter: &'static str,
        want: Want,
        /// The `ResourceType` and `Service` every resource it reports must carry, when
        /// the view is cut to exactly one type.
        want_type: Option<(&'static str, &'static str)>,
        /// Whether this view's own filter string admits the Region's index, which is
        /// what the caller's filter below is narrowed to.
        admits_index: bool,
    }

    let rows = vec![
        Row {
            case: "a view cut to one resource type",
            view_name: "queues-only",
            view_filter: "resourcetype:sqs:queue",
            want: Want::Exactly(set(&[&fixture.queue, &fixture.fifo_queue])),
            want_type: Some(("sqs:queue", "sqs")),
            admits_index: false,
        },
        Row {
            case: "a view cut to another service",
            view_name: "topics-only",
            view_filter: "service:sns",
            want: Want::Exactly(set(&[&fixture.topic])),
            want_type: Some(("sns:topic", "sns")),
            admits_index: false,
        },
        Row {
            case: "a view cut by a negated term",
            view_name: "nothing-of-sqs",
            view_filter: "-service:sqs",
            want: Want::EverythingExcept(set(&[&fixture.queue, &fixture.fifo_queue])),
            want_type: None,
            admits_index: true,
        },
        Row {
            case: "a view cut to one ARN",
            view_name: "one-topic",
            view_filter: "arn:PLACEHOLDER",
            want: Want::Exactly(set(&[&fixture.topic])),
            want_type: Some(("sns:topic", "sns")),
            admits_index: false,
        },
        Row {
            case: "a view cut to a Region nothing is in",
            view_name: "elsewhere",
            view_filter: "region:ap-southeast-2",
            want: Want::Exactly(BTreeSet::new()),
            want_type: None,
            admits_index: false,
        },
    ];

    for row in rows {
        let filter = row
            .view_filter
            .replace("PLACEHOLDER", fixture.topic.as_str());
        let view = fixture
            .emulator
            .create_view(
                REGION,
                json!({
                    "ViewName": row.view_name,
                    "Filters": { "FilterString": filter },
                }),
            )
            .await;

        let answer = fixture.emulator.list_through(REGION, &view, "").await;
        assert_eq!(
            answer.ok(row.case)["ViewArn"].as_str(),
            Some(view.as_str()),
            "case {:?}: the answer names the view it used, got {}",
            row.case,
            answer.body
        );
        let got = answer.arns(row.case);
        let want = match &row.want {
            Want::Exactly(arns) => arns.clone(),
            // What the Region indexes right now, which the views the earlier rows
            // created are part of, less the ARNs this row's filter excludes.
            Want::EverythingExcept(excluded) => {
                let case = format!("{}: everything the Region indexes", row.case);
                let every = fixture.emulator.list(REGION, "").await;
                every
                    .arns(&case)
                    .into_iter()
                    .filter(|arn| !excluded.contains(arn))
                    .collect()
            }
        };
        assert_eq!(
            got, want,
            "case {:?}: expected the view {filter:?} to return {want:?}, got {}",
            row.case, answer.body
        );
        if let Some((resource_type, service)) = row.want_type {
            assert!(
                !got.is_empty(),
                "case {:?}: the row is about the type of what it returns, so it must \
                 return something",
                row.case
            );
            for resource in answer.resources(row.case) {
                assert_eq!(
                    (
                        resource["ResourceType"].as_str(),
                        resource["Service"].as_str()
                    ),
                    (Some(resource_type), Some(service)),
                    "case {:?}: every resource of this view reports {resource_type}/{service}, \
                     got {resource}",
                    row.case
                );
            }
        }

        // The caller's own filter string intersects with the view's: a term the view
        // excludes leaves nothing, and one it allows narrows what it returns.
        let excluded = fixture
            .emulator
            .list_through(REGION, &view, "resourcetype:resource-explorer-2:index")
            .await;
        let case = format!("{}: a filter the view excludes", row.case);
        if row.admits_index {
            assert_eq!(
                excluded.arns(&case),
                set(&[&fixture.index]),
                "case {case:?}: expected only the index, got {}",
                excluded.body
            );
        } else {
            assert!(
                excluded.arns(&case).is_empty(),
                "case {case:?}: a view that cannot return the index must return nothing, \
                 got {}",
                excluded.body
            );
        }
    }
}

/// The one sentence `ListResources` answers a filter string that is not composed solely
/// of supported filter operators with. The text is fixed for the class: it states the
/// member and the requirement and carries no part of the submitted filter string, which
/// `SearchFilter` being `@sensitive` is what requires.
const FILTER_STRING_SYNTAX_MESSAGE: &str =
    "FilterString must contain only AREX supported filter operators and no freeform text.";

/// The filter string "supports only filter prefixes with optional operators. It doesn't
/// support free-form text", so a term that is not a COMPLETE filter operator — a
/// documented prefix followed by a value — is refused with 400 `ValidationException` and
/// the sentence that class fixes, never matched as text and never a 501. The accepted
/// rows show what a complete operator is: every documented prefix is served with a
/// value, even a value that selects nothing.
#[tokio::test]
async fn a_filter_string_carrying_free_form_text_is_refused() {
    let fixture = Fixture::new().await;

    // The rows differ along the FORM of the offending term: a bare word, a word beside a
    // well-formed filter, a negated bare word, a prefix that is not a filter name, one
    // that is nearly a filter name, and a documented prefix carrying no value at all —
    // an operator without the value that completes it, which is a prefix rather than a
    // pair however well the name itself is spelled.
    let refused = [
        ("a bare word", "orders"),
        ("a word beside a filter", "service:sqs orders"),
        ("a negated bare word", "-orders"),
        ("a prefix that names no filter", "name:orders"),
        ("a negated unknown prefix", "-queuename:orders"),
        ("a prefix nearly a filter name", "resourcetypes:sqs:queue"),
        ("a prefix nearly a tag filter name", "tag.name:stage"),
        ("a service prefix with no value", "service:"),
        ("a region prefix with no value", "region:"),
        ("a resource type prefix with no value", "resourcetype:"),
        ("an arn prefix with no value", "arn:"),
        ("an id prefix with no value", "id:"),
        ("an account prefix with no value", "accountid:"),
        ("a tag prefix with no value", "tag:"),
        ("a tag key prefix with no value", "tag.key:"),
        ("a negated prefix with no value", "-service:"),
        ("a valueless prefix beside a filter", "service:sqs region:"),
        ("a valueless prefix before a filter", "service: region:us-east-1"),
        ("a colon with no filter name", ":sqs"),
    ];
    for (case, filter_string) in refused {
        let answer = fixture.emulator.list(REGION, filter_string).await;
        answer.failed(case, 400, "ValidationException");
        let message = answer.error_message();
        assert_eq!(
            message, FILTER_STRING_SYNTAX_MESSAGE,
            "case {case:?}: expected the sentence this class fixes, got {message:?}"
        );
        // `SearchFilter` is @sensitive, so the string the caller sent is not echoed.
        assert!(
            !answer.body.to_string().contains(filter_string) || filter_string.is_empty(),
            "case {case:?}: a sensitive member's value must not be echoed, got {}",
            answer.body
        );
    }

    // Every documented filter prefix is served WITH a value. `tag` filters are included
    // because the fixture's default view includes the `tags` property.
    let accepted = [
        ("an empty filter string", ""),
        ("a service filter", "service:sqs"),
        ("a resource type filter", "resourcetype:sqs:queue"),
        ("a region filter", "region:us-east-1"),
        ("an arn filter", "arn:arn:aws:sqs:us-east-1:000000000000:ghost"),
        ("an id filter", "id:orders"),
        ("an account filter", "accountid:000000000000"),
        ("a global resource type filter", "globalresourcetype:ec2:vpc"),
        ("a tag filter", "tag:stage=prod"),
        ("a tag key filter", "tag.key:stage"),
        ("a tag value filter", "tag.value:prod"),
        ("a negated filter", "-service:sns"),
        ("a wildcard filter", "service:sq*"),
        ("a wildcard value of one character", "service:*"),
        ("a value that selects nothing", "service:no-such-service"),
        ("filters AND-ed", "service:sqs region:us-east-1 -id:orders"),
    ];
    for (case, filter_string) in accepted {
        let answer = fixture.emulator.list(REGION, filter_string).await;
        answer.ok(case);
    }
}

/// The filter-operator requirement belongs to `ListResources`' `Filters.FilterString`
/// ALONE: `Search`'s `QueryString` "supports the search keywords, prefixes, and
/// operators" with no such restriction, so the very strings `ListResources` refuses are
/// queries `Search` answers — reporting whatever they match, which for a term that
/// matches nothing is an empty result set with a complete count.
///
/// The rows span both ways a term fails `ListResources`' requirement (free-form text, a
/// documented prefix with no value) and, as positive controls, strings both operations
/// accept.
#[tokio::test]
async fn the_filter_operator_requirement_is_list_resources_own() {
    let fixture = Fixture::new().await;

    // (case, the string, whether ListResources accepts it)
    let rows = [
        ("free-form text", "orders", false),
        ("an unknown prefix", "name:orders", false),
        ("a prefix with no value", "service:", false),
        ("a negated prefix with no value", "-service:", false),
        ("a complete filter", "service:sqs", true),
        ("an empty query", "", true),
    ];
    for (case, query, listable) in rows {
        let listed = fixture.emulator.list(REGION, query).await;
        if listable {
            listed.ok(case);
        } else {
            listed.failed(case, 400, "ValidationException");
            assert_eq!(
                listed.error_message(),
                FILTER_STRING_SYNTAX_MESSAGE,
                "case {case:?}: expected the sentence this class fixes, got {}",
                listed.body
            );
        }

        // The same string through `Search`, which validates no operator syntax.
        let searched = fixture
            .emulator
            .explorer(
                REGION,
                "Search",
                json!({ "QueryString": query, "MaxResults": 5 }),
            )
            .await;
        let page = searched.arns(case).len() as u64;
        let body = searched.ok(case);
        let total = body["Count"]["TotalResources"].as_u64();
        assert!(
            total.is_some_and(|total| total >= page),
            "case {case:?}: Search reports the count of the whole match, got {}",
            searched.body
        );
        assert_eq!(
            body["Count"]["Complete"],
            json!(true),
            "case {case:?}: a match under the result limit is complete, got {}",
            searched.body
        );
    }

    // A term carrying a prefix with no value selects no value of that field, so `Search`
    // answers it with nothing at all — while the well-formed neighbour selects the
    // queues, which is what makes the empty answer about the term and not about the index.
    let case = "Search with a prefix carrying no value";
    let searched = fixture
        .emulator
        .explorer(
            REGION,
            "Search",
            json!({ "QueryString": "service:", "MaxResults": 5 }),
        )
        .await;
    assert_eq!(
        searched.arns(case),
        BTreeSet::new(),
        "case {case:?}: expected no match at all, got {}",
        searched.body
    );
    assert_eq!(
        searched.body["Count"],
        json!({ "TotalResources": 0, "Complete": true }),
        "case {case:?}: expected a complete count of nothing, got {}",
        searched.body
    );
    let case = "Search with the well-formed neighbour";
    let searched = fixture
        .emulator
        .explorer(
            REGION,
            "Search",
            json!({ "QueryString": "service:sqs", "MaxResults": 5 }),
        )
        .await;
    assert_eq!(
        searched.arns(case),
        set(&[&fixture.queue, &fixture.fifo_queue]),
        "case {case:?}: expected the queues, got {}",
        searched.body
    );
}

/// The match pages by `MaxResults` with an OPAQUE `NextToken` that resumes where the
/// page stopped, the walk covering the whole match exactly once whatever the page size;
/// a token issued for one query cannot resume another; and a page of the operation's
/// 1,000-result limit carries no token at all, because that page already holds every
/// result there is.
#[tokio::test]
async fn the_match_pages_with_an_opaque_next_token() {
    let fixture = Fixture::new().await;
    let every = fixture.every_arn();

    for page_size in [1_usize, 2, 3, 5, 100] {
        let case = format!("pages of {page_size}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request = json!({
                "Filters": { "FilterString": "" },
                "MaxResults": page_size,
            });
            if let Some(token) = &token {
                request["NextToken"] = json!(token);
            }
            let answer = fixture
                .emulator
                .explorer(REGION, "ListResources", request)
                .await;
            let page = answer.arns(&case);
            assert!(
                page.len() <= page_size,
                "case {case:?}: a page holds at most MaxResults, got {}",
                answer.body
            );
            seen.extend(page);
            pages += 1;
            assert!(
                pages <= every.len() + 1,
                "case {case:?}: the walk must terminate, got {pages} pages"
            );
            match answer.ok(&case).get("NextToken").and_then(Value::as_str) {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        let walked: BTreeSet<String> = seen.iter().cloned().collect();
        assert_eq!(
            walked, every,
            "case {case:?}: the walk covers the whole match, got {seen:?}"
        );
        assert_eq!(
            seen.len(),
            every.len(),
            "case {case:?}: no resource is reported twice, got {seen:?}"
        );
    }

    // A token is bound to the request it was issued for.
    let first = fixture
        .emulator
        .explorer(
            REGION,
            "ListResources",
            json!({ "Filters": { "FilterString": "" }, "MaxResults": 1 }),
        )
        .await;
    let token = first.ok("the first page")["NextToken"]
        .as_str()
        .expect("a first page of one carries a NextToken")
        .to_string();
    fixture
        .emulator
        .explorer(
            REGION,
            "ListResources",
            json!({
                "Filters": { "FilterString": "service:sqs" },
                "MaxResults": 1,
                "NextToken": token,
            }),
        )
        .await
        .failed("a token from another query", 400, "ValidationException");

    // "The ListResources operation does not generate a NextToken if you set MaxResults
    // to 1000": that page already carries every result the operation can return.
    let whole = fixture
        .emulator
        .explorer(
            REGION,
            "ListResources",
            json!({ "Filters": { "FilterString": "" }, "MaxResults": 1000 }),
        )
        .await;
    let case = "a page of the result limit";
    assert_eq!(
        whole.arns(case),
        every,
        "case {case:?}: expected the whole match, got {}",
        whole.body
    );
    assert_eq!(
        whole.ok(case).get("NextToken"),
        None,
        "case {case:?}: a page of 1000 issues no NextToken, got {}",
        whole.body
    );
}

/// A `ViewArn` the request NAMES and that resolves to no view this account holds is the
/// authorization failure that member documents: "if you don't have permission to use the
/// default view, then the operation fails with a 401 Unauthorized exception". The refusal
/// carries nothing from the request.
///
/// A request that names NO view is not one of these rows: it has no ARN to authorize, and
/// [`a_listing_with_no_view_reports_the_regions_index`] pins what it answers instead.
#[tokio::test]
async fn a_named_view_that_cannot_be_resolved_is_unauthorized() {
    let fixture = Fixture::new().await;

    // A Region with an index of its own, cleared of the default view turning Resource
    // Explorer on there left it: a Region with no default view is a state it has to be
    // cleared into.
    let bare = "ap-southeast-2";
    fixture.emulator.create_index(bare).await;
    fixture
        .emulator
        .explorer(bare, "DisassociateDefaultView", json!({}))
        .await
        .ok("clear the Region's default view");

    let unresolvable: Vec<(&str, Value)> = vec![
        (
            "a view ARN never minted",
            json!({
                "ViewArn": format!(
                    "arn:aws:resource-explorer-2:{bare}:{ACCOUNT_ID}:view/ghost/\
                     0f8c2a1e-5d3b-8a47-9c61-2b7e4d905a13"
                ),
            }),
        ),
        (
            "another account's view",
            json!({
                "ViewArn": format!(
                    "arn:aws:resource-explorer-2:{bare}:999999999999:view/other/\
                     5c2d8f31-7a46-8b92-ae03-1d49f6520b78"
                ),
            }),
        ),
        ("a bare name, not an ARN", json!({ "ViewArn": "not-an-arn" })),
    ];
    for (case, request) in unresolvable {
        let answer = fixture
            .emulator
            .explorer(bare, "ListResources", request.clone())
            .await;
        answer.failed(case, 401, "UnauthorizedException");
        assert_eq!(
            answer.error_message(),
            "Unauthorized",
            "case {case:?}: the refusal carries nothing from the request, got {}",
            answer.body
        );
    }
}

/// `ViewArn` is optional, and a Region with no default view does not make a listing
/// unanswerable: the operation reports that Region's index scoped by no view at all,
/// names no `ViewArn` because it used none, and still honours the caller's own filter
/// string, page size and refusals. A view the Region holds but has not made its default
/// scopes nothing until a request names it — which the positive control pins.
///
/// The rows span the filter NAME and operator, the number of resources selected, the page
/// size, a Region with no index at all, and the refusals a filter string still earns with
/// no view in play.
#[tokio::test]
async fn a_listing_with_no_view_reports_the_regions_index() {
    let fixture = Fixture::new().await;

    // Turning Resource Explorer on in a Region left it a default view, so having none is
    // a state the Region has to be CLEARED into.
    fixture
        .emulator
        .explorer(REGION, "DisassociateDefaultView", json!({}))
        .await
        .ok("clear the Region's default view");

    // A view the Region holds but has NOT made its default: its own filter string must
    // not scope a listing that does not name it.
    let unassociated = fixture
        .emulator
        .create_view(
            REGION,
            json!({
                "ViewName": "topics-only",
                "Filters": { "FilterString": "service:sns" },
            }),
        )
        .await;
    let mut every = fixture.every_arn();
    every.insert(unassociated.clone());
    let without_queues: BTreeSet<String> = every
        .iter()
        .filter(|arn| *arn != &fixture.queue && *arn != &fixture.fifo_queue)
        .cloned()
        .collect();

    let unscoped: Vec<(&str, Value, BTreeSet<String>)> = vec![
        ("no Filters member at all", json!({}), every.clone()),
        (
            "an empty filter string",
            json!({ "Filters": { "FilterString": "" } }),
            every.clone(),
        ),
        (
            "one service",
            json!({ "Filters": { "FilterString": "service:sqs" } }),
            set(&[&fixture.queue, &fixture.fifo_queue]),
        ),
        (
            "that service negated",
            json!({ "Filters": { "FilterString": "-service:sqs" } }),
            without_queues,
        ),
        (
            "a qualified resource type",
            json!({ "Filters": { "FilterString": "resourcetype:sns:topic" } }),
            set(&[&fixture.topic]),
        ),
        (
            "a suffix wildcard on the Region",
            json!({ "Filters": { "FilterString": "region:us-east-*" } }),
            every.clone(),
        ),
    ];

    for (case, request, expected) in unscoped {
        let answer = fixture
            .emulator
            .explorer(REGION, "ListResources", request.clone())
            .await;
        assert_eq!(
            answer.arns(case),
            expected,
            "case {case:?}: expected the Region's index scoped by the filter alone, got {}",
            answer.body
        );
        assert_eq!(
            answer.ok(case).get("ViewArn"),
            None,
            "case {case:?}: a listing that used no view names none, got {}",
            answer.body
        );
        for resource in answer.resources(case) {
            assert_eq!(
                resource.get("Properties"),
                None,
                "case {case:?}: no view included the {TAGS_PROPERTY} property, so no \
                 resource carries it, got {resource}"
            );
        }
    }

    // The page size still applies, and the token still resumes the same listing.
    let case = "a page of two with no view";
    let first = fixture
        .emulator
        .explorer(REGION, "ListResources", json!({ "MaxResults": 2 }))
        .await;
    let page = first.arns(case);
    assert_eq!(
        page.len(),
        2,
        "case {case:?}: expected a page of two, got {}",
        first.body
    );
    assert_eq!(
        first.ok(case).get("ViewArn"),
        None,
        "case {case:?}: a listing that used no view names none, got {}",
        first.body
    );
    let token = first.ok(case)["NextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: a partial page leaves a NextToken: {}", first.body))
        .to_string();
    let case = "the rest of that listing";
    let rest = fixture
        .emulator
        .explorer(
            REGION,
            "ListResources",
            json!({ "MaxResults": 100, "NextToken": token }),
        )
        .await;
    let whole: BTreeSet<String> = page.union(&rest.arns(case)).cloned().collect();
    assert_eq!(
        whole, every,
        "case {case:?}: the pages together are the whole listing, got {}",
        rest.body
    );

    // A Region with no index at all holds nothing, which is an empty listing and not a
    // refusal either.
    let case = "a Region with no index and no default view";
    let empty = fixture
        .emulator
        .explorer("ap-south-1", "ListResources", json!({}))
        .await;
    assert!(
        empty.arns(case).is_empty(),
        "case {case:?}: expected an empty Resources list, got {}",
        empty.body
    );
    assert_eq!(
        empty.ok(case).get("ViewArn"),
        None,
        "case {case:?}: a listing that used no view names none, got {}",
        empty.body
    );

    // The positive control: naming that view scopes the listing by its filter string and
    // echoes its ARN.
    let case = "the view named explicitly";
    let scoped = fixture
        .emulator
        .list_through(REGION, &unassociated, "")
        .await;
    assert_eq!(
        scoped.arns(case),
        set(&[&fixture.topic]),
        "case {case:?}: expected the view's own filter string to scope the listing, got {}",
        scoped.body
    );
    assert_eq!(
        scoped.ok(case)["ViewArn"].as_str(),
        Some(unassociated.as_str()),
        "case {case:?}: the answer names the view it used, got {}",
        scoped.body
    );

    // The refusals a filter string earns do not depend on a view resolving: a term that
    // is not a filter prefix is still free-form text, and a tag filter still needs the
    // `tags` property, which a listing scoped by no view includes no more than a view
    // without it does.
    let refusals: Vec<(&str, Value, &str)> = vec![
        (
            "free-form text with no view",
            json!({ "Filters": { "FilterString": "orders" } }),
            FILTER_STRING_SYNTAX_MESSAGE,
        ),
        (
            "a prefix with no value and no view",
            json!({ "Filters": { "FilterString": "service:" } }),
            FILTER_STRING_SYNTAX_MESSAGE,
        ),
        (
            "a tag filter with no view",
            json!({ "Filters": { "FilterString": "tag:stage=prod" } }),
            "not included in the view",
        ),
    ];
    for (case, request, expected) in refusals {
        let answer = fixture
            .emulator
            .explorer(REGION, "ListResources", request.clone())
            .await;
        answer.failed(case, 400, "ValidationException");
        assert!(
            answer.error_message().contains(expected),
            "case {case:?}: expected a message stating {expected:?}, got {:?}",
            answer.error_message()
        );
    }
}

/// The index is a LIVE view over the owning services: a resource created there is in it
/// with no Resource Explorer call in between, a tag written there is the tag a `tag:`
/// filter matches and the one a view that includes the `tags` property reports, and a
/// resource deleted there leaves the index while everything beside it stays.
#[tokio::test]
async fn the_index_follows_the_owning_services_own_records() {
    let fixture = Fixture::new().await;
    let case = "a queue created through SQS";
    let (url, arn) = fixture
        .emulator
        .create_queue(REGION, "invoices", Value::Null, Value::Null)
        .await;
    let answer = fixture.emulator.list(REGION, "resourcetype:sqs:queue").await;
    assert!(
        answer.arns(case).contains(&arn),
        "case {case:?}: expected {arn} in the index, got {}",
        answer.body
    );

    // A tag written through the owning service is the resource's own tag.
    let case = "a tag written through SQS";
    fixture
        .emulator
        .sqs(
            REGION,
            "TagQueue",
            json!({ "QueueUrl": &url, "Tags": { "stage": "test" } }),
        )
        .await;
    let answer = fixture.emulator.list(REGION, "tag:stage=test").await;
    assert_eq!(
        answer.arns(case),
        set(&[&arn]),
        "case {case:?}: expected the tag filter to select {arn}, got {}",
        answer.body
    );
    let properties = answer.resources(case)[0]["Properties"].clone();
    assert_eq!(
        properties[0]["Name"], json!(TAGS_PROPERTY),
        "case {case:?}: a view that includes the tags property reports it, got {properties}"
    );
    assert_eq!(
        properties[0]["Data"],
        json!([{ "Key": "stage", "Value": "test" }]),
        "case {case:?}: the reported tags are the owner's own, as a list of Key/Value \
         entries, got {properties}"
    );

    // A delete there removes it from the index; the queues beside it stay.
    let case = "a queue deleted through SQS";
    fixture
        .emulator
        .sqs(REGION, "DeleteQueue", json!({ "QueueUrl": &url }))
        .await;
    let answer = fixture.emulator.list(REGION, "resourcetype:sqs:queue").await;
    assert_eq!(
        answer.arns(case),
        set(&[&fixture.queue, &fixture.fifo_queue]),
        "case {case:?}: expected {arn} to leave the index, got {}",
        answer.body
    );

    // And the same for the topic, so the rule is per-owner rather than per-service.
    let case = "a topic deleted through SNS";
    fixture
        .emulator
        .sns(REGION, "DeleteTopic", &[("TopicArn", &fixture.topic)])
        .await;
    let answer = fixture.emulator.list(REGION, "").await;
    let remaining = answer.arns(case);
    assert!(
        !remaining.contains(&fixture.topic),
        "case {case:?}: expected the topic to leave the index, got {}",
        answer.body
    );
    assert!(
        remaining.contains(&fixture.queue) && remaining.contains(&fixture.index),
        "case {case:?}: everything beside it stays, got {}",
        answer.body
    );

    // Nothing above went through this service's store: the queue URL the fixture kept is
    // still the one SQS answers for its own queue.
    let read = fixture
        .emulator
        .sqs(
            REGION,
            "GetQueueAttributes",
            json!({ "QueueUrl": &fixture.queue_url, "AttributeNames": ["QueueArn"] }),
        )
        .await;
    assert_eq!(
        read["Attributes"]["QueueArn"],
        json!(fixture.queue),
        "the owning service still holds its own queue, got {read}"
    );
}

/// A tag filter reads a resource's TAGS, which a view carries only when it includes the
/// `tags` property: a view without it refuses a query that filters on tags, naming the
/// filter at fault, and reports no `Properties` at all.
#[tokio::test]
async fn a_tag_filter_needs_a_view_that_includes_the_tags_property() {
    let fixture = Fixture::new().await;
    let plain = fixture
        .emulator
        .create_view(REGION, json!({ "ViewName": "no-properties" }))
        .await;

    for (case, filter_string) in [
        ("the tag filter", "tag:stage=prod"),
        ("the tag key filter", "tag.key:stage"),
        ("the tag value filter", "tag.value:prod"),
    ] {
        let answer = fixture.emulator.list_through(REGION, &plain, filter_string).await;
        answer.failed(case, 400, "ValidationException");
        assert!(
            answer.error_message().contains("not included in the view"),
            "case {case:?}: the refusal names the view's missing property, got {:?}",
            answer.error_message()
        );
    }

    let case = "a view with no properties";
    let answer = fixture.emulator.list_through(REGION, &plain, "service:sqs").await;
    for resource in answer.resources(case) {
        assert_eq!(
            resource.get("Properties"),
            None,
            "case {case:?}: a view that does not include a property reports none, got {resource}"
        );
    }

    // The fixture's default view DOES include it, so the same filter is served there.
    let case = "a view that includes the tags property";
    let answer = fixture.emulator.list(REGION, "tag:stage=prod").await;
    assert_eq!(
        answer.arns(case),
        set(&[&fixture.queue]),
        "case {case:?}: expected the tagged queue, got {}",
        answer.body
    );
}

/// Every resource `ListResources` reports through one view is the resource `Search`
/// reports through the same view: there is ONE index and ONE projection of it, so the
/// two query operations cannot disagree about what the account holds.
#[tokio::test]
async fn list_resources_and_search_report_the_same_index() {
    let fixture = Fixture::new().await;
    for filter_string in ["", "service:sqs", "resourcetype:sns:topic", "region:us*"] {
        let listed = fixture.emulator.list(REGION, filter_string).await;
        let searched = fixture
            .emulator
            .explorer(
                REGION,
                "Search",
                json!({ "QueryString": filter_string, "MaxResults": 100 }),
            )
            .await;
        let case = format!("{filter_string:?} through both operations");
        assert_eq!(
            listed.arns(&case),
            searched.arns(&case),
            "case {case:?}: expected the same resources, got {} and {}",
            listed.body,
            searched.body
        );
    }
}

/// RULE: `ListSupportedResourceTypes` publishes Resource Explorer's own service-wide
/// catalogue — "all resource types currently supported by Amazon Web Services Resource
/// Explorer", each row carrying the associated service, the `<service>:<type>` identifier
/// and the CloudFormation type identifiers of that type — so the catalogue is a SUPERSET of
/// the types any one index actually reports resources for, is the same whatever the account
/// holds, and pages like every other paginated listing of this service.
///
/// This suite checks the superset direction end to end: every type the fixture's own
/// indexed resources are reported with is published, with that owning service and that
/// CloudFormation type, while the catalogue goes on to publish types nothing in this
/// emulator could ever report. The fixture's rows span every owning service the index reads
/// — this service's own index and views, an SQS queue, an SNS topic — and the page sizes
/// span one entry at a time up to the whole catalogue at once.
#[tokio::test]
async fn the_supported_resource_types_cover_every_type_a_query_can_report() {
    let fixture = Fixture::new().await;

    // The catalogue, read one whole page at a time.
    let whole = fixture
        .emulator
        .explorer(REGION, "ListSupportedResourceTypes", json!({}))
        .await;
    let case = "the whole catalogue";
    let published = whole.ok(case)["ResourceTypes"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a ResourceTypes list, got {}", whole.body))
        .clone();
    assert_eq!(
        whole.body.get("NextToken"),
        None,
        "case {case:?}: a page that carries the whole catalogue resumes nothing, got {}",
        whole.body
    );

    // What a query actually reports for each resource the fixture created, which is the
    // projection the catalogue has to describe.
    let queried = fixture.emulator.list(REGION, "").await;
    let reported = queried.resources("every indexed resource");
    let mut covered = 0usize;
    for resource in &fixture.indexed() {
        let case = resource.case;
        let found = reported
            .iter()
            .find(|candidate| candidate["Arn"] == json!(resource.arn))
            .unwrap_or_else(|| panic!("case {case:?}: {} is not indexed", resource.arn));
        let entry = published
            .iter()
            .find(|candidate| candidate["ResourceType"] == found["ResourceType"])
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: the index reports {} but the catalogue does not publish \
                     it, got {published:?}",
                    found["ResourceType"]
                )
            });
        assert_eq!(
            &entry["Service"],
            &json!(resource.service),
            "case {case:?}: expected the owning service of this kind, got {entry}"
        );
        let cfn = entry["CFNResourceTypes"]
            .as_array()
            .unwrap_or_else(|| {
                panic!("case {case:?}: expected a CFNResourceTypes list, got {entry}")
            })
            .clone();
        assert!(
            cfn.contains(&json!(resource.cfn_resource_type)),
            "case {case:?}: expected the CloudFormation type of this kind among {cfn:?}"
        );
        covered += 1;
    }
    assert_eq!(
        covered,
        fixture.indexed().len(),
        "every indexed resource's type is published"
    );
    // The catalogue is the SERVICE's list, not this emulator's: it publishes strictly more
    // types than anything in this process can report a resource of.
    let reportable: BTreeSet<String> = reported
        .iter()
        .map(|resource| resource["ResourceType"].clone().to_string())
        .collect();
    assert!(
        published.len() > reportable.len(),
        "the catalogue publishes every type Resource Explorer supports, which is more than \
         the {} this emulator's index can report, got {}",
        reportable.len(),
        published.len()
    );

    // The page size cuts the catalogue and the token resumes it, for every size from one
    // entry to more than the catalogue holds.
    for size in [1, 2, published.len(), published.len() + 1] {
        let case = format!("pages of {size}");
        let mut walked: Vec<Value> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut request = json!({ "MaxResults": size });
            if let Some(resume) = &token {
                request["NextToken"] = json!(resume);
            }
            let answer = fixture
                .emulator
                .explorer(REGION, "ListSupportedResourceTypes", request)
                .await;
            let page = answer.ok(&case)["ResourceTypes"]
                .as_array()
                .unwrap_or_else(|| panic!("case {case:?}: expected a ResourceTypes list"))
                .clone();
            assert!(
                page.len() <= size,
                "case {case:?}: a page holds at most the size asked for, got {}",
                page.len()
            );
            walked.extend(page);
            token = answer.body["NextToken"].as_str().map(str::to_string);
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            walked, published,
            "case {case:?}: the pages walk the whole catalogue in one order, got {walked:?}"
        );
    }

    // A token this listing never issued is refused, not answered with a slice of it.
    let answer = fixture
        .emulator
        .explorer(
            REGION,
            "ListSupportedResourceTypes",
            json!({ "NextToken": "not-a-token-this-listing-issued" }),
        )
        .await;
    answer.failed("a token this listing never issued", 400, "ValidationException");
}

/// RULE: the catalogue `ListSupportedResourceTypes` publishes is Resource Explorer's own
/// fixed service metadata — one row per type the SERVICE supports, independent of what any
/// account, Region or deployment holds — where `Service` is the first segment of the
/// `<service>:<type>` identifier and `CFNResourceTypes` carries the CloudFormation type
/// identifiers of that type; the catalogue is published in ascending `ResourceType` order,
/// with no identifier listed twice.
///
/// The spot-checked rows are entries of Resource Explorer's published "Supported resource
/// types" reference, chosen for the forms the rule generalises over and NOT for anything
/// this emulator implements: a plain type of a service no crate here emulates
/// (`ec2:instance`, `s3:bucket`, `lambda:function`, `iam:role`, `dynamodb:table`), one whose
/// service namespace differs from its CloudFormation service (`acm` ->
/// `AWS::CertificateManager::Certificate`, `airflow` -> `AWS::MWAA::Environment`), one whose
/// identifier carries a path segment (`amplify:apps/branches`), one whose type name is
/// camel-cased (`states:stateMachine`), and one type covering SEVERAL CloudFormation types
/// (`datasync:location`). The invariants are then asserted over EVERY row, and the whole
/// answer is compared against an emulator holding no index and no resources at all, which is
/// what says the catalogue is not derived from state.
#[tokio::test]
async fn the_published_catalogue_is_resource_explorers_fixed_service_metadata() {
    let fixture = Fixture::new().await;
    let case = "the whole catalogue";
    let answer = fixture
        .emulator
        .explorer(REGION, "ListSupportedResourceTypes", json!({}))
        .await;
    let published = answer.ok(case)["ResourceTypes"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a ResourceTypes list, got {}", answer.body))
        .clone();

    // (the `<service>:<type>` identifier, its `Service`, its CloudFormation type identifiers)
    let rows: Vec<(&str, &str, Vec<&str>)> = vec![
        ("ec2:instance", "ec2", vec!["AWS::EC2::Instance"]),
        ("s3:bucket", "s3", vec!["AWS::S3::Bucket"]),
        ("lambda:function", "lambda", vec!["AWS::Lambda::Function"]),
        ("iam:role", "iam", vec!["AWS::IAM::Role"]),
        ("dynamodb:table", "dynamodb", vec!["AWS::DynamoDB::Table"]),
        (
            "acm:certificate",
            "acm",
            vec!["AWS::CertificateManager::Certificate"],
        ),
        ("airflow:environment", "airflow", vec!["AWS::MWAA::Environment"]),
        ("amplify:apps/branches", "amplify", vec!["AWS::Amplify::Branch"]),
        (
            "states:stateMachine",
            "states",
            vec!["AWS::StepFunctions::StateMachine"],
        ),
        (
            "datasync:location",
            "datasync",
            vec![
                "AWS::DataSync::LocationEFS",
                "AWS::DataSync::LocationFSxLustre",
                "AWS::DataSync::LocationFSxONTAP",
                "AWS::DataSync::LocationFSxOpenZFS",
                "AWS::DataSync::LocationFSxWindows",
                "AWS::DataSync::LocationHDFS",
                "AWS::DataSync::LocationNFS",
                "AWS::DataSync::LocationObjectStorage",
                "AWS::DataSync::LocationS3",
                "AWS::DataSync::LocationSMB",
            ],
        ),
    ];
    for (resource_type, service, cfn_resource_types) in rows {
        let case = resource_type;
        let entry = published
            .iter()
            .find(|candidate| candidate["ResourceType"] == json!(resource_type))
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: the catalogue must publish this supported type, and it \
                     publishes {} entries",
                    published.len()
                )
            });
        assert_eq!(
            entry["Service"],
            json!(service),
            "case {case:?}: expected the service associated with this type, got {entry}"
        );
        assert_eq!(
            entry["CFNResourceTypes"],
            json!(cfn_resource_types),
            "case {case:?}: expected this type's CloudFormation identifiers, got {entry}"
        );
    }

    // Every row, not only the ones named above, is well formed and in the published order.
    let mut previous: Option<String> = None;
    for entry in &published {
        let identifier = entry["ResourceType"]
            .as_str()
            .unwrap_or_else(|| panic!("a catalogue entry carries a ResourceType, got {entry}"))
            .to_string();
        let case = identifier.clone();
        let (service, kind) = identifier
            .split_once(':')
            .unwrap_or_else(|| panic!("case {case:?}: a ResourceType is <service>:<type>"));
        assert!(
            !service.is_empty() && !kind.is_empty(),
            "case {case:?}: both segments of a ResourceType carry text"
        );
        assert_eq!(
            entry["Service"],
            json!(service),
            "case {case:?}: Service is the identifier's first segment, got {entry}"
        );
        let cfn = entry["CFNResourceTypes"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected a CFNResourceTypes list, got {entry}"));
        for identifier in cfn {
            let identifier = identifier
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: a CloudFormation identifier is a string"));
            assert!(
                identifier.starts_with("AWS::") && identifier.split("::").count() == 3,
                "case {case:?}: expected an AWS::<Service>::<Type> identifier, got \
                 {identifier:?}"
            );
        }
        if let Some(previous) = &previous {
            assert!(
                previous.as_str() < case.as_str(),
                "case {case:?}: the catalogue is ascending by ResourceType with no duplicates, \
                 got {previous:?} before it"
            );
        }
        previous = Some(case);
    }

    // The same answer from an emulator that holds nothing at all: no index, no view, no
    // resource of any owning service. A published catalogue is not an inventory.
    let empty = Emulator::new();
    let case = "an emulator holding nothing";
    let answer = empty
        .explorer(REGION, "ListSupportedResourceTypes", json!({}))
        .await;
    assert_eq!(
        answer.ok(case)["ResourceTypes"],
        json!(published),
        "case {case:?}: expected the same published catalogue, got {}",
        answer.body
    );
}

/// The `Resources` list is a `BTreeMap`-free projection of persisted state, so two reads
/// of an unchanged index answer identically — a query writes nothing.
#[tokio::test]
async fn a_query_changes_nothing_a_later_caller_sees() {
    let fixture = Fixture::new().await;
    let before = fixture.emulator.list(REGION, "").await;
    let case = "the first read";
    let first: BTreeMap<String, Value> = before
        .resources(case)
        .into_iter()
        .map(|resource| (resource["Arn"].as_str().unwrap_or_default().to_string(), resource))
        .collect();
    let after = fixture.emulator.list(REGION, "").await;
    let case = "the second read";
    let second: BTreeMap<String, Value> = after
        .resources(case)
        .into_iter()
        .map(|resource| (resource["Arn"].as_str().unwrap_or_default().to_string(), resource))
        .collect();
    assert_eq!(
        first, second,
        "case {case:?}: a read must leave the index exactly as it was, got {} then {}",
        before.body, after.body
    );
}

/// A Region's default-view ASSOCIATION, and nothing else, decides which view a
/// `ListResources` request that names none is served through: the view the Region
/// associates while it associates one, and no view at all while it associates none —
/// which is a 200 over that Region's index naming no `ViewArn`, never a refusal.
/// `ViewArn` is an optional input member, so what the listing is scoped by is a property
/// of the Region it reads, and the association is state a caller moves back and forth.
///
/// The rows span the association state (associated, cleared, associated to another view,
/// cleared again, associated to the view the Region was turned on with, cleared a third
/// time) and the scope the associated view carries (unfiltered, and one whose own filter
/// string selects a single resource type). Each row is read back through `GetDefaultView`
/// too, so the association the listing used is the association the service reports.
#[tokio::test]
async fn the_default_view_association_decides_the_view_a_viewless_listing_uses() {
    let fixture = Fixture::new().await;

    // A view whose own filter string selects ONE resource type: what tells a listing
    // scoped by the associated view apart from one scoped by nothing at all, since the
    // fixture's default view is unfiltered.
    let indexes_only = fixture
        .emulator
        .create_view(
            REGION,
            json!({
                "ViewName": "indexes-only",
                "Filters": { "FilterString": "resourcetype:resource-explorer-2:index" },
            }),
        )
        .await;
    // Creating it put it in the Region's index, so an unscoped listing reports it too.
    let mut every = fixture.every_arn();
    every.insert(indexes_only.clone());

    // (case, the view the Region associates — `None` for no association at all, the ARNs
    // a listing that names no view must then report)
    let rows: Vec<(&str, Option<&String>, BTreeSet<String>)> = vec![
        (
            "the unfiltered view the fixture associated",
            Some(&fixture.default_view),
            every.clone(),
        ),
        ("that association cleared", None, every.clone()),
        (
            "a view of its own scope associated",
            Some(&indexes_only),
            set(&[&fixture.index]),
        ),
        ("cleared again", None, every.clone()),
        (
            "the view the Region was turned on with associated",
            Some(&fixture.turn_on_view),
            every.clone(),
        ),
        ("cleared a third time", None, every.clone()),
    ];

    for (case, associated, expected) in rows {
        match associated {
            Some(view_arn) => fixture.emulator.set_default_view(REGION, view_arn).await,
            None => {
                fixture
                    .emulator
                    .explorer(REGION, "DisassociateDefaultView", json!({}))
                    .await
                    .ok(&format!("{case}: clear the Region's default view"));
            }
        }

        let reported = fixture
            .emulator
            .explorer(REGION, "GetDefaultView", json!({}))
            .await;
        assert_eq!(
            reported.ok(case).get("ViewArn").and_then(Value::as_str),
            associated.map(String::as_str),
            "case {case:?}: GetDefaultView must report the association in force, got {}",
            reported.body
        );

        let answer = fixture
            .emulator
            .explorer(REGION, "ListResources", json!({}))
            .await;
        assert_eq!(
            answer.arns(case),
            expected,
            "case {case:?}: expected the listing the association in force scopes, got {}",
            answer.body
        );
        assert_eq!(
            answer.ok(case).get("ViewArn").and_then(Value::as_str),
            associated.map(String::as_str),
            "case {case:?}: the answer names the view it used, and none where it used \
             none, got {}",
            answer.body
        );
    }
}

/// A listing that names no view is answered per REGION and is a pure read: the Region the
/// request was signed for decides which index it reports and whether any default view is
/// in force, a Region keeps its own association when another Region's is cleared, and the
/// call associates no default view as a side effect. A view the request NAMES is read in
/// its OWN Region whatever Region the request was signed for, and its ARN is echoed.
///
/// The rows span the Region the request is signed for: one whose association was cleared
/// and one that kept its own. What the same file already owns is not repeated here — a
/// Region with no index at all and a view named explicitly in the Region's own
/// association-free state are rows of [`a_listing_with_no_view_reports_the_regions_index`],
/// a second read of an unchanged index is
/// [`a_query_changes_nothing_a_later_caller_sees`], and a named ARN that resolves to no
/// view is [`a_named_view_that_cannot_be_resolved_is_unauthorized`].
#[tokio::test]
async fn a_viewless_listing_is_answered_per_region_and_associates_nothing() {
    let fixture = Fixture::new().await;
    fixture
        .emulator
        .explorer(REGION, "DisassociateDefaultView", json!({}))
        .await
        .ok("clear the Region's default view");
    let abroad = set(&[
        &fixture.foreign_index,
        &fixture.foreign_turn_on_view,
        &fixture.foreign_view,
        &fixture.foreign_queue,
    ]);

    // (case, the Region the request is signed for, the ViewArn the answer must name, the
    // ARNs it must report)
    let regions: Vec<(&str, &str, Option<&String>, BTreeSet<String>)> = vec![
        (
            "the Region whose association was cleared",
            REGION,
            None,
            fixture.every_arn(),
        ),
        (
            "a Region that kept its own association",
            OTHER_REGION,
            Some(&fixture.foreign_view),
            abroad.clone(),
        ),
    ];
    for (case, region, view_arn, expected) in regions {
        let answer = fixture
            .emulator
            .explorer(region, "ListResources", json!({}))
            .await;
        assert_eq!(
            answer.arns(case),
            expected,
            "case {case:?}: expected the index of the Region the request names, got {}",
            answer.body
        );
        assert_eq!(
            answer.ok(case).get("ViewArn").and_then(Value::as_str),
            view_arn.map(String::as_str),
            "case {case:?}: the answer names the view it used, and none where it used \
             none, got {}",
            answer.body
        );
    }

    // Nothing about the listing associated a default view: the Region still has none, so a
    // later caller is answered the same way rather than through a view this read wrote.
    let case = "the association after the listing";
    let reported = fixture
        .emulator
        .explorer(REGION, "GetDefaultView", json!({}))
        .await;
    assert_eq!(
        reported.ok(case).get("ViewArn"),
        None,
        "case {case:?}: a listing must associate no default view, got {}",
        reported.body
    );

    // A view of ANOTHER Region, named by a request signed for a Region that associates
    // none: the listing is the named view's own Region's index, and its ARN is echoed —
    // which is what says the Region the request was signed for governs only the
    // association, not the index a named view reads.
    let case = "a view of another Region, named explicitly";
    let answer = fixture
        .emulator
        .list_through(REGION, &fixture.foreign_view, "")
        .await;
    assert_eq!(
        answer.arns(case),
        abroad,
        "case {case:?}: expected the index of the named view's Region, got {}",
        answer.body
    );
    assert_eq!(
        answer.ok(case)["ViewArn"].as_str(),
        Some(fixture.foreign_view.as_str()),
        "case {case:?}: the answer echoes the view the request named, got {}",
        answer.body
    );
}

// =====================================================================================
// The wire FORM of what a query answers: the shapes an SDK client deserializes
// =====================================================================================

/// Whether `value` is the RFC 3339 / ISO-8601 date-time STRING a member the model marks
/// `@timestampFormat("date-time")` travels as on this restJson1 service.
///
/// The epoch-seconds NUMBER the JSON protocols default to for a timestamp is exactly what
/// this rejects: an SDK deserializing a `date-time` member runs a date-time parser over it
/// and does not coerce a bare number. Smithy's `date-time` is RFC 3339 at the `Z` offset,
/// so the date/time separator and the designator are part of the form.
fn is_date_time(value: &Value) -> bool {
    value.as_str().is_some_and(|text| {
        text.contains('T')
            && text.ends_with('Z')
            && ::aws_smithy_types::DateTime::from_str(
                text,
                ::aws_smithy_types::date_time::Format::DateTime,
            )
            .is_ok()
    })
}

/// RULE: a timestamp member this model marks `@timestampFormat("date-time")` travels on
/// the wire as an RFC 3339 date-time STRING, never as the epoch-seconds number the JSON
/// protocol defaults to.
///
/// The rows span the operations and the members the rule covers — every timestamp member
/// this model declares carries that format: the resource and its reported property
/// (`Resource$LastReportedAt`, `ResourceProperty$LastReportedAt`, on both query
/// operations), a view (`View$LastUpdatedAt`, as created and as read back) and an index
/// (`GetIndexOutput$CreatedAt`/`$LastUpdatedAt`, `CreateIndexOutput$CreatedAt`). The
/// recorded case — one `ListResources` resource and its `tags` property — is two rows.
#[tokio::test]
async fn every_declared_date_time_member_travels_as_a_date_time_string() {
    let fixture = Fixture::new().await;
    // (case, the member's value as it reached the wire)
    let mut reported: Vec<(String, Value)> = Vec::new();

    // The two query operations, which project an indexed resource the same way: the
    // listing uses the default view, which includes the `tags` property, so each
    // resource's property carries its own instant beside the resource's.
    for (operation, request) in [
        ("ListResources", json!({ "Filters": { "FilterString": "" } })),
        ("Search", json!({ "QueryString": "" })),
    ] {
        let answer = fixture.emulator.explorer(REGION, operation, request).await;
        let resources = answer.ok(operation)["Resources"]
            .as_array()
            .unwrap_or_else(|| panic!("case {operation:?}: expected a Resources list"))
            .clone();
        assert!(
            !resources.is_empty(),
            "case {operation:?}: the fixture's index holds resources, got {}",
            answer.body
        );
        for resource in resources {
            let arn = resource["Arn"].as_str().unwrap_or_default().to_string();
            reported.push((
                format!("{operation} Resources[{arn}].LastReportedAt"),
                resource["LastReportedAt"].clone(),
            ));
            let properties = resource["Properties"].as_array().cloned().unwrap_or_default();
            assert_eq!(
                properties.len(),
                1,
                "case {operation:?}: the view includes the {TAGS_PROPERTY} property, so \
                 {arn} reports it, got {resource}"
            );
            for property in properties {
                reported.push((
                    format!("{operation} Resources[{arn}].Properties[].LastReportedAt"),
                    property["LastReportedAt"].clone(),
                ));
            }
        }
    }

    // The same rule holds on every other member the model marks with the format: a view's
    // own last-updated instant, on the answer that created it and on the read back, and an
    // index's created and last-updated instants.
    let created_view = fixture
        .emulator
        .explorer(REGION, "CreateView", json!({ "ViewName": "wire-form" }))
        .await;
    reported.push((
        "CreateView View.LastUpdatedAt".to_string(),
        created_view.ok("CreateView")["View"]["LastUpdatedAt"].clone(),
    ));
    let read_view = fixture
        .emulator
        .explorer(
            REGION,
            "GetView",
            json!({ "ViewArn": &fixture.default_view }),
        )
        .await;
    reported.push((
        "GetView View.LastUpdatedAt".to_string(),
        read_view.ok("GetView")["View"]["LastUpdatedAt"].clone(),
    ));
    let read_index = fixture.emulator.explorer(REGION, "GetIndex", json!({})).await;
    for member in ["CreatedAt", "LastUpdatedAt"] {
        reported.push((
            format!("GetIndex {member}"),
            read_index.ok("GetIndex")[member].clone(),
        ));
    }
    let third_region = "ap-northeast-1";
    let created_index = fixture
        .emulator
        .explorer(
            third_region,
            "CreateIndex",
            json!({ "ClientToken": third_region }),
        )
        .await;
    reported.push((
        "CreateIndex CreatedAt".to_string(),
        created_index.ok("CreateIndex")["CreatedAt"].clone(),
    ));

    for (case, value) in reported {
        assert!(
            is_date_time(&value),
            "case {case:?}: the model marks this member @timestampFormat(\"date-time\"), so \
             it must be an RFC 3339 date-time string, got {value}"
        );
    }
}

/// RULE: the `tags` resource property reports a resource's tag set in its `Data` document
/// as a LIST with one entry per tag, each entry an object carrying the tag's key under
/// `Key` and its value under `Value` — the form every AWS tagging surface reports a tag
/// set in, and the one a client walking a resource's tags iterates.
///
/// The rows span what the rule generalises over: the NUMBER of tags on the resource (none,
/// one, several), the ORDER the owning service wrote them in, punctuation in a key, and a
/// tag whose value is the empty string. Both query operations report the identical
/// document for one resource, because one projection builds it.
#[tokio::test]
async fn the_tags_property_reports_one_key_value_entry_per_tag() {
    let emulator = Emulator::new();
    emulator.create_index(REGION).await;
    let view = emulator
        .create_view(
            REGION,
            json!({
                "ViewName": "tagged",
                "IncludedProperties": [{ "Name": TAGS_PROPERTY }],
            }),
        )
        .await;
    emulator.set_default_view(REGION, &view).await;

    // (case, the queue name, the tag set written through the owning service, the document
    // the `tags` property must report)
    let rows: Vec<(&str, &str, Value, Value)> = vec![
        ("a resource carrying no tags", "untagged", Value::Null, json!([])),
        (
            "one tag",
            "single",
            json!({ "stage": "prod" }),
            json!([{ "Key": "stage", "Value": "prod" }]),
        ),
        (
            "three tags, written in another order",
            "triple",
            json!({ "team": "payments", "cost:center": "4711", "stage": "prod" }),
            json!([
                { "Key": "cost:center", "Value": "4711" },
                { "Key": "stage", "Value": "prod" },
                { "Key": "team", "Value": "payments" },
            ]),
        ),
        (
            "a tag whose value is the empty string",
            "empty-value",
            json!({ "stage": "" }),
            json!([{ "Key": "stage", "Value": "" }]),
        ),
    ];

    let mut expected: Vec<(&str, String, Value)> = Vec::new();
    for (case, name, tags, document) in rows {
        let (_, arn) = emulator.create_queue(REGION, name, Value::Null, tags).await;
        expected.push((case, arn, document));
    }

    for (operation, request) in [
        ("ListResources", json!({ "Filters": { "FilterString": "service:sqs" } })),
        ("Search", json!({ "QueryString": "service:sqs" })),
    ] {
        let answer = emulator.explorer(REGION, operation, request).await;
        let resources = answer.ok(operation)["Resources"]
            .as_array()
            .unwrap_or_else(|| panic!("case {operation:?}: expected a Resources list"))
            .clone();
        for (case, arn, document) in &expected {
            let found = resources
                .iter()
                .find(|candidate| candidate["Arn"] == json!(arn))
                .unwrap_or_else(|| {
                    panic!("case {case:?} ({operation}): {arn} is not in {}", answer.body)
                });
            assert_eq!(
                found["Properties"][0]["Name"],
                json!(TAGS_PROPERTY),
                "case {case:?} ({operation}): the view includes the {TAGS_PROPERTY} \
                 property, so the resource reports it, got {found}"
            );
            assert_eq!(
                found["Properties"][0]["Data"],
                *document,
                "case {case:?} ({operation}): expected the {TAGS_PROPERTY} document \
                 {document}, got {}",
                found["Properties"][0]["Data"]
            );
        }
    }
}

/// RULE: a failure of this service reports its detail text in the body member `message` —
/// the same member its framework-rendered refusals already carry it in, so a client reads
/// one member whichever layer answered.
///
/// The rows span the failure classes a query earns: the authorization failure a `ViewArn`
/// that resolves to no view this account holds is answered with (whose text is wholly
/// fixed for that class), the modelled refusals the operation words itself, and a request
/// body that breaks a modelled constraint, which the framework refuses before the handler
/// runs. The recorded case — the 401 — is one row.
#[tokio::test]
async fn a_failures_detail_text_is_reported_in_the_message_member() {
    let fixture = Fixture::new().await;
    // (case, the request, the status, the error code, the exact text where the class fixes
    // it)
    let rows: Vec<(&str, Value, u16, &str, Option<&str>)> = vec![
        (
            "a ViewArn that resolves to no view this account holds",
            json!({ "ViewArn": format!(
                "arn:aws:resource-explorer-2:{REGION}:{ACCOUNT_ID}:view/ghost/\
                 0f8c2a1e-5d3b-8a47-9c61-2b7e4d905a13"
            ) }),
            401,
            "UnauthorizedException",
            Some("Unauthorized"),
        ),
        (
            "a filter string carrying free-form text",
            json!({ "Filters": { "FilterString": "orders" } }),
            400,
            "ValidationException",
            Some(FILTER_STRING_SYNTAX_MESSAGE),
        ),
        (
            "a filter string carrying a prefix with no value",
            json!({ "Filters": { "FilterString": "service:" } }),
            400,
            "ValidationException",
            Some(FILTER_STRING_SYNTAX_MESSAGE),
        ),
        (
            "a page size outside the modelled range",
            json!({ "MaxResults": 0 }),
            400,
            "ValidationException",
            None,
        ),
    ];

    for (case, request, status, code, text) in rows {
        let answer = fixture
            .emulator
            .explorer(REGION, "ListResources", request)
            .await;
        answer.failed(case, status, code);
        let reported = answer.body.get("message").and_then(Value::as_str);
        assert!(
            reported.is_some_and(|message| !message.is_empty()),
            "case {case:?}: the failure must report its text in `message`, got {}",
            answer.body
        );
        assert_eq!(
            answer.body.get("Message"),
            None,
            "case {case:?}: this service reports that text in `message` alone, got {}",
            answer.body
        );
        if let Some(expected) = text {
            assert_eq!(
                reported,
                Some(expected),
                "case {case:?}: expected the fixed text {expected:?}, got {}",
                answer.body
            );
        }
    }
}
