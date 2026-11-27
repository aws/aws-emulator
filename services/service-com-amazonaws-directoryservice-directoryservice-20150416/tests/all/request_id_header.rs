// SPDX-License-Identifier: CC0-1.0

//! The request id a Directory Service response reports in its header, driven end to end
//! over HTTP.
//!
//! Every error shape this service models carries a `RequestId` member, and a refusal
//! states the id of the call it answers there and at the end of `Message`. AWS reports
//! that same id in the response's own `x-amzn-requestid` header — where an SDK's response
//! metadata reads a failed call's request id from — so a client correlating on the header
//! and one reading the document must see ONE value.
//!
//! The rule is the ENVELOPE's, not any one operation's, so it is asserted over a TABLE of
//! refusals spanning several operations and several of the modelled error shapes, each row
//! differing in which layer produced the refusal. Nothing here reaches into the state
//! struct: the rows go through the same front door a client uses.
//!
//! ## Both declared protocols
//!
//! This service's model declares `awsJson1_1` and `rpcv2Cbor` side by side
//! (`smithy_query(query="service_info",model_path="services/service-com-amazonaws-directoryservice-directoryservice-20150416/smithy-model/com-amazonaws-directoryservice-directoryservice-20150416.json")`),
//! so a client may call every operation over `POST /` with `X-Amz-Target` or over
//! `POST /service/{Service}/operation/{Operation}` with `smithy-protocol: rpc-v2-cbor`, and
//! each request is read and answered in the protocol it arrived in. That makes the rule
//! above one about EVERY declared protocol, and with it the two refusals this service words
//! from the request itself in the same wrapper (`service::list_element_refusals`,
//! `service::model_pattern_refusals`): the last three cases below assert that one request
//! is answered the same way on either route — the same class, the same sentence, the same
//! member path and the id reported in the same header — and that a create made on the CBOR
//! route is a directory both routes then report.

use aws_smithy_schema::codec::{Codec, FinishSerializer};
use aws_smithy_schema::serde::{SerdeError, SerializableStruct, ShapeDeserializer, ShapeSerializer};
use aws_smithy_schema::{Schema, ShapeId, ShapeType};
use aws_smithy_schema_server::{CborCodec, DeserializableStruct};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_directoryservice_directoryservice_20150416::ops::types::{
    CreateDirectoryInput, CreateDirectoryOutput, CreateConditionalForwarderInput,
    DescribeClientAuthenticationSettingsInput, DescribeDirectoriesInput, DescribeDirectoriesOutput,
    DescribeSnapshotsInput, ListTagsForResourceInput, RemoveIpRoutesInput,
    CREATE_CONDITIONAL_FORWARDER_INPUT_SCHEMA, CREATE_DIRECTORY_INPUT_SCHEMA,
    DESCRIBE_CLIENT_AUTHENTICATION_SETTINGS_INPUT_SCHEMA, DESCRIBE_DIRECTORIES_INPUT_SCHEMA,
    DESCRIBE_SNAPSHOTS_INPUT_SCHEMA, LIST_TAGS_FOR_RESOURCE_INPUT_SCHEMA,
    REMOVE_IP_ROUTES_INPUT_SCHEMA,
};

use crate::directory_lifecycle::{assert_directory_id, create_directory, minimal_create, PASSWORD};
use crate::harness::{call_raw, ok, service};

/// The header AWS reports a response's own request id in.
const REQUEST_ID_HEADER: &str = "x-amzn-requestid";

/// One refusal to assert the rule on: the call to make and what it is.
struct HeaderCase {
    case: String,
    operation: &'static str,
    body: Value,
}

