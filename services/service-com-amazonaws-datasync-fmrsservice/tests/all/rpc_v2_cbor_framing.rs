// SPDX-License-Identifier: CC0-1.0

//! The Smithy RPC v2 CBOR framing of this service, driven end to end over HTTP.
//!
//! The model declares TWO protocols for this service
//! (`smithy_query(query="service_info")` -> `awsJson1_1`, `rpcv2Cbor`), so a client may
//! reach it over either, and which one carries a request is named by the request itself:
//! the awsJson framing posts to `/` with an `X-Amz-Target` header and a JSON body, while
//! the CBOR framing posts to `/service/{Service}/operation/{Operation}`, declares itself
//! with `smithy-protocol: rpc-v2-cbor` and carries a CBOR body and no target header.
//!
//! Both decode to the same modelled input and are served by the same operation over the
//! same store, so a location created over one framing is reported by the other, and a
//! refusal is worded the same way in either — in that framing's own envelope.
//!
//! The CBOR bodies are written and read with the CBOR implementation the framework's own
//! codec is built on, so the bytes a case sends are the bytes a client of that framing
//! sends.

use std::collections::BTreeMap;
use std::sync::Arc;

use aws_smithy_cbor::data::Type;
use aws_smithy_cbor::{Decoder, Encoder};
use smithy_server_runtime::MultiServiceRouter;

/// The `{Service}` label of this service's RPC v2 CBOR paths: the local name of the
/// model's service shape (`FmrsService`), which is also the `X-Amz-Target` prefix of its
/// awsJson framing.
const SERVICE_LABEL: &str = "FmrsService";

/// The header the RPC v2 CBOR specification requires on every request and response of
/// that framing, and the value it carries.
const SMITHY_PROTOCOL: (&str, &str) = ("smithy-protocol", "rpc-v2-cbor");

/// The media type that framing carries its bodies under.
const CBOR_MEDIA_TYPE: &str = "application/cbor";

/// The media type the awsJson1_1 framing carries its bodies under.
const JSON_MEDIA_TYPE: &str = "application/x-amz-json-1.1";

/// The SigV4 credential scope that names the `datasync` signing name this service claims,
/// in either framing.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDDATASYNCTEST/20260101/us-west-2/datasync/aws4_request, \
     SignedHeaders=host, Signature=00";

/// The framing one request is sent in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Framing {
    /// The awsJson1_1 framing: `POST /` with `X-Amz-Target` and a JSON body.
    AwsJson,
    /// Smithy RPC v2 CBOR: the operation in the path, the body as CBOR.
    RpcV2Cbor,
}

/// How a CBOR request writes its container headers. Both forms are CBOR and both are what
/// some client sends, so the cases send each of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Lengths {
    /// Every map and array declares its entry count up front.
    Declared,
    /// Every map and array is opened and closed by a break, as the framework's own
    /// serializer writes them.
    Indefinite,
}

/// A router with just this service mounted on a fresh engine.
fn service() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_datasync_fmrsservice::EmulatedService::new(engine),
    );
    router
}

