// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the SECOND wire protocol this service's model declares, over the
//! wire.
//!
//! RULE: this model declares `rpcv2Cbor` beside `awsJson1_1`
//! (`smithy_query(query="service_info")` reports both), and a service that declares several
//! protocols serves every request in the protocol THAT request names — `POST
//! /service/{Service}/operation/{Operation}` with `smithy-protocol: rpc-v2-cbor` and a CBOR
//! body for the one, `POST /` with `X-Amz-Target` and a JSON body for the other. Both decode
//! to the same modelled input and are served by the same operation over the same store, so:
//!
//! 1. a call served over either wire reads and writes the one store, and a resource written
//!    over one is read back through the other;
//! 2. every answer carries the framing markers of the protocol that carried the call;
//! 3. a REFUSAL is the same class, status and sentence on both wires — including the
//!    refusals this service states for itself in the layers in front of its operations (the
//!    modelled `@pattern`s the boundary validator declares and cannot apply, the collection a
//!    clause about a collection member quotes, and the code the validation in front of the
//!    data endpoint names) — and reports that sentence in the member the model declares.
//!
//! The rows differ along what the rule generalises over: which wire carried the call, how the
//! CBOR request writes its container lengths, which operation and which endpoint it names, and
//! which class of answer it earns.
//!
//! Every request goes through `MultiServiceRouter::dispatch`, the same path the emulator
//! serves in production; the `awsJson1_1` half of the harness is `cloud_map_core`'s.

use aws_smithy_cbor::data::Type;
use aws_smithy_cbor::{Decoder, Encoder};
use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::cloud_map_core::{call, emulator, ok, REGION};

/// The `{Service}` label of this service's `rpcv2Cbor` paths: the local name of the model's
/// service shape (`smithy_query(query="service_info")` -> service
/// `Route53AutoNaming_v20170314`), which is also the `X-Amz-Target` prefix of its `awsJson1_1`
/// framing.
const SERVICE_LABEL: &str = "Route53AutoNaming_v20170314";

/// The header the `rpcv2Cbor` specification requires on every request and response of that
/// protocol, and the value it carries.
const SMITHY_PROTOCOL: (&str, &str) = ("smithy-protocol", "rpc-v2-cbor");

/// The media type that protocol carries its bodies under.
const CBOR_MEDIA_TYPE: &str = "application/cbor";

/// The member this model declares an error's text in, which every error shape it declares
/// carries (`crate::cloud_map_core` reads either spelling, so this suite names it itself).
const MODELLED_MESSAGE_MEMBER: &str = "Message";

/// The member the FRAMEWORK's own refusal renderer writes that text in, which a failure whose
/// class this model does not declare keeps.
const RENDERED_MESSAGE_MEMBER: &str = "message";

/// How a CBOR request writes its container headers. Both forms are CBOR and both are what
/// some client sends, so each case is sent in both.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Lengths {
    /// Every map and array declares its entry count up front.
    Declared,
    /// Every map and array is opened and closed by a break, as the framework's own serializer
    /// writes them.
    Indefinite,
}

impl Lengths {
    /// Both framings, for a case that is sent in each.
    const BOTH: [Self; 2] = [Self::Declared, Self::Indefinite];

    /// How a case names this framing.
    fn case(self) -> &'static str {
        match self {
            Self::Declared => "declared lengths",
            Self::Indefinite => "indefinite lengths",
        }
    }
}

/// One answer a call earns: the status, the framing markers the response carries, and its
/// decoded body.
#[derive(Debug, Clone, PartialEq)]
struct Answer {
    status: u16,
    content_type: String,
    smithy_protocol: String,
    body: Value,
}