#[tokio::test]
async fn a_refusal_reports_the_request_id_it_states_in_the_header_too() {
    let router = service();

    let directory_id =
        create_directory(&router, minimal_create("header.example.com"), "header").await;
    ok(
        &router,
        "CreateLogSubscription",
        json!({ "DirectoryId": &directory_id, "LogGroupName": "/aws/header" }),
        "header: subscribe",
    )
    .await;

    // The rows differ along the operation refused and the error shape it answers with: a
    // directory that does not exist, an entity that already exists, a subscription that
    // does not exist, a token this service did not issue and a resource id that names
    // nothing taggable.
    let cases = vec![
        HeaderCase {
            case: "CreateLogSubscription for an absent directory".to_string(),
            operation: "CreateLogSubscription",
            body: json!({ "DirectoryId": "d-0000000000", "LogGroupName": "nowhere" }),
        },
        HeaderCase {
            case: "CreateLogSubscription for a directory that already has one".to_string(),
            operation: "CreateLogSubscription",
            body: json!({ "DirectoryId": &directory_id, "LogGroupName": "/aws/second" }),
        },
        HeaderCase {
            case: "DeleteLogSubscription for an absent directory".to_string(),
            operation: "DeleteLogSubscription",
            body: json!({ "DirectoryId": "d-0000000000" }),
        },
        HeaderCase {
            case: "ListLogSubscriptions with a token this service did not issue".to_string(),
            operation: "ListLogSubscriptions",
            body: json!({ "NextToken": "not-a-token" }),
        },
        HeaderCase {
            case: "DescribeSnapshots for an absent directory".to_string(),
            operation: "DescribeSnapshots",
            body: json!({ "DirectoryId": "d-0000000000" }),
        },
        HeaderCase {
            case: "ListTagsForResource for an id that is not taggable".to_string(),
            operation: "ListTagsForResource",
            body: json!({ "ResourceId": "d-0000000000" }),
        },
    ];

    let mut seen: Vec<String> = Vec::new();
    for HeaderCase {
        case,
        operation,
        body,
    } in cases
    {
        let response = call_raw(&router, operation, body).await;
        let header = response
            .headers()
            .get(REQUEST_ID_HEADER)
            .map(|value| value.to_str().expect("a textual header").to_string());
        let payload = response.into_body();
        let parsed: Value = serde_json::from_slice(&payload)
            .unwrap_or_else(|_| panic!("case {case:?}: expected a JSON body, got {payload:?}"));
        let stated = parsed["RequestId"].as_str().unwrap_or_else(|| {
            panic!("case {case:?}: expected the modelled RequestId member, got {parsed}")
        });
        assert_eq!(
            header.as_deref(),
            Some(stated),
            "case {case:?}: expected the header to carry the id the document states \
             ({stated}), got {header:?}"
        );
        // The id names ONE call, so no two refusals report the same one.
        assert!(
            !seen.contains(&stated.to_string()),
            "case {case:?}: expected a request id not yet reported, got {stated} after {seen:?}"
        );
        seen.push(stated.to_string());
    }
}

/// The service shape a call on the Smithy RPC v2 CBOR route names in its path: "every
/// request is `POST /service/{ServiceName}/operation/{OperationName}`" (that protocol's
/// specification), and the name is this model's own service shape.
const SERVICE_SHAPE: &str = "DirectoryService_20150416";

/// An id of the modelled `DirectoryId` / `ResourceId` shape (`^d-[0-9a-f]{10}$`) that names
/// no stored directory, so a lookup of it is refused whichever route it arrives on.
const ABSENT_DIRECTORY_ID: &str = "d-0000000000";

/// One of the two wire protocols this service's model declares.
#[derive(Debug, Clone, Copy)]
enum Route {
    /// `POST /` with `X-Amz-Target`, the protocol the sibling suites call on.
    AwsJson,
    /// `POST /service/{Service}/operation/{Operation}` with `smithy-protocol: rpc-v2-cbor`.
    Cbor,
}

impl Route {
    /// The protocol's own name, for the case a failing assertion names.
    fn named(self) -> &'static str {
        match self {
            Self::AwsJson => "awsJson1_1",
            Self::Cbor => "rpcv2Cbor",
        }
    }
}

/// `input` as the request body a CBOR client sends: written through the protocol's own codec
/// against the operation's input schema, so nothing here encodes CBOR by hand.
fn cbor_body<I: SerializableStruct>(input_schema: &Schema, input: &I) -> bytes::Bytes {
    let codec = CborCodec::default();
    let mut serializer = codec.create_serializer();
    serializer
        .write_struct(input_schema, input)
        .expect("the input shape serializes");
    bytes::Bytes::from(serializer.finish())
}

