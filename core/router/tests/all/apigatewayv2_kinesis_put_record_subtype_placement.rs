// SPDX-License-Identifier: CC0-1.0

//! WHICH parameters a `Kinesis-PutRecord` subtype route carries into the put: every member of
//! the action its `RequestParameters` define a value for, and not only the three the subtype
//! cannot be configured without.
//!
//! RULE: **an HTTP api route whose integration is `IntegrationType=AWS_PROXY` naming an
//! `IntegrationSubtype` performs that AWS service action with EVERY parameter its
//! `RequestParameters` map resolved out of the request — an optional one included, and one
//! whose expression the request carries no value for simply left unsupplied — so for
//! `Kinesis-PutRecord` a mapped `ExplicitHashKey` is the hash the stream places the record by,
//! a request that carries no value for it leaves the record placed by its partition key, a
//! mapped `SequenceNumberForOrdering` is carried into the call as well, and a hash key outside
//! the key space is the stream api's own refusal relayed to the caller with nothing stored.**
//!
//! `Integration$RequestParameters` is "a key-value map specifying parameters that are passed to
//! AWS_PROXY integrations" — the ARGUMENTS of the action the subtype names, which for this one
//! are the members of `PutRecordInput`
//! (`smithy_query(query="operation", operation="PutRecord")` on the Amazon Kinesis model). Two
//! of those members decide WHERE the record goes: `PartitionKey` ("an MD5 hash function is used
//! to map partition keys to 128-bit integer values … all data records with the same partition
//! key map to the same shard within the stream") and `ExplicitHashKey` ("the hash value used to
//! explicitly determine the shard the data record is assigned to by overriding the partition
//! key hash"), each a value an integration may map — so a route that maps the hash key places
//! its records by it, and the SAME route placing a request that carries none falls back to the
//! key. `SequenceNumberForOrdering` ("guarantees strictly increasing sequence numbers, for puts
//! from the same client and to the same partition key") is the third optional one.
//!
//! Nothing asserted here is a value this gateway fills in. Where a record went is read off the
//! SHARD MAP the stream api publishes — `ListShards`, whose `Shard$HashKeyRange` is "the range
//! of possible hash key values for the shard, which is a set of ordered contiguous positive
//! integers" — and the record itself is read back through that service's own
//! `GetShardIterator`/`GetRecords` at the very position the route reported. The hash values the
//! rows below are driven with are the PUBLISHED bounds of each of the stream's shards rather
//! than numbers written out here, plus the two values just outside the 128-bit key space that
//! `HashKey`'s own pattern (`^(0|([1-9]\d{0,38}))$`) still admits.
//!
//! What the sibling suite
//! (`core/router/tests/all/apigatewayv2_kinesis_put_record_subtype.rs`) pins and this one does
//! not restate: the three REQUIRED parameters' own mapping expressions, the relay of a refusal
//! the role's policies or the named stream caused, and the gateway's own answer for a mapping
//! this request or this gateway cannot satisfy.

use std::cmp::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::query_wire::form_encode;

/// The Region every call below is signed for, and the Region in the hosts of each service.
const REGION: &str = "us-east-1";

/// The subtype every route below names, and the action its role has to hold for the put to
/// happen.
const PUT_RECORD_SUBTYPE: &str = "Kinesis-PutRecord";
const PUT_RECORD_ACTION: &str = "kinesis:PutRecord";

/// The inline policy this suite writes the role's grant on.
const INLINE_POLICY: &str = "may-put";

/// The service principal API Gateway assumes a `CredentialsArn` role as.
const GATEWAY_PRINCIPAL: &str = "apigateway.amazonaws.com";

/// The service shape an `awsJson1_1` `X-Amz-Target` names an operation of the stream api under
/// (`smithy_query(query="service_info")` on that model).
const KINESIS_TARGET_PREFIX: &str = "Kinesis_20131202";

/// How many shards every stream below is created with, so a record's placement is one of
/// several and each shard publishes a range of its own.
const SHARD_COUNT: u64 = 4;

/// The status Kinesis reports for a stream whose create has fallen due.
const STREAM_ACTIVE: &str = "ACTIVE";