/// `value` as the CBOR item carrying the same members, with container lengths written as
/// `lengths` says.
///
/// Encoded with the CBOR implementation the framework's own codec is built on, so the bytes a
/// case sends are the bytes a client of this protocol sends.
fn cbor_bytes(value: &Value, lengths: Lengths) -> Vec<u8> {
    fn write(encoder: &mut Encoder, value: &Value, lengths: Lengths) {
        match value {
            Value::Null => {
                encoder.null();
            }
            Value::Bool(flag) => {
                encoder.boolean(*flag);
            }
            Value::Number(number) => match number.as_i64() {
                Some(whole) => {
                    encoder.long(whole);
                }
                None => {
                    encoder.double(number.as_f64().unwrap_or_default());
                }
            },
            Value::String(text) => {
                encoder.str(text);
            }
            Value::Array(elements) => {
                match lengths {
                    Lengths::Declared => encoder.array(elements.len()),
                    Lengths::Indefinite => encoder.begin_array(),
                };
                for element in elements {
                    write(encoder, element, lengths);
                }
                if lengths == Lengths::Indefinite {
                    encoder.end();
                }
            }
            Value::Object(members) => {
                match lengths {
                    Lengths::Declared => encoder.map(members.len()),
                    Lengths::Indefinite => encoder.begin_map(),
                };
                for (name, held) in members {
                    encoder.str(name);
                    write(encoder, held, lengths);
                }
                if lengths == Lengths::Indefinite {
                    encoder.end();
                }
            }
        }
    }
    let mut encoder = Encoder::new(Vec::new());
    write(&mut encoder, value, lengths);
    encoder.into_writer()
}

/// A CBOR response body as the value a client of this protocol decodes, decoded with the same
/// implementation rather than scanned as bytes — so what an assertion reads is the WHOLE
/// decoded answer.
///
/// Every number is carried as a double, because the two protocols write one number in
/// different wire forms and the form is each codec's own; [`comparable`] takes the other wire's
/// numbers the same way, so the two answers are compared on their values.
fn decoded_cbor(body: &[u8]) -> Value {
    fn read(decoder: &mut Decoder<'_>) -> Value {
        let datatype = decoder
            .datatype()
            .unwrap_or_else(|error| panic!("the body ends where an item was expected: {error}"));
        match datatype {
            Type::String | Type::StringIndef => Value::String(
                decoder
                    .string()
                    .unwrap_or_else(|error| panic!("a text item does not read: {error}")),
            ),
            Type::Bool => Value::Bool(
                decoder
                    .boolean()
                    .unwrap_or_else(|error| panic!("a boolean does not read: {error}")),
            ),
            Type::U8 | Type::U16 | Type::U32 | Type::U64 | Type::I8 | Type::I16 | Type::I32
            | Type::I64 | Type::Int => number(
                decoder
                    .long()
                    .unwrap_or_else(|error| panic!("a whole number does not read: {error}"))
                    as f64,
            ),
            Type::F16 | Type::F32 | Type::F64 => number(
                decoder
                    .double()
                    .unwrap_or_else(|error| panic!("a number does not read: {error}")),
            ),
            Type::Null => {
                decoder.null().expect("a null reads");
                Value::Null
            }
            // A timestamp: this protocol writes one as a TAGGED number of epoch seconds,
            // where the other wire writes the bare number, so it is read back as that
            // number and the two answers compare on the same instant.
            Type::Tag => number(
                decoder
                    .timestamp()
                    .unwrap_or_else(|error| panic!("a timestamp does not read: {error}"))
                    .as_secs_f64(),
            ),
            Type::Array | Type::ArrayIndef => {
                let declared = decoder
                    .list()
                    .unwrap_or_else(|error| panic!("an array header does not read: {error}"));
                let mut elements = Vec::new();
                while !at_end(decoder, declared, elements.len()) {
                    elements.push(read(decoder));
                }
                end_container(decoder, declared);
                Value::Array(elements)
            }
            Type::Map | Type::MapIndef => {
                let declared = decoder
                    .map()
                    .unwrap_or_else(|error| panic!("a map header does not read: {error}"));
                let mut entries = Map::new();
                while !at_end(decoder, declared, entries.len()) {
                    let key = decoder
                        .string()
                        .unwrap_or_else(|error| panic!("a map key is not text: {error}"));
                    entries.insert(key, read(decoder));
                }
                end_container(decoder, declared);
                Value::Object(entries)
            }
            other => panic!("a response body carries no item of type {other:?}"),
        }
    }

    /// Whether a container that declared `declared` entries (or none, for an indefinite one)
    /// ends here, `read` entries in.
    fn at_end(decoder: &mut Decoder<'_>, declared: Option<u64>, read: usize) -> bool {
        match declared {
            Some(entries) => read as u64 == entries,
            None => matches!(
                decoder
                    .datatype()
                    .unwrap_or_else(|error| panic!("a container ends mid-entry: {error}")),
                Type::Break
            ),
        }
    }

    /// Consume the break that terminates an indefinite-length container.
    fn end_container(decoder: &mut Decoder<'_>, declared: Option<u64>) {
        if declared.is_none() {
            decoder
                .skip()
                .unwrap_or_else(|error| panic!("the break of a container does not read: {error}"));
        }
    }

    let mut decoder = Decoder::new(body);
    read(&mut decoder)
}