/// One call on the CBOR route, returning the WHOLE response — headers included.
async fn cbor_call_raw(
    router: &MultiServiceRouter,
    operation: &str,
    body: bytes::Bytes,
) -> http::Response<bytes::Bytes> {
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("/service/{SERVICE_SHAPE}/operation/{operation}"))
        .header("host", "ds.us-east-1.amazonaws.com")
        .header("content-type", "application/cbor")
        .header("accept", "application/cbor")
        .header("smithy-protocol", "rpc-v2-cbor")
        // The sigv4 credential scope is what names the `ds` signing name, exactly as it
        // does for this service's awsJson calls.
        .header(
            "authorization",
            "AWS4-HMAC-SHA256 Credential=AKIDDSTEST/20260101/us-east-1/ds/aws4_request, \
             SignedHeaders=host;x-amz-date, Signature=00",
        )
        .body(body)
        .expect("a well-formed rpcv2Cbor request");
    router.dispatch(request).await
}

/// A call on the CBOR route that must succeed, decoded against its output schema.
async fn cbor_ok<O: DeserializableStruct>(
    router: &MultiServiceRouter,
    operation: &str,
    body: bytes::Bytes,
    case: &str,
) -> O {
    let response = cbor_call_raw(router, operation, body).await;
    let status = response.status().as_u16();
    let payload = response.into_body();
    assert_eq!(
        status, 200,
        "case {case:?}: expected {operation} to answer 200 over rpc-v2-cbor, got {status} \
         with body {payload:?}"
    );
    let codec = CborCodec::default();
    let mut deserializer = codec.create_deserializer(&payload);
    O::deserialize(&mut deserializer).unwrap_or_else(|error| {
        panic!(
            "case {case:?}: expected {operation}'s CBOR answer to decode against its output \
             schema, got {error}"
        )
    })
}

/// The class an answer names, as the codec writes that member's key.
static ANSWERED_CLASS_MEMBER: Schema = Schema::new_member(
    ShapeId::from_static(
        "smithy.framework#AnsweredDocument$__type",
        "smithy.framework",
        "__type",
    ),
    ShapeType::String,
    "__type",
    0,
)
.with_json_name("__type");

/// The sentence in the spelling this service's modelled error shapes declare it.
static ANSWERED_MESSAGE_MEMBER: Schema = Schema::new_member(
    ShapeId::from_static(
        "smithy.framework#AnsweredDocument$Message",
        "smithy.framework",
        "Message",
    ),
    ShapeType::String,
    "Message",
    1,
)
.with_json_name("Message");

/// The sentence in the spelling a refusal the framework words carries it under.
static ANSWERED_LOWER_MESSAGE_MEMBER: Schema = Schema::new_member(
    ShapeId::from_static(
        "smithy.framework#AnsweredDocument$message",
        "smithy.framework",
        "message",
    ),
    ShapeType::String,
    "message",
    2,
)
.with_json_name("message");

/// The id the answer states, the member this suite's rule is about.
static ANSWERED_REQUEST_ID_MEMBER: Schema = Schema::new_member(
    ShapeId::from_static(
        "smithy.framework#AnsweredDocument$RequestId",
        "smithy.framework",
        "RequestId",
    ),
    ShapeType::String,
    "RequestId",
    3,
)
.with_json_name("RequestId");

/// The document a client reads an answer of this service out of, as far as this suite reads
/// it. Declared in the `smithy.framework` namespace because it is no shape of the model: it
/// is the members an answer of ANY of the model's error shapes states, named once so one
/// reader serves every row.
static ANSWERED_DOCUMENT_SCHEMA: Schema = Schema::new_struct(
    ShapeId::from_static(
        "smithy.framework#AnsweredDocument",
        "smithy.framework",
        "AnsweredDocument",
    ),
    ShapeType::Structure,
    &[
        &ANSWERED_CLASS_MEMBER,
        &ANSWERED_MESSAGE_MEMBER,
        &ANSWERED_LOWER_MESSAGE_MEMBER,
        &ANSWERED_REQUEST_ID_MEMBER,
    ],
);