/// The statuses the calls below are answered with: a performed action, a created resource, and
/// the client refusal the stream api states for a parameter it cannot use.
const PERFORMED: u16 = 200;
const CREATED: u16 = 201;
const ACTION_REFUSED: u16 = 400;

/// The error the stream api names for a hash key it cannot place a record by
/// (`smithy_query(query="operation", operation="PutRecord")` lists it as a client error), and
/// the members a relayed refusal carries: the error shape, under the discriminator a JSON error
/// envelope names a shape in, and the text that service stated.
const INVALID_ARGUMENT: &str = "InvalidArgumentException";
const SHAPE_MEMBER: &str = "__type";
const DETAIL_MEMBER: &str = "Message";

/// The one route every api below carries, and the path a client posts to on it.
const ROUTE_KEY: &str = "POST /put";
const ROUTE_PATH: &str = "/put";

/// How long a clock-owed transition is waited for, and how often it is looked at while waiting.
/// Generous: the test waits for the CONDITION, and the budget is never the thing asserted.
const SETTLE_BUDGET: Duration = Duration::from_secs(60);
const LOOK_INTERVAL: Duration = Duration::from_millis(25);

/// API Gateway v2, Amazon Kinesis Data Streams, IAM and the token service over ONE engine, as
/// `core/router/src/lib.rs` mounts them: the action the integration performs is dispatched back
/// into this same router, so the stream it writes to is the stream this suite created.
struct Estate {
    router: MultiServiceRouter,
}