/// `held` as a JSON number, which every finite double is.
fn number(held: f64) -> Value {
    serde_json::Number::from_f64(held).map_or(Value::Null, Value::Number)
}

/// How far apart two numbers may be and still be the same value: one millisecond.
///
/// The two protocols write a TIMESTAMP with the precision each of their codecs keeps — the
/// CBOR one a tagged number of epoch seconds to the millisecond, the JSON one the bare number
/// to the microsecond — and that precision is the codec's, not part of the payload. Every
/// other number this service answers is whole, so nothing else turns on it.
const SAME_NUMBER: f64 = 0.001;

/// Whether two answers carry the same payload: the same members, the same elements in the
/// same order, and values equal — numbers to within [`SAME_NUMBER`], since the wire FORM and
/// the precision a number travels in are each protocol's own.
fn agree(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::Number(held), Value::Number(other)) => {
            (held.as_f64().unwrap_or_default() - other.as_f64().unwrap_or_default()).abs()
                <= SAME_NUMBER
        }
        (Value::Array(held), Value::Array(other)) => {
            held.len() == other.len()
                && held
                    .iter()
                    .zip(other.iter())
                    .all(|(element, peer)| agree(element, peer))
        }
        (Value::Object(held), Value::Object(other)) => {
            held.len() == other.len()
                && held.iter().all(|(name, value)| {
                    other.get(name).is_some_and(|peer| agree(value, peer))
                })
        }
        _ => left == right,
    }
}

/// Call `operation` over this service's `rpcv2Cbor` protocol with the input `body` names, and
/// answer what the endpoint replied: the operation in the request PATH, the body as CBOR, no
/// `X-Amz-Target`, which is what a client of that protocol sends.
async fn cbor_call(
    router: &MultiServiceRouter,
    lengths: Lengths,
    operation: &str,
    body: Value,
) -> Answer {
    // The SigV4 credential scope is the routing signal that names this service's signing
    // name, in either protocol.
    let authorization = format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEMULATOR/20260101/{REGION}/servicediscovery/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature=unsigned"
    );
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("/service/{SERVICE_LABEL}/operation/{operation}"))
        .header(SMITHY_PROTOCOL.0, SMITHY_PROTOCOL.1)
        .header("content-type", CBOR_MEDIA_TYPE)
        .header("accept", CBOR_MEDIA_TYPE)
        .header("authorization", authorization)
        .body(bytes::Bytes::from(cbor_bytes(&body, lengths)))
        .expect("a well-formed rpc-v2-cbor request");
    let response = router.dispatch(request).await;
    let header = |name: &str| {
        response
            .headers()
            .get(name)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string()
    };
    let content_type = header("content-type");
    let smithy_protocol = header(SMITHY_PROTOCOL.0);
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        decoded_cbor(&bytes)
    };
    Answer {
        status,
        content_type,
        smithy_protocol,
        body,
    }
}

/// [`cbor_call`] for a call that must succeed, asserting the framing every answer of this
/// protocol carries and returning the decoded output.
async fn cbor_ok(
    router: &MultiServiceRouter,
    case: &str,
    lengths: Lengths,
    operation: &str,
    body: Value,
) -> Value {
    let answer = cbor_call(router, lengths, operation, body).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: {operation} must succeed over rpc-v2-cbor, got {} with {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        (
            answer.content_type.as_str(),
            answer.smithy_protocol.as_str()
        ),
        (CBOR_MEDIA_TYPE, SMITHY_PROTOCOL.1),
        "case {case:?}: {operation} must declare this protocol's framing, got {answer:?}"
    );
    answer.body
}

/// The text one refusal carries, and the member it carried it in.
fn stated_text(body: &Value) -> (Option<String>, Option<&'static str>) {
    for member in [MODELLED_MESSAGE_MEMBER, RENDERED_MESSAGE_MEMBER] {
        if let Some(text) = body.get(member).and_then(Value::as_str) {
            return (Some(text.to_string()), Some(member));
        }
    }
    (None, None)
}