/// The text members of an answered document, read with the codec that wrote it.
#[derive(Debug, Default)]
struct AnsweredDocument {
    class: Option<String>,
    message: Option<String>,
    request_id: Option<String>,
}

impl DeserializableStruct for AnsweredDocument {
    fn deserialize(deserializer: &mut dyn ShapeDeserializer) -> Result<Self, SerdeError> {
        let mut document = Self::default();
        deserializer.read_struct(&ANSWERED_DOCUMENT_SCHEMA, &mut |member, reader| {
            match member.member_name() {
                Some("__type") => document.class = Some(reader.read_string(member)?),
                Some("Message" | "message") => {
                    document.message = Some(reader.read_string(member)?);
                }
                Some("RequestId") => document.request_id = Some(reader.read_string(member)?),
                _ => {}
            }
            Ok(())
        })?;
        Ok(document)
    }
}

/// One answer as a client reads it, whichever route it came back on.
#[derive(Debug)]
struct Answered {
    status: u16,
    /// The class the document names under `__type`.
    class: String,
    /// The sentence the document states.
    message: String,
    /// The id the document states in its request-id member, if any.
    stated: Option<String>,
    /// The id the response reports in the header AWS carries it in, if any.
    header: Option<String>,
}

/// The id a response reports in the header AWS carries it in.
fn reported_header(response: &http::Response<bytes::Bytes>) -> Option<String> {
    response
        .headers()
        .get(REQUEST_ID_HEADER)
        .map(|value| value.to_str().expect("a textual header").to_string())
}

/// One answer on the awsJson route, read as a client of that protocol reads it.
fn read_as_json(case: &str, response: http::Response<bytes::Bytes>) -> Answered {
    let status = response.status().as_u16();
    let header = reported_header(&response);
    let payload = response.into_body();
    let document: Value = serde_json::from_slice(&payload)
        .unwrap_or_else(|_| panic!("case {case:?}: expected a JSON document, got {payload:?}"));
    Answered {
        status,
        class: document["__type"].as_str().unwrap_or_default().to_string(),
        message: document["Message"]
            .as_str()
            .or_else(|| document["message"].as_str())
            .unwrap_or_default()
            .to_string(),
        stated: document["RequestId"].as_str().map(str::to_string),
        header,
    }
}

/// The same answer on the CBOR route, read with that protocol's own codec.
fn read_as_cbor(case: &str, response: http::Response<bytes::Bytes>) -> Answered {
    let status = response.status().as_u16();
    let header = reported_header(&response);
    let payload = response.into_body();
    let codec = CborCodec::default();
    let mut deserializer = codec.create_deserializer(&payload);
    let document = AnsweredDocument::deserialize(&mut deserializer).unwrap_or_else(|error| {
        panic!("case {case:?}: expected a CBOR document, got {error} for {payload:?}")
    });
    Answered {
        status,
        class: document.class.unwrap_or_default(),
        message: document.message.unwrap_or_default(),
        stated: document.request_id,
        header,
    }
}

/// `operation` called with the same request on both declared routes, read back the way a
/// client of each reads it.
async fn answered_on_both(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    over_cbor: bytes::Bytes,
    over_json: Value,
) -> [(Route, Answered); 2] {
    let cbor = read_as_cbor(case, cbor_call_raw(router, operation, over_cbor).await);
    let json = read_as_json(case, call_raw(router, operation, over_json).await);
    [(Route::Cbor, cbor), (Route::AwsJson, json)]
}

/// One refusal asserted on BOTH declared protocols: the same call, written for each route.
struct BothRoutes {
    /// What the row is, named in every assertion of it.
    case: &'static str,
    operation: &'static str,
    /// The modelled error shape the `__type` of either answer must name.
    class: &'static str,
    cbor: bytes::Bytes,
    json: Value,
}