impl Estate {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // The suite exercises the services, not SigV4: a control-plane call carries a credential
        // scope so the router can name the target service, and the invoke of the published
        // address carries none, as a client of a published HTTP api endpoint does. The
        // INTEGRATION's own call is signed by the emulator, with the session it minted.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_apigatewayv2_apigatewayv2::EmulatedService::new(Arc::clone(
                &engine,
            )),
        );
        router.register_service(
            service_com_amazonaws_kinesis_kinesis_20131202::EmulatedService::new(Arc::clone(
                &engine,
            )),
        );
        router.register_service(
            service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        router.register_service(
            service_com_amazonaws_sts_awssecuritytokenservicev20110615::EmulatedService::new(
                engine,
            ),
        );
        Self { router }
    }

    /// A SigV4 `Authorization` whose credential scope names `service`, which is what routes the
    /// request. The signature is never checked here.
    fn authorization(service: &str) -> String {
        format!(
            "AWS4-HMAC-SHA256 Credential=AKIDEMULATOR/20260101/{REGION}/{service}/aws4_request, \
             SignedHeaders=host, Signature=not-verified"
        )
    }

    /// One Kinesis call (`awsJson1_1`, `Kinesis_20131202.<Operation>`), as `(status, parsed
    /// body)`.
    async fn kinesis(&self, operation: &str, body: &Value) -> (u16, Value) {
        let host = format!("kinesis.{REGION}.amazonaws.com");
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", host)
            .header("content-type", "application/x-amz-json-1.1")
            .header(
                "x-amz-target",
                format!("{KINESIS_TARGET_PREFIX}.{operation}"),
            )
            .header("authorization", Self::authorization("kinesis"))
            .body(Bytes::from(body.to_string()))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        (status, parsed_body(response.body()))
    }

    /// One Kinesis call that must be served.
    async fn kinesis_ok(&self, case: &str, operation: &str, body: &Value) -> Value {
        let (status, answered) = self.kinesis(operation, body).await;
        assert_eq!(
            status, PERFORMED,
            "case {case:?}: expected HTTP 200 from {operation}, got {status} with {answered}"
        );
        answered
    }

    /// One awsQuery call to IAM that must be served.
    async fn iam(&self, case: &str, action: &str, params: &[(&str, String)]) {
        let mut body = format!("Action={action}&Version=2010-05-08");
        for (name, value) in params {
            body.push('&');
            body.push_str(name);
            body.push('=');
            body.push_str(&form_encode(value));
        }
        let request = http::Request::builder()
            .method("POST")
            .uri("https://iam.amazonaws.com/")
            .header("host", "iam.amazonaws.com")
            .header("content-type", "application/x-www-form-urlencoded")
            .header("authorization", Self::authorization("iam"))
            .body(Bytes::from(body))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let answered = String::from_utf8_lossy(response.body()).to_string();
        assert_eq!(
            status, PERFORMED,
            "case {case:?}: IAM {action} must succeed, got {status} {answered}"
        );
    }

    /// One API Gateway v2 control-plane call (`restJson1` under `/v2`), as `(status, parsed
    /// body)`.
    async fn control(&self, method: &str, uri: &str, body: Option<Value>) -> (u16, Value) {
        let payload = match &body {
            Some(value) => Bytes::from(value.to_string()),
            None => Bytes::new(),
        };
        let host = format!("apigateway.{REGION}.amazonaws.com");
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://{host}{uri}"))
            .header("host", host)
            .header("content-type", "application/json")
            .header("authorization", Self::authorization("apigateway"))
            .body(payload)
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        (status, parsed_body(response.body()))
    }

    /// The role the integration's `CredentialsArn` names: trusted by API Gateway and holding the
    /// put as its one inline identity policy, written with IAM's own operations. Answers its
    /// ARN.
    async fn role(&self, case: &str, name: &str) -> String {
        let trust = json!({
            "Version": "2012-10-17",
            "Statement": [{
                "Effect": "Allow",
                "Principal": { "Service": GATEWAY_PRINCIPAL },
                "Action": "sts:AssumeRole",
            }],
        })
        .to_string();
        self.iam(
            case,
            "CreateRole",
            &[
                ("RoleName", name.to_string()),
                ("AssumeRolePolicyDocument", trust),
            ],
        )
        .await;
        let document = json!({
            "Version": "2012-10-17",
            "Statement": [{ "Effect": "Allow", "Action": PUT_RECORD_ACTION, "Resource": "*" }],
        })
        .to_string();
        self.iam(
            case,
            "PutRolePolicy",
            &[
                ("RoleName", name.to_string()),
                ("PolicyName", INLINE_POLICY.to_string()),
                ("PolicyDocument", document),
            ],
        )
        .await;
        format!("arn:aws:iam::{}:role/{name}", engine::emulator_account_id())
    }

    /// A PROVISIONED stream of [`SHARD_COUNT`] shards — the capacity mode whose records are
    /// placed by the hash a request names, rather than by the service-managed distribution an
    /// `AUTO` strategy applies (`PutRecordInput$PartitionKey`) — polled until its create has
    /// fallen due.
    ///
    /// The wait is for the CONDITION: the summary is re-read until the status is `ACTIVE`, or
    /// until a budget far beyond any span this emulator owes the transition has passed.
    async fn stream(&self, case: &str, name: &str) {
        self.kinesis_ok(
            case,
            "CreateStream",
            &json!({ "StreamName": name, "ShardCount": SHARD_COUNT }),
        )
        .await;
        let deadline = Instant::now() + SETTLE_BUDGET;
        loop {
            let described = self
                .kinesis_ok(case, "DescribeStreamSummary", &json!({ "StreamName": name }))
                .await;
            if described["StreamDescriptionSummary"]["StreamStatus"] == json!(STREAM_ACTIVE) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "case {case:?}: expected {name} to reach {STREAM_ACTIVE}, got {described}"
            );
            tokio::time::sleep(LOOK_INTERVAL).await;
        }
    }

    /// An HTTP api carrying ONE `Kinesis-PutRecord` integration on [`ROUTE_KEY`], mapping
    /// `parameters` and performed under `role_arn`, deployed to the `$default` stage with
    /// auto-deploy on; answers the address its `ApiEndpoint` announces.
    async fn api(&self, case: &str, role_arn: &str, parameters: Value) -> String {
        let (status, api) = self
            .control(
                "POST",
                "/v2/apis",
                Some(json!({ "name": "placement-api", "protocolType": "HTTP" })),
            )
            .await;
        assert_eq!(
            status, CREATED,
            "case {case:?}: CreateApi must answer {CREATED}, got {status} {api}"
        );
        let api_id = text(case, &api, "apiId");
        let endpoint = text(case, &api, "apiEndpoint");
        let (status, created) = self
            .control(
                "POST",
                &format!("/v2/apis/{api_id}/integrations"),
                Some(json!({
                    "integrationType": "AWS_PROXY",
                    "integrationSubtype": PUT_RECORD_SUBTYPE,
                    "payloadFormatVersion": "1.0",
                    "credentialsArn": role_arn,
                    "requestParameters": parameters,
                })),
            )
            .await;
        assert_eq!(
            status, CREATED,
            "case {case:?}: CreateIntegration must answer {CREATED}, got {status} {created}"
        );
        let integration_id = text(case, &created, "integrationId");
        let (status, route) = self
            .control(
                "POST",
                &format!("/v2/apis/{api_id}/routes"),
                Some(json!({
                    "routeKey": ROUTE_KEY,
                    "target": format!("integrations/{integration_id}"),
                })),
            )
            .await;
        assert_eq!(
            status, CREATED,
            "case {case:?}: CreateRoute must answer {CREATED}, got {status} {route}"
        );
        let (status, stage) = self
            .control(
                "POST",
                &format!("/v2/apis/{api_id}/stages"),
                Some(json!({ "stageName": "$default", "autoDeploy": true })),
            )
            .await;
        assert_eq!(
            status, CREATED,
            "case {case:?}: CreateStage must answer {CREATED}, got {status} {stage}"
        );
        endpoint
    }

    /// One client request to the route of the deployed stage, carrying `body` — the invoke the
    /// integration is performed for. No credential: the route's `AuthorizationType` is the
    /// default `NONE`.
    ///
    /// Handed straight to the router IN PROCESS, so no socket is opened and nothing leaves this
    /// test: the authority is only the address the request carries.
    async fn invoke(&self, endpoint: &str, body: &Value) -> (u16, Value) {
        let authority = endpoint
            .split_once("://")
            .map_or(endpoint, |(_scheme, rest)| rest)
            .trim_end_matches('/')
            .to_string();
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("https://{authority}{ROUTE_PATH}"))
            .header("host", authority.as_str())
            .header("content-type", "application/json")
            .body(Bytes::from(body.to_string()))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        (status, parsed_body(response.body()))
    }

    /// The SHARD MAP of the stream `name`, as its own `ListShards` publishes it: every shard
    /// with the inclusive bounds of the hash values it holds.
    async fn shard_map(&self, case: &str, name: &str) -> Vec<Shard> {
        let listed = self
            .kinesis_ok(case, "ListShards", &json!({ "StreamName": name }))
            .await;
        let shards = listed["Shards"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            shards.len(),
            usize::try_from(SHARD_COUNT).unwrap_or_default(),
            "case {case:?}: expected the stream's {SHARD_COUNT} shards, got {listed}"
        );
        shards
            .iter()
            .map(|shard| Shard {
                id: text(case, shard, "ShardId"),
                starting: text(case, &shard["HashKeyRange"], "StartingHashKey"),
                ending: text(case, &shard["HashKeyRange"], "EndingHashKey"),
            })
            .collect()
    }

    /// The record the shard `shard_id` of the stream `name` holds AT `sequence_number`, read the
    /// way a client comes back for the record it just put: an iterator at that very position
    /// (`AT_SEQUENCE_NUMBER` — "start reading from the position denoted by a specific sequence
    /// number", `GetShardIteratorInput$ShardIteratorType`). `None` when that shard holds no
    /// record under it.
    async fn record_at(
        &self,
        case: &str,
        name: &str,
        shard_id: &str,
        sequence_number: &str,
    ) -> Option<Value> {
        let iterator = self
            .kinesis_ok(
                case,
                "GetShardIterator",
                &json!({
                    "StreamName": name,
                    "ShardId": shard_id,
                    "ShardIteratorType": "AT_SEQUENCE_NUMBER",
                    "StartingSequenceNumber": sequence_number,
                }),
            )
            .await;
        let read = self
            .kinesis_ok(
                case,
                "GetRecords",
                &json!({ "ShardIterator": iterator["ShardIterator"] }),
            )
            .await;
        read["Records"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .find(|record| record["SequenceNumber"] == json!(sequence_number))
    }

    /// Every record the stream `name` holds, as `(the shard holding it, the record)`, read
    /// through Kinesis' own `ListShards`/`GetShardIterator`/`GetRecords` — so what this answers
    /// is what THAT service holds rather than what the route said.
    async fn records(&self, case: &str, name: &str) -> Vec<(String, Value)> {
        let mut held = Vec::new();
        for shard in self.shard_map(case, name).await {
            let iterator = self
                .kinesis_ok(
                    case,
                    "GetShardIterator",
                    &json!({
                        "StreamName": name,
                        "ShardId": shard.id,
                        "ShardIteratorType": "TRIM_HORIZON",
                    }),
                )
                .await;
            let read = self
                .kinesis_ok(
                    case,
                    "GetRecords",
                    &json!({ "ShardIterator": iterator["ShardIterator"] }),
                )
                .await;
            for record in read["Records"].as_array().cloned().unwrap_or_default() {
                held.push((shard.id.clone(), record));
            }
        }
        held
    }

    /// Put one record through the route and answer `(the shard the action named, the sequence
    /// number it assigned)`, holding the call to the action's own answer.
    async fn put(&self, case: &str, endpoint: &str, body: &Value) -> (String, String) {
        let (status, answered) = self.invoke(endpoint, body).await;
        assert_eq!(
            status, PERFORMED,
            "case {case:?}: expected the put's own answer, got {status} with {answered}"
        );
        (
            text(case, &answered, "ShardId"),
            text(case, &answered, "SequenceNumber"),
        )
    }
}