/// The error code a refusal names, as a client branches on it: the `__type` without its
/// namespace.
fn refusal_code(body: &Value) -> String {
    let raw = body.get("__type").and_then(Value::as_str).unwrap_or_default();
    raw.rsplit('#').next().unwrap_or(raw).to_string()
}

/// How one wire refused a call: the status, the class, the text and the member it is in.
#[derive(Debug, Clone, PartialEq)]
struct Refused {
    status: u16,
    code: String,
    text: Option<String>,
    member: Option<&'static str>,
}

/// The refusal `operation` earns over `wire`, where `None` for `lengths` is the `awsJson1_1`
/// wire and `Some(lengths)` the `rpcv2Cbor` one.
async fn refused(
    router: &MultiServiceRouter,
    lengths: Option<Lengths>,
    operation: &str,
    body: Value,
) -> Refused {
    let (status, document) = match lengths {
        None => call(router, operation, body).await,
        Some(lengths) => {
            let answer = cbor_call(router, lengths, operation, body).await;
            (answer.status, answer.body)
        }
    };
    let (text, member) = stated_text(&document);
    Refused {
        status,
        code: refusal_code(&document),
        text,
        member,
    }
}

/// RULE 1 and 2: the core loop served over `rpcv2Cbor` writes the one store this service's
/// other wire reads, every answer of it carries that protocol's framing, and a resource
/// written over either wire is read back through the other.
#[tokio::test]
async fn the_core_loop_over_rpcv2_cbor_writes_the_store_the_other_wire_reads() {
    let router = emulator();

    for lengths in Lengths::BOTH {
        let case = lengths.case();
        let namespace_name = format!("cbor-ns-{}", case.replace(' ', "-"));
        let service_name = format!("cbor-svc-{}", case.replace(' ', "-"));

        // The create and the operation that resolves it, both over CBOR.
        let created = cbor_ok(
            &router,
            case,
            lengths,
            "CreateHttpNamespace",
            json!({ "Name": namespace_name, "Description": "written over cbor" }),
        )
        .await;
        let operation_id = created["OperationId"]
            .as_str()
            .expect("CreateHttpNamespace returns an OperationId")
            .to_string();
        let filed = cbor_ok(
            &router,
            case,
            lengths,
            "GetOperation",
            json!({ "OperationId": operation_id }),
        )
        .await;
        let namespace_id = filed["Operation"]["Targets"]["NAMESPACE"]
            .as_str()
            .expect("the CREATE_NAMESPACE operation targets the namespace it created")
            .to_string();

        // A service in it, and an instance in the service — the write whose members include a
        // map, which each codec carries in its own form.
        let service = cbor_ok(
            &router,
            case,
            lengths,
            "CreateService",
            json!({ "Name": service_name, "NamespaceId": namespace_id }),
        )
        .await;
        let service_id = service["Service"]["Id"]
            .as_str()
            .expect("CreateService returns the service it stored")
            .to_string();
        cbor_ok(
            &router,
            case,
            lengths,
            "RegisterInstance",
            json!({
                "ServiceId": service_id,
                "InstanceId": "i-over-cbor",
                "Attributes": { "AWS_INSTANCE_IPV4": "192.0.2.7", "tier": "web" },
            }),
        )
        .await;

        // Every read answers the same payload on both wires, member for member: the two
        // protocols decode to one input and are served by one operation over one store.
        let reads: Vec<(&str, Value)> = vec![
            ("GetNamespace", json!({ "Id": namespace_id })),
            ("GetService", json!({ "Id": service_id })),
            (
                "GetInstance",
                json!({ "ServiceId": service_id, "InstanceId": "i-over-cbor" }),
            ),
            ("ListInstances", json!({ "ServiceId": service_id })),
            (
                "ListServices",
                json!({ "Filters": [{
                    "Name": "NAMESPACE_ID",
                    "Values": [namespace_id],
                    "Condition": "EQ",
                }] }),
            ),
            (
                "ListTagsForResource",
                json!({ "ResourceARN": format!(
                    "arn:aws:servicediscovery:{REGION}:{}:service/{service_id}",
                    engine::EMULATOR_ACCOUNT_ID
                ) }),
            ),
        ];
        for (operation, request) in reads {
            let as_cbor = cbor_ok(&router, case, lengths, operation, request.clone()).await;
            let as_json = ok(&router, operation, request.clone()).await;
            assert!(
                agree(&as_cbor, &as_json),
                "case {case}: {operation} must answer one payload on both wires, got \
                 {as_cbor} over cbor and {as_json} over awsJson"
            );
        }

        // The data endpoint's own read, over CBOR: it finds the instance the CBOR write
        // registered …
        let discover = json!({
            "NamespaceName": namespace_name,
            "ServiceName": service_name,
            "HealthStatus": "ALL",
        });
        let found = cbor_ok(
            &router,
            case,
            lengths,
            "DiscoverInstances",
            discover.clone(),
        )
        .await;
        assert_eq!(
            found["Instances"]
                .as_array()
                .expect("Instances is a list")
                .len(),
            1,
            "case {case}: the instance written over cbor is discoverable, got {found}"
        );

        // … and a write made over the OTHER wire is discovered through this one, so neither
        // wire holds a store of its own.
        ok(
            &router,
            "RegisterInstance",
            json!({
                "ServiceId": service_id,
                "InstanceId": "i-over-awsjson",
                "Attributes": { "AWS_INSTANCE_IPV4": "192.0.2.8" },
            }),
        )
        .await;
        let found = cbor_ok(&router, case, lengths, "DiscoverInstances", discover).await;
        let mut discovered: Vec<String> = found["Instances"]
            .as_array()
            .expect("Instances is a list")
            .iter()
            .map(|instance| {
                instance["InstanceId"]
                    .as_str()
                    .expect("every summary names its instance")
                    .to_string()
            })
            .collect();
        discovered.sort();
        assert_eq!(
            discovered,
            vec![
                "i-over-awsjson".to_string(),
                "i-over-cbor".to_string()
            ],
            "case {case}: both wires write the one store, got {discovered:?}"
        );
    }
}