/// The refusals the rule is asserted on, differing in the operation refused, the modelled
/// error shape it answers with and which diagnostic family worded it: a listing filtered by
/// a directory that does not exist, an id that names nothing taggable (whose family restates
/// the request id inside its own sentence), a read of a directory that does not exist, and a
/// page token this service did not issue.
fn both_route_rows() -> Vec<BothRoutes> {
    vec![
        BothRoutes {
            case: "a snapshot listing filtered by a directory that does not exist",
            operation: "DescribeSnapshots",
            class: "EntityDoesNotExistException",
            cbor: cbor_body(
                DESCRIBE_SNAPSHOTS_INPUT_SCHEMA,
                &DescribeSnapshotsInput {
                    directory_id: Some(ABSENT_DIRECTORY_ID.to_string()),
                    ..DescribeSnapshotsInput::default()
                },
            ),
            json: json!({ "DirectoryId": ABSENT_DIRECTORY_ID }),
        },
        BothRoutes {
            case: "a tag listing for an id that is not taggable",
            operation: "ListTagsForResource",
            class: "EntityDoesNotExistException",
            cbor: cbor_body(
                LIST_TAGS_FOR_RESOURCE_INPUT_SCHEMA,
                &ListTagsForResourceInput {
                    resource_id: ABSENT_DIRECTORY_ID.to_string(),
                    ..ListTagsForResourceInput::default()
                },
            ),
            json: json!({ "ResourceId": ABSENT_DIRECTORY_ID }),
        },
        BothRoutes {
            case: "a client-authentication read of a directory that does not exist",
            operation: "DescribeClientAuthenticationSettings",
            class: "DirectoryDoesNotExistException",
            cbor: cbor_body(
                DESCRIBE_CLIENT_AUTHENTICATION_SETTINGS_INPUT_SCHEMA,
                &DescribeClientAuthenticationSettingsInput {
                    directory_id: ABSENT_DIRECTORY_ID.to_string(),
                    ..DescribeClientAuthenticationSettingsInput::default()
                },
            ),
            json: json!({ "DirectoryId": ABSENT_DIRECTORY_ID }),
        },
        BothRoutes {
            case: "a directory listing resumed from a token this service did not issue",
            operation: "DescribeDirectories",
            class: "InvalidNextTokenException",
            cbor: cbor_body(
                DESCRIBE_DIRECTORIES_INPUT_SCHEMA,
                &DescribeDirectoriesInput {
                    next_token: Some("not-a-token-this-service-issued".to_string()),
                    ..DescribeDirectoriesInput::default()
                },
            ),
            json: json!({ "NextToken": "not-a-token-this-service-issued" }),
        },
    ]
}

/// RULE: a refusal reports the id its document states in the header AWS carries it in, on
/// EVERY protocol the model declares — and one request is answered with the same class and
/// the same sentence whichever of them it arrived on, down to the per-call id.
#[tokio::test]
async fn a_refusal_reads_the_same_on_either_declared_protocol() {
    let router = service();

    let mut seen: Vec<String> = Vec::new();
    for row in both_route_rows() {
        let answers = answered_on_both(
            &router,
            row.case,
            row.operation,
            row.cbor.clone(),
            row.json.clone(),
        )
        .await;
        for (route, answered) in &answers {
            let route = route.named();
            assert_eq!(
                answered.status, 400,
                "case {:?} on {route}: expected {} to answer 400, got {}",
                row.case, row.operation, answered.status
            );
            assert!(
                answered.class.contains(row.class),
                "case {:?} on {route}: expected the class to name {}, got {answered:?}",
                row.case,
                row.class
            );
            let stated = answered.stated.clone().unwrap_or_else(|| {
                panic!(
                    "case {:?} on {route}: expected the modelled RequestId member, got \
                     {answered:?}",
                    row.case
                )
            });
            assert_eq!(
                answered.header.as_deref(),
                Some(stated.as_str()),
                "case {:?} on {route}: expected the header to carry the id the document \
                 states ({stated}), got {:?}",
                row.case,
                answered.header
            );
            assert!(
                answered.message.ends_with(&format!(" : RequestId: {stated}")),
                "case {:?} on {route}: expected the sentence to close with the id it \
                 reports, got {:?}",
                row.case,
                answered.message
            );
            // The id names ONE call, so no two answers report the same one.
            assert!(
                !seen.contains(&stated),
                "case {:?} on {route}: expected a request id not yet reported, got {stated} \
                 after {seen:?}",
                row.case
            );
            seen.push(stated);
        }
        // ...and with the per-call ids aligned, the two routes state the SAME sentence: the
        // refusal is the service's, not the protocol's.
        let [(_, cbor), (_, json)] = &answers;
        let aligned = cbor.message.replace(
            cbor.stated.as_deref().unwrap_or_default(),
            json.stated.as_deref().unwrap_or_default(),
        );
        assert_eq!(
            aligned, json.message,
            "case {:?}: expected one sentence on both routes, got {:?} and {:?}",
            row.case, cbor.message, json.message
        );
    }
}