/// One shard of a stream as its own `ListShards` reports it: its id and the inclusive bounds of
/// the hash values it holds (`Shard$HashKeyRange`).
struct Shard {
    id: String,
    starting: String,
    ending: String,
}

/// One response body as the JSON it is, or the text it is for a body that is not JSON.
fn parsed_body(body: &[u8]) -> Value {
    if body.is_empty() {
        return Value::Null;
    }
    serde_json::from_slice(body)
        .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(body).to_string()))
}

/// The text of the string member `member` of `document`; panics quoting the document.
fn text(case: &str, document: &Value, member: &str) -> String {
    document[member]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a string {member:?} in {document}"))
        .to_string()
}

/// `data` as `Record$Data` carries it: base64, which is how a JSON protocol states a blob — so
/// the record read off the shard is compared against the bytes the client sent rather than
/// against itself. Encoded by the smithy runtime's own coder, which is what that wire uses.
fn as_blob(data: &str) -> Value {
    json!(aws_smithy_types::base64::encode(data.as_bytes()))
}

/// Whether the sequence number `later` is strictly larger than `earlier`.
///
/// `SequenceNumber` is a decimal integer without leading zeros (`^(0|([1-9]\d{0,128}))$`) and
/// wider than any integer type this test holds, so the two are compared the way that form
/// compares: by digit COUNT first, and then digit by digit.
fn larger(later: &str, earlier: &str) -> bool {
    match later.len().cmp(&earlier.len()) {
        Ordering::Equal => later > earlier,
        other => other == Ordering::Greater,
    }
}