/// RULE 3: a refusal is the same class, status and sentence whichever wire carried the call,
/// and reports that sentence in the member the model declares — over the refusal classes this
/// service answers, each of which is stated by a different layer.
#[tokio::test]
async fn each_refusal_is_the_same_class_and_sentence_on_both_wires() {
    let router = emulator();
    let created = ok(
        &router,
        "CreateHttpNamespace",
        json!({ "Name": "both-wires-ns" }),
    )
    .await;
    let filed = ok(
        &router,
        "GetOperation",
        json!({ "OperationId": created["OperationId"].as_str().expect("an operation id") }),
    )
    .await;
    let namespace_id = filed["Operation"]["Targets"]["NAMESPACE"]
        .as_str()
        .expect("the create targets its namespace")
        .to_string();
    let service = ok(
        &router,
        "CreateService",
        json!({ "Name": "both-wires-svc", "NamespaceId": namespace_id }),
    )
    .await;
    let service_id = service["Service"]["Id"]
        .as_str()
        .expect("a service id")
        .to_string();
    ok(
        &router,
        "RegisterInstance",
        json!({ "ServiceId": service_id, "InstanceId": "i-held", "Attributes": { "k": "v" } }),
    )
    .await;

    // A map one entry short of `ServiceAttributesMap`'s declared minimum of 1, and one past
    // its maximum of 30 — the clause a layer of this service quotes the submitted entries in.
    let over_the_map_maximum: Map<String, Value> = (0..31)
        .map(|index| (format!("key{index}"), json!(format!("value{index}"))))
        .collect();

    // (case, operation, the request, the member the refusal states its text in)
    let rows: Vec<(&str, &str, Value, Option<&str>)> = vec![
        (
            "a namespace read of a namespace nothing created, which carries no text at all",
            "GetNamespace",
            json!({ "Id": "ns-nothinghere" }),
            None,
        ),
        (
            "a namespace delete echoing the reference it could not resolve",
            "DeleteNamespace",
            json!({ "Id": "ns-nothinghere" }),
            Some(MODELLED_MESSAGE_MEMBER),
        ),
        (
            "an instance read whose sentence is built from the request",
            "GetInstance",
            json!({ "ServiceId": service_id, "InstanceId": "i-never" }),
            Some(MODELLED_MESSAGE_MEMBER),
        ),
        (
            "a delete of a service that still holds an instance",
            "DeleteService",
            json!({ "Id": service_id }),
            Some(MODELLED_MESSAGE_MEMBER),
        ),
        (
            "a value outside a declared enumerated type",
            "UpdateInstanceCustomHealthStatus",
            json!({ "ServiceId": service_id, "InstanceId": "i-held", "Status": "DEGRADED" }),
            Some(MODELLED_MESSAGE_MEMBER),
        ),
        (
            "a value under a declared range minimum",
            "ListInstances",
            json!({ "ServiceId": service_id, "MaxResults": 0 }),
            Some(MODELLED_MESSAGE_MEMBER),
        ),
        (
            "a required member the request omits",
            "CreateService",
            json!({ "NamespaceId": namespace_id }),
            Some(MODELLED_MESSAGE_MEMBER),
        ),
        (
            // The expression the boundary validator declares and cannot apply: a name
            // carrying the prefix `NamespaceNameHttp`'s look-around reserves.
            "a name breaking the modelled pattern the boundary cannot apply",
            "CreateHttpNamespace",
            json!({ "Name": "arn:aws:iam::000000000000:role/example" }),
            Some(MODELLED_MESSAGE_MEMBER),
        ),
        (
            // The same class on the service name, whose expression bounds its own length
            // with a look-ahead.
            "a service name past the length its own expression bounds",
            "CreateService",
            json!({ "Name": "n".repeat(128), "NamespaceId": namespace_id }),
            Some(MODELLED_MESSAGE_MEMBER),
        ),
        (
            "a map one entry short of a declared collection minimum",
            "UpdateServiceAttributes",
            json!({ "ServiceId": service_id, "Attributes": {} }),
            Some(MODELLED_MESSAGE_MEMBER),
        ),
        (
            "a map past a declared collection maximum, whose entries a clause quotes",
            "UpdateServiceAttributes",
            json!({ "ServiceId": service_id, "Attributes": over_the_map_maximum }),
            Some(MODELLED_MESSAGE_MEMBER),
        ),
        (
            // The operations the model hosts on a prefixed endpoint are refused under the
            // code the generic validation in front of THAT endpoint names, a class this
            // model does not declare — so its text stays in the member that layer wrote it.
            "a bad value on an operation the model hosts on the data endpoint",
            "DiscoverInstances",
            json!({
                "NamespaceName": "both-wires-ns",
                "ServiceName": "both-wires-svc",
                "HealthStatus": "BOGUS",
            }),
            Some(RENDERED_MESSAGE_MEMBER),
        ),
        (
            "a required member the data endpoint's own read omits",
            "DiscoverInstancesRevision",
            json!({ "NamespaceName": "both-wires-ns" }),
            Some(RENDERED_MESSAGE_MEMBER),
        ),
    ];

    for (case, operation, body, expected_member) in rows {
        let over_json = refused(&router, None, operation, body.clone()).await;
        assert_ne!(
            over_json.status, 200,
            "case {case:?} on {operation}: the call must be refused, got {over_json:?}"
        );
        assert_eq!(
            (over_json.member, over_json.text.is_some()),
            (expected_member, expected_member.is_some()),
            "case {case:?} on {operation} over awsJson: expected the text in \
             {expected_member:?}, got {over_json:?}"
        );

        for lengths in Lengths::BOTH {
            let over_cbor = refused(&router, Some(lengths), operation, body.clone()).await;
            assert_eq!(
                over_cbor, over_json,
                "case {case:?} on {operation} with {}: a refusal must be the same class, \
                 status and sentence on both wires, got {over_cbor:?} over cbor and \
                 {over_json:?} over awsJson",
                lengths.case()
            );
        }
    }

    // The refused calls wrote nothing on either wire: the service still holds the one
    // instance, and the namespace the reserved-prefix create named was never stored.
    let read = ok(&router, "GetService", json!({ "Id": service_id })).await;
    assert_eq!(
        read["Service"]["InstanceCount"],
        json!(1),
        "the refused calls changed nothing, got {read}"
    );
    let listed = ok(&router, "ListNamespaces", json!({})).await;
    let names: Vec<&str> = listed["Namespaces"]
        .as_array()
        .expect("Namespaces is a list")
        .iter()
        .map(|entry| entry["Name"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(
        names,
        vec!["both-wires-ns"],
        "a refused create stores no namespace, got {names:?}"
    );
}