/// One request whose LIST's ELEMENTS break their constraint, written for each route.
struct ElementRefusal {
    case: &'static str,
    operation: &'static str,
    /// The member the clause must be stated at — the list member itself, with no element
    /// position in it.
    member: &'static str,
    cbor: bytes::Bytes,
    json: Value,
}

/// The rows the element rule is asserted on, differing in the member that carries the list,
/// how many of its entries offend and whether the operation's own input or a model shape it
/// targets declares the element constraint.
fn element_refusal_rows() -> Vec<ElementRefusal> {
    vec![
        ElementRefusal {
            case: "one offending address block in a removal's defaulted list",
            operation: "RemoveIpRoutes",
            member: "cidrIps",
            cbor: cbor_body(
                REMOVE_IP_ROUTES_INPUT_SCHEMA,
                &RemoveIpRoutesInput {
                    directory_id: ABSENT_DIRECTORY_ID.to_string(),
                    cidr_ips: Some(vec!["198.51.100.0".to_string()]),
                    ..RemoveIpRoutesInput::default()
                },
            ),
            json: json!({ "DirectoryId": ABSENT_DIRECTORY_ID, "CidrIps": ["198.51.100.0"] }),
        },
        ElementRefusal {
            case: "a well-formed DNS address beside an ill-formed one",
            operation: "CreateConditionalForwarder",
            member: "dnsIpAddrs",
            cbor: cbor_body(
                CREATE_CONDITIONAL_FORWARDER_INPUT_SCHEMA,
                &CreateConditionalForwarderInput {
                    directory_id: ABSENT_DIRECTORY_ID.to_string(),
                    remote_domain_name: "remote.example.com".to_string(),
                    dns_ip_addrs: Some(vec!["10.0.0.1".to_string(), "10.0.0".to_string()]),
                    ..CreateConditionalForwarderInput::default()
                },
            ),
            json: json!({
                "DirectoryId": ABSENT_DIRECTORY_ID,
                "RemoteDomainName": "remote.example.com",
                "DnsIpAddrs": ["10.0.0.1", "10.0.0"],
            }),
        },
        ElementRefusal {
            case: "an id that is no directory id in a listing's filter",
            operation: "DescribeDirectories",
            member: "directoryIds",
            cbor: cbor_body(
                DESCRIBE_DIRECTORIES_INPUT_SCHEMA,
                &DescribeDirectoriesInput {
                    directory_ids: Some(vec!["not-a-directory-id".to_string()]),
                    ..DescribeDirectoriesInput::default()
                },
            ),
            json: json!({ "DirectoryIds": ["not-a-directory-id"] }),
        },
    ]
}

/// RULE: a constraint a list's ELEMENTS break is stated at the list MEMBER, quoting the
/// whole list the request sent, on EVERY protocol the model declares — the refusals this
/// service words from the request itself are collected with the codec that request arrived
/// in, so neither route falls back to the framework's per-element clause.
#[tokio::test]
async fn a_list_element_refusal_is_stated_at_the_member_on_either_declared_protocol() {
    let router = service();

    for row in element_refusal_rows() {
        let answers = answered_on_both(
            &router,
            row.case,
            row.operation,
            row.cbor.clone(),
            row.json.clone(),
        )
        .await;
        for (route, answered) in &answers {
            let route = route.named();
            assert_eq!(
                answered.status, 400,
                "case {:?} on {route}: expected the element constraint to refuse the \
                 request, got {} with {answered:?}",
                row.case, answered.status
            );
            assert_eq!(
                answered.class, "ValidationException",
                "case {:?} on {route}: expected the framework's constraint refusal, got \
                 {answered:?}",
                row.case
            );
            assert!(
                answered.message.contains(&format!("at '{}'", row.member)),
                "case {:?} on {route}: expected the clause stated at the list member, got \
                 {:?}",
                row.case,
                answered.message
            );
            // ...at the member alone: an element POSITION in the path would be the
            // framework's own clause rather than this service's.
            assert!(
                !answered.message.contains(&format!("{}.", row.member)),
                "case {:?} on {route}: expected no element position in the member path, got \
                 {:?}",
                row.case,
                answered.message
            );
            assert!(
                answered.message.contains("Member must satisfy constraint: ["),
                "case {:?} on {route}: expected the element shape's own constraint set \
                 nested in the clause, got {:?}",
                row.case,
                answered.message
            );
        }
        let [(_, cbor), (_, json)] = &answers;
        assert_eq!(
            cbor.message, json.message,
            "case {:?}: expected one sentence on both routes, got {:?} and {:?}",
            row.case, cbor.message, json.message
        );
    }
}