/// The mapping a route places its records by: the stream, the record and the key, plus whatever
/// `extra` optional parameters the case adds — each read out of the posted document, which is
/// what makes the request rather than the configuration the thing under test.
fn mapping(stream: &str, extra: &[(&str, &str)]) -> Value {
    let mut parameters = serde_json::Map::new();
    parameters.insert("StreamName".to_string(), json!(stream));
    parameters.insert("Data".to_string(), json!("$request.body.data"));
    parameters.insert("PartitionKey".to_string(), json!("$request.body.pk"));
    for (parameter, expression) in extra {
        parameters.insert((*parameter).to_string(), json!(*expression));
    }
    Value::Object(parameters)
}

/// RULE: a route that maps `ExplicitHashKey` places each record on the ONE shard whose published
/// `HashKeyRange` holds the hash the request carried — and the record read back at the position
/// the action reported carries the mapped data and key.
#[tokio::test]
async fn a_mapped_explicit_hash_key_places_the_record_on_the_shard_whose_range_holds_it() {
    const STREAM: &str = "placed";
    let case = "every shard of the map, on each bound of its own range";
    let estate = Estate::new();
    estate.stream(case, STREAM).await;
    let role = estate.role(case, "apigatewayv2-places-records").await;
    // The hash key is the request's, so one route serves every row below.
    let endpoint = estate
        .api(
            case,
            &role,
            mapping(STREAM, &[("ExplicitHashKey", "$request.body.hash")]),
        )
        .await;
    let shards = estate.shard_map(case, STREAM).await;
    // The values driven are each shard's OWN published bounds: the first hash it holds and the
    // last, which together are every boundary of the key space the stream divides.
    for (position, shard) in shards.iter().enumerate() {
        for (bound, hash) in [("starting", &shard.starting), ("ending", &shard.ending)] {
            let case = format!("the {bound} hash key of shard {position}");
            let case = case.as_str();
            let data = format!("record-for-{}-{bound}", shard.id);
            let partition_key = format!("key-{position}-{bound}");
            let (placed_on, sequence_number) = estate
                .put(
                    case,
                    &endpoint,
                    &json!({ "data": data, "pk": partition_key, "hash": hash }),
                )
                .await;
            assert_eq!(
                placed_on, shard.id,
                "case {case:?}: expected the record on the shard whose range holds {hash:?}, got \
                 {placed_on:?}"
            );
            let record = estate
                .record_at(case, STREAM, &shard.id, &sequence_number)
                .await
                .unwrap_or_else(|| {
                    panic!(
                        "case {case:?}: expected the record at {sequence_number} on {}, got \
                         nothing",
                        shard.id
                    )
                });
            assert_eq!(
                record["Data"],
                as_blob(&data),
                "case {case:?}: expected the mapped data, got {record}"
            );
            assert_eq!(
                record["PartitionKey"],
                json!(partition_key),
                "case {case:?}: expected the mapped partition key, got {record}"
            );
        }
    }
    // Two bounds per shard and nothing else: every put above landed exactly one record.
    let held = estate.records(case, STREAM).await;
    assert_eq!(
        held.len(),
        shards.len() * 2,
        "case {case:?}: expected one record per put, got {held:?}"
    );
}