/// `value` as the CBOR item carrying the same members, with container lengths written as
/// `lengths` says.
fn cbor_bytes(value: &serde_json::Value, lengths: Lengths) -> Vec<u8> {
    fn write(encoder: &mut Encoder, value: &serde_json::Value, lengths: Lengths) {
        match value {
            serde_json::Value::Null => {
                encoder.null();
            }
            serde_json::Value::Bool(flag) => {
                encoder.boolean(*flag);
            }
            serde_json::Value::Number(number) => match number.as_i64() {
                Some(whole) => {
                    encoder.long(whole);
                }
                None => {
                    encoder.double(number.as_f64().unwrap_or_default());
                }
            },
            serde_json::Value::String(text) => {
                encoder.str(text);
            }
            serde_json::Value::Array(elements) => {
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
            serde_json::Value::Object(members) => {
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

/// One value a response body carries, whichever framing carries it: the two framings
/// decode to the same shape, so every assertion below is written once.
///
/// A map's entries are held in KEY order rather than in the order the body wrote them,
/// because the order two codecs write one structure's members in is each codec's own and
/// is not part of the payload.
#[derive(Debug, Clone, PartialEq)]
enum Decoded {
    Text(String),
    Number(f64),
    Flag(bool),
    Bytes(Vec<u8>),
    List(Vec<Decoded>),
    Map(BTreeMap<String, Decoded>),
    Null,
}

impl Decoded {
    /// The value under `key`, for a map.
    fn get(&self, key: &str) -> Option<&Decoded> {
        match self {
            Self::Map(entries) => entries.get(key),
            _ => None,
        }
    }

    /// The text under `key`, or the empty string when the body carries none there — so an
    /// assertion reports what the body held rather than panicking on its absence.
    fn text_at(&self, key: &str) -> &str {
        match self.get(key) {
            Some(Self::Text(text)) => text,
            _ => "",
        }
    }

    /// This body with the member `key` removed, for comparing two answers that agree on
    /// everything but a per-call value.
    fn without(&self, key: &str) -> Decoded {
        match self {
            Self::Map(entries) => Self::Map(
                entries
                    .iter()
                    .filter(|(name, _)| name.as_str() != key)
                    .map(|(name, held)| (name.clone(), held.clone()))
                    .collect(),
            ),
            other => other.clone(),
        }
    }

    /// The error code a refusal names, as a client branches on it: the `__type` without
    /// its namespace.
    fn refusal_code(&self) -> String {
        let raw = self.text_at("__type");
        raw.rsplit(['#', ':']).next().unwrap_or(raw).to_string()
    }
}

/// A JSON response body as the shape above.
fn decoded_json(body: &[u8]) -> Decoded {
    fn convert(value: serde_json::Value) -> Decoded {
        match value {
            serde_json::Value::Null => Decoded::Null,
            serde_json::Value::Bool(flag) => Decoded::Flag(flag),
            serde_json::Value::Number(number) => {
                Decoded::Number(number.as_f64().unwrap_or_default())
            }
            serde_json::Value::String(text) => Decoded::Text(text),
            serde_json::Value::Array(elements) => {
                Decoded::List(elements.into_iter().map(convert).collect())
            }
            serde_json::Value::Object(members) => Decoded::Map(
                members
                    .into_iter()
                    .map(|(key, held)| (key, convert(held)))
                    .collect(),
            ),
        }
    }
    match serde_json::from_slice::<serde_json::Value>(body) {
        Ok(value) => convert(value),
        Err(_) => Decoded::Null,
    }
}

/// A CBOR response body as the shape above, decoded rather than scanned as text, so what
/// is asserted is the WHOLE decoded value a client of that framing reads.
fn decoded_cbor(body: &[u8]) -> Decoded {
    fn read(decoder: &mut Decoder<'_>) -> Decoded {
        let datatype = decoder
            .datatype()
            .unwrap_or_else(|error| panic!("the body ends where an item was expected: {error}"));
        match datatype {
            Type::String | Type::StringIndef => Decoded::Text(
                decoder
                    .string()
                    .unwrap_or_else(|error| panic!("a text item does not read: {error}")),
            ),
            Type::Bytes | Type::BytesIndef => Decoded::Bytes(
                decoder
                    .blob()
                    .unwrap_or_else(|error| panic!("a byte string does not read: {error}"))
                    .into_inner(),
            ),
            Type::Bool => Decoded::Flag(
                decoder
                    .boolean()
                    .unwrap_or_else(|error| panic!("a boolean does not read: {error}")),
            ),
            Type::U8 | Type::U16 | Type::U32 | Type::U64 | Type::I8 | Type::I16 | Type::I32
            | Type::I64 | Type::Int => Decoded::Number(
                decoder
                    .long()
                    .unwrap_or_else(|error| panic!("a whole number does not read: {error}"))
                    as f64,
            ),
            Type::F16 | Type::F32 | Type::F64 => Decoded::Number(
                decoder
                    .double()
                    .unwrap_or_else(|error| panic!("a number does not read: {error}")),
            ),
            Type::Null => {
                decoder.null().expect("a null reads");
                Decoded::Null
            }
            // An `@timestamp` member: the CBOR framing carries it as an instant under the
            // tag that framing reserves for one.
            Type::Tag => Decoded::Number(
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
                Decoded::List(elements)
            }
            Type::Map | Type::MapIndef => {
                let declared = decoder
                    .map()
                    .unwrap_or_else(|error| panic!("a map header does not read: {error}"));
                let mut entries = BTreeMap::new();
                while !at_end(decoder, declared, entries.len()) {
                    let key = decoder
                        .string()
                        .unwrap_or_else(|error| panic!("a map key is not text: {error}"));
                    entries.insert(key, read(decoder));
                }
                end_container(decoder, declared);
                Decoded::Map(entries)
            }
            other => panic!("a response body carries no item of type {other:?}"),
        }
    }

    /// Whether a container that declared `declared` entries (or none, for an indefinite
    /// one) ends here, `read` entries in.
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

/// One answer a call earns: the status, the framing markers the response carries, and the
/// decoded body.
#[derive(Debug, Clone, PartialEq)]
struct Answer {
    status: u16,
    content_type: String,
    smithy_protocol: String,
    body: Decoded,
}

/// Call `operation` in `framing` with the input `body` names, and answer what the service
/// replied.
async fn framed_call(
    router: &MultiServiceRouter,
    framing: Framing,
    lengths: Lengths,
    operation: &str,
    body: serde_json::Value,
) -> Answer {
    let request = match framing {
        Framing::AwsJson => http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", JSON_MEDIA_TYPE)
            .header("x-amz-target", format!("{SERVICE_LABEL}.{operation}"))
            .header("authorization", AUTHORIZATION)
            .body(bytes::Bytes::from(body.to_string()))
            .expect("build the awsJson request"),
        Framing::RpcV2Cbor => http::Request::builder()
            .method("POST")
            .uri(format!("/service/{SERVICE_LABEL}/operation/{operation}"))
            .header(SMITHY_PROTOCOL.0, SMITHY_PROTOCOL.1)
            .header("content-type", CBOR_MEDIA_TYPE)
            .header("accept", CBOR_MEDIA_TYPE)
            .header("authorization", AUTHORIZATION)
            .body(bytes::Bytes::from(cbor_bytes(&body, lengths)))
            .expect("build the rpc-v2-cbor request"),
    };

    let response = router.dispatch(request).await;
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
    let smithy_protocol = header(SMITHY_PROTOCOL.0);
    let payload = response.into_body();
    let body = match framing {
        Framing::AwsJson => decoded_json(&payload),
        Framing::RpcV2Cbor if payload.is_empty() => Decoded::Null,
        Framing::RpcV2Cbor => decoded_cbor(&payload),
    };
    Answer {
        status,
        content_type,
        smithy_protocol,
        body,
    }
}

/// A call in `framing` that must succeed, returning its decoded body.
async fn ok(
    router: &MultiServiceRouter,
    framing: Framing,
    lengths: Lengths,
    operation: &str,
    body: serde_json::Value,
    case: &str,
) -> Decoded {
    let answer = framed_call(router, framing, lengths, operation, body).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected {operation} to answer 200 in {framing:?}, got {answer:?}"
    );
    match framing {
        Framing::AwsJson => {
            assert_eq!(
                answer.content_type, JSON_MEDIA_TYPE,
                "case {case:?}: an awsJson1_1 answer carries that media type, got {answer:?}"
            );
        }
        Framing::RpcV2Cbor => {
            assert_eq!(
                answer.content_type, CBOR_MEDIA_TYPE,
                "case {case:?}: an rpc-v2-cbor answer carries CBOR bytes, got {answer:?}"
            );
            assert_eq!(
                answer.smithy_protocol, SMITHY_PROTOCOL.1,
                "case {case:?}: every rpc-v2-cbor response declares the framing, got {answer:?}"
            );
        }
    }
    answer.body
}

/// A location created in one framing is reported by the other, member for member: the two
/// framings are two encodings of ONE service over ONE store.
///
/// The rows span what the rule generalises over: which framing carries the create, which
/// framing carries the read, which kind of location it is (the two whose members the
/// upstream model last grew), and how the CBOR request writes its container lengths.
#[tokio::test]
async fn a_location_created_in_either_framing_is_reported_by_both() {
    struct Row {
        case: &'static str,
        wrote_in: Framing,
        lengths: Lengths,
        create: &'static str,
        describe: &'static str,
        input: serde_json::Value,
        expected: Vec<(&'static str, &'static str)>,
    }

    let rows = [
        Row {
            case: "an object storage location written over rpc-v2-cbor, lengths declared",
            wrote_in: Framing::RpcV2Cbor,
            lengths: Lengths::Declared,
            create: "CreateLocationObjectStorage",
            describe: "DescribeLocationObjectStorage",
            input: serde_json::json!({
                "ServerHostname": "cbor.storage.example.com",
                "BucketName": "cbor-bucket",
                "AccessKey": "AKIDCBORWRITE",
            }),
            expected: vec![
                ("LocationUri", "object-storage://cbor.storage.example.com/cbor-bucket/"),
                ("AccessKey", "AKIDCBORWRITE"),
                ("ServerProtocol", "HTTPS"),
            ],
        },
        Row {
            case: "an azure blob location written over rpc-v2-cbor, indefinite lengths",
            wrote_in: Framing::RpcV2Cbor,
            lengths: Lengths::Indefinite,
            create: "CreateLocationAzureBlob",
            describe: "DescribeLocationAzureBlob",
            input: serde_json::json!({
                "ContainerUrl": "https://cborstore.blob.core.windows.net/cbor-container",
                "AuthenticationType": "NONE",
                "AccessTier": "COOL",
            }),
            expected: vec![
                (
                    "LocationUri",
                    "azure-blob://cborstore.blob.core.windows.net/cbor-container/",
                ),
                ("AuthenticationType", "NONE"),
                ("AccessTier", "COOL"),
                ("BlobType", "BLOCK"),
            ],
        },
        Row {
            case: "an azure blob location written over awsJson, read over rpc-v2-cbor",
            wrote_in: Framing::AwsJson,
            lengths: Lengths::Declared,
            create: "CreateLocationAzureBlob",
            describe: "DescribeLocationAzureBlob",
            input: serde_json::json!({
                "ContainerUrl": "https://jsonstore.blob.core.windows.net/json-container",
                "AuthenticationType": "OIDC",
                "FederatedIdentity": {
                    "AwsIamRole": "arn:aws:iam::000000000000:role/datasync-federated-identity",
                },
            }),
            expected: vec![
                (
                    "LocationUri",
                    "azure-blob://jsonstore.blob.core.windows.net/json-container/",
                ),
                ("AuthenticationType", "OIDC"),
            ],
        },
    ];

    let router = service();

    for row in &rows {
        let created = ok(
            &router,
            row.wrote_in,
            row.lengths,
            row.create,
            row.input.clone(),
            row.case,
        )
        .await;
        let arn = created.text_at("LocationArn").to_string();
        assert!(
            arn.contains(":datasync:") && arn.contains(":location/loc-"),
            "case {:?}: expected a minted location ARN, got {created:?}",
            row.case
        );

        // Read back in BOTH framings: one store, so the two answers are the same value.
        let read_as_cbor = ok(
            &router,
            Framing::RpcV2Cbor,
            row.lengths,
            row.describe,
            serde_json::json!({ "LocationArn": arn }),
            row.case,
        )
        .await;
        let read_as_json = ok(
            &router,
            Framing::AwsJson,
            row.lengths,
            row.describe,
            serde_json::json!({ "LocationArn": arn }),
            row.case,
        )
        .await;
        // `CreationTime` is an `@timestamp`: the CBOR framing carries it as a tagged
        // instant and the awsJson one as epoch seconds, so the member is compared on its
        // own rather than through the two encodings of it.
        assert_eq!(
            read_as_cbor.without("CreationTime"),
            read_as_json.without("CreationTime"),
            "case {:?}: the two framings must report ONE location, got {read_as_cbor:?} over \
             CBOR and {read_as_json:?} over awsJson",
            row.case
        );
        for body in [&read_as_cbor, &read_as_json] {
            assert!(
                body.get("CreationTime").is_some(),
                "case {:?}: every framing reports the stored creation time, got {body:?}",
                row.case
            );
            assert_eq!(
                body.text_at("LocationArn"),
                arn,
                "case {:?}: the describe must report the ARN it was asked about, got {body:?}",
                row.case
            );
            for (member, expected) in &row.expected {
                assert_eq!(
                    body.text_at(member),
                    *expected,
                    "case {:?}: expected {member} {expected:?}, got {body:?}",
                    row.case
                );
            }
        }
    }
}

/// A refused request is refused the same way in either framing, each in that framing's own
/// error envelope.
///
/// The rows span what the rule generalises over: a refusal the handler words from the
/// members a request combined, and one the framework words for a member outside its
/// modelled constraint.
#[tokio::test]
async fn a_refusal_is_worded_the_same_way_in_either_framing() {
    struct Row {
        case: &'static str,
        operation: &'static str,
        input: serde_json::Value,
        expected_code: &'static str,
    }

    let rows = [
        Row {
            case: "a federated identity beside the long-term keys",
            operation: "CreateLocationObjectStorage",
            input: serde_json::json!({
                "ServerHostname": "refused.storage.example.com",
                "BucketName": "refused-bucket",
                "AccessKey": "AKIDREFUSED",
                "FederatedIdentity": {
                    "AwsIamRole": "arn:aws:iam::000000000000:role/datasync-federated-identity",
                },
            }),
            expected_code: "InvalidRequestException",
        },
        Row {
            case: "a location ARN no location answers to",
            operation: "DescribeLocationObjectStorage",
            input: serde_json::json!({
                "LocationArn":
                    "arn:aws:datasync:us-west-2:000000000000:location/loc-00000000000000000",
            }),
            expected_code: "InvalidRequestException",
        },
    ];

    let router = service();

    for row in &rows {
        let mut answers = Vec::new();
        for framing in [Framing::AwsJson, Framing::RpcV2Cbor] {
            let answer = framed_call(
                &router,
                framing,
                Lengths::Declared,
                row.operation,
                row.input.clone(),
            )
            .await;
            assert_eq!(
                answer.status, 400,
                "case {:?}: expected {} to be refused with 400 in {framing:?}, got {answer:?}",
                row.case, row.operation
            );
            assert_eq!(
                answer.body.refusal_code(),
                row.expected_code,
                "case {:?}: expected a {} in {framing:?}, got {answer:?}",
                row.case,
                row.expected_code
            );
            if framing == Framing::RpcV2Cbor {
                assert_eq!(
                    answer.smithy_protocol, SMITHY_PROTOCOL.1,
                    "case {:?}: an rpc-v2-cbor refusal declares the framing too, got {answer:?}",
                    row.case
                );
            }
            answers.push((framing, answer));
        }

        // One service, one refusal: the message is the same text whichever framing asked.
        let (first_framing, first) = &answers[0];
        let (second_framing, second) = &answers[1];
        assert_eq!(
            first.body.text_at("message"),
            second.body.text_at("message"),
            "case {:?}: {first_framing:?} and {second_framing:?} must word one refusal the same \
             way, got {first:?} and {second:?}",
            row.case
        );
        assert!(
            !first.body.text_at("message").is_empty(),
            "case {:?}: a refusal states why, got {first:?}",
            row.case
        );
    }
}