/// `CreateDirectory` on `route`, answering the id it reports.
async fn create_on(router: &MultiServiceRouter, route: Route, name: &str, case: &str) -> String {
    match route {
        Route::AwsJson => create_directory(router, minimal_create(name), case).await,
        Route::Cbor => {
            let created: CreateDirectoryOutput = cbor_ok(
                router,
                "CreateDirectory",
                cbor_body(
                    CREATE_DIRECTORY_INPUT_SCHEMA,
                    &CreateDirectoryInput {
                        name: name.to_string(),
                        password: PASSWORD.to_string(),
                        size: "Small".to_string(),
                        ..CreateDirectoryInput::default()
                    },
                ),
                case,
            )
            .await;
            let directory_id = created.directory_id.unwrap_or_else(|| {
                panic!("case {case:?}: expected the create to report a DirectoryId, got none")
            });
            assert_directory_id(&directory_id, case);
            directory_id
        }
    }
}

/// The name `DescribeDirectories` reports for `directory_id` on `route`, or `None` when the
/// account holds no record for it.
async fn described_name_on(
    router: &MultiServiceRouter,
    route: Route,
    directory_id: &str,
    case: &str,
) -> Option<String> {
    match route {
        Route::AwsJson => {
            let answer = ok(
                router,
                "DescribeDirectories",
                json!({ "DirectoryIds": [directory_id] }),
                case,
            )
            .await;
            answer["DirectoryDescriptions"].as_array()?.first()?["Name"]
                .as_str()
                .map(str::to_string)
        }
        Route::Cbor => {
            let described: DescribeDirectoriesOutput = cbor_ok(
                router,
                "DescribeDirectories",
                cbor_body(
                    DESCRIBE_DIRECTORIES_INPUT_SCHEMA,
                    &DescribeDirectoriesInput {
                        directory_ids: Some(vec![directory_id.to_string()]),
                        ..DescribeDirectoriesInput::default()
                    },
                ),
                case,
            )
            .await;
            described.directory_descriptions?.into_iter().next()?.name
        }
    }
}

/// RULE, the success half: an operation is served on every protocol the model declares, and
/// the two are one service over one store — a directory created on either route is the
/// directory both routes then report.
#[tokio::test]
async fn a_directory_created_on_either_declared_protocol_is_reported_by_both() {
    let router = service();

    for created_on in [Route::AwsJson, Route::Cbor] {
        // A name the modelled `Name` pattern admits (`^([a-zA-Z0-9]+[\.-])+([a-zA-Z0-9])+$`),
        // distinct per row so no row reads another's directory.
        let name = match created_on {
            Route::AwsJson => "json-created.example.com",
            Route::Cbor => "cbor-created.example.com",
        };
        let case = format!("a directory created over {}", created_on.named());
        let directory_id = create_on(&router, created_on, name, &case).await;
        for read_on in [Route::AwsJson, Route::Cbor] {
            let case = format!("{case}, read back over {}", read_on.named());
            let reported = described_name_on(&router, read_on, &directory_id, &case).await;
            assert_eq!(
                reported.as_deref(),
                Some(name),
                "case {case:?}: expected the stored directory's name, got {reported:?}"
            );
        }
    }
}