/// RULE: an optional parameter is carried into the put when the request satisfies the expression
/// mapped onto it and left unsupplied when it does not — so the SAME route places a record by
/// the hash key a request carries and by the partition key's own hash when it carries none, and
/// a mapped `SequenceNumberForOrdering` is carried into a call that is performed all the same.
#[tokio::test]
async fn an_optional_parameter_is_carried_only_when_the_request_satisfies_its_mapping() {
    const STREAM: &str = "ordered";
    let case = "the optional parameters of the put";
    let estate = Estate::new();
    estate.stream(case, STREAM).await;
    let role = estate.role(case, "apigatewayv2-orders-records").await;
    let endpoint = estate
        .api(
            case,
            &role,
            mapping(
                STREAM,
                &[
                    ("ExplicitHashKey", "$request.body.hash"),
                    ("SequenceNumberForOrdering", "$request.body.after"),
                ],
            ),
        )
        .await;
    let shards = estate.shard_map(case, STREAM).await;

    // A request carrying neither optional parameter: the record is placed by its partition key,
    // and "all data records with the same partition key map to the same shard within the
    // stream" — so the same key, twice, is the same shard.
    let by_key = "placed by the partition key alone";
    let (first_shard, first_sequence_number) = estate
        .put(
            by_key,
            &endpoint,
            &json!({ "data": "first", "pk": "a-stable-key" }),
        )
        .await;
    let (again, second_sequence_number) = estate
        .put(
            by_key,
            &endpoint,
            &json!({ "data": "second", "pk": "a-stable-key" }),
        )
        .await;
    assert_eq!(
        again, first_shard,
        "case {by_key:?}: expected one key to map to one shard, got {first_shard:?} then \
         {again:?}"
    );
    assert!(
        shards.iter().any(|shard| shard.id == first_shard),
        "case {by_key:?}: expected a shard of the stream's own map, got {first_shard:?}"
    );
    assert!(
        larger(&second_sequence_number, &first_sequence_number),
        "case {by_key:?}: expected the later put to carry the larger sequence number, got \
         {second_sequence_number} after {first_sequence_number}"
    );

    // The SAME key with a hash key the request carries: the hash decides, so the record lands on
    // a shard the key alone would not have placed it on. The shard chosen is the first of the
    // map that is not the one the key hashes to, read off the map rather than named here.
    let overriding = "the hash key overriding the partition key";
    let elsewhere = shards
        .iter()
        .find(|shard| shard.id != first_shard)
        .expect("a stream of several shards holds one the key did not map to");
    let (placed_on, _sequence_number) = estate
        .put(
            overriding,
            &endpoint,
            &json!({
                "data": "overridden",
                "pk": "a-stable-key",
                "hash": elsewhere.starting,
            }),
        )
        .await;
    assert_eq!(
        placed_on, elsewhere.id,
        "case {overriding:?}: expected the hash key to decide the shard, got {placed_on:?}"
    );

    // A request carrying the ordering parameter as the documented usage states it — "set the
    // SequenceNumberForOrdering of record n to the sequence number of record n-1"
    // (`PutRecordInput$SequenceNumberForOrdering`) — is performed, and the record it stored is
    // the one the answer names, at a position after the record it was ordered behind.
    let ordered = "the sequence number of the record before it";
    let (ordered_shard, ordered_sequence_number) = estate
        .put(
            ordered,
            &endpoint,
            &json!({
                "data": "after-the-second",
                "pk": "a-stable-key",
                "after": second_sequence_number,
            }),
        )
        .await;
    assert_eq!(
        ordered_shard, first_shard,
        "case {ordered:?}: expected the key's own shard, got {ordered_shard:?}"
    );
    assert!(
        larger(&ordered_sequence_number, &second_sequence_number),
        "case {ordered:?}: expected a sequence number after the one the request named, got \
         {ordered_sequence_number} against {second_sequence_number}"
    );
    let record = estate
        .record_at(ordered, STREAM, &ordered_shard, &ordered_sequence_number)
        .await
        .unwrap_or_else(|| {
            panic!("case {ordered:?}: expected the record at {ordered_sequence_number}, got none")
        });
    assert_eq!(
        record["Data"],
        as_blob("after-the-second"),
        "case {ordered:?}: expected the mapped data, got {record}"
    );

    // Four puts, four records: nothing above was stored twice or dropped.
    let held = estate.records(case, STREAM).await;
    assert_eq!(
        held.len(),
        4,
        "case {case:?}: expected one record per put, got {held:?}"
    );
}

/// RULE: the hash a route maps is the stream api's to judge — a value outside the 128-bit key
/// space is that service's own client refusal, relayed to the caller with its status, error
/// shape and text, and nothing is stored.
#[tokio::test]
async fn a_hash_key_outside_the_key_space_is_the_stream_apis_own_refusal_relayed() {
    const STREAM: &str = "refused";
    // `HashKey` is modelled `^(0|([1-9]\d{0,38}))$`, which admits 39-digit values the 128-bit key
    // space does not hold — so the rows are the first value past that space and the largest the
    // pattern admits at all. The bounds INSIDE the space are driven by the sibling test above,
    // off the shard map's own published values.
    let rows: &[(&str, &str)] = &[
        (
            "the first hash key past the key space",
            "340282366920938463463374607431768211456",
        ),
        (
            "the largest hash key the member's own pattern admits",
            "999999999999999999999999999999999999999",
        ),
    ];
    let estate = Estate::new();
    let case = "a hash key outside the key space";
    estate.stream(case, STREAM).await;
    let role = estate.role(case, "apigatewayv2-refused-records").await;
    let endpoint = estate
        .api(
            case,
            &role,
            mapping(STREAM, &[("ExplicitHashKey", "$request.body.hash")]),
        )
        .await;
    for (case, hash) in rows {
        let (status, answered) = estate
            .invoke(
                &endpoint,
                &json!({ "data": "never-stored", "pk": "a-key", "hash": hash }),
            )
            .await;
        assert_eq!(
            status, ACTION_REFUSED,
            "case {case:?}: expected the stream api's own status, got {status} with {answered}"
        );
        let shape = answered[SHAPE_MEMBER].as_str().unwrap_or_default();
        assert!(
            shape.ends_with(INVALID_ARGUMENT),
            "case {case:?}: expected the {INVALID_ARGUMENT} Kinesis named, got {answered}"
        );
        let detail = answered[DETAIL_MEMBER].as_str().unwrap_or_default();
        assert!(
            detail.contains("ExplicitHashKey") && detail.contains(hash),
            "case {case:?}: expected the service's own text to name the member and the value it \
             was sent, got {answered}"
        );
        let held = estate.records(case, STREAM).await;
        assert!(
            held.is_empty(),
            "case {case:?}: expected a refused put to store nothing, got {held:?}"
        );
    }
}
