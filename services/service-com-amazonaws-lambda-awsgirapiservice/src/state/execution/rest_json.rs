// SPDX-License-Identifier: CC0-1.0

//! Binding one AWS SDK call to HTTP for a service whose protocol is `restJson1`.
//!
//! A service with an AWS JSON protocol needs nothing of this: its operation travels in a
//! header and its input document IS the request body, so one code path serves every
//! operation ([`super::aws_api`]). A `restJson1` service binds each operation separately —
//! a method, a URI with labels in it, query parameters, headers and a body — and WHICH
//! member goes where is stated by that service's own model, member by member. So this module
//! reads the model: the operation's `@http` trait gives the method and the URI, and each
//! input member's `@httpLabel`, `@httpQuery`, `@httpQueryParams`, `@httpHeader`,
//! `@httpPrefixHeaders` and `@httpPayload` traits say where its value belongs; everything
//! else is a member of the JSON body, under the name `@jsonName` gives it. Reading the
//! answer back is the same walk in reverse, so what the guest reads are the output's own
//! member names whatever the wire called them.
//!
//! A member bound to the WHOLE body whose shape is a `blob` is the one value here that is not
//! text: the body IS its bytes, with nothing of the protocol encoded around them. So such a
//! member travels as the bytes it names in both directions ([`payload_bytes`],
//! [`carried_payload`]) rather than as the text the body happens to decode to, and WHETHER the
//! blob it targets carries `@streaming` is what decides the form the SDK the guest calls hands
//! those bytes over in ([`PayloadBlob`]).
//!
//! Nothing here is a per-operation table: the model is the authority, so every operation the
//! service binds is reachable and none is described twice. An operation the model does not
//! hold, or one whose input the guest filled with a value that cannot be bound, is reported
//! as a call that was not made rather than sent as a request whose shape is guessed at.
//!
//! What bounds this walk is the documents it walks. Both the input the guest sent and the
//! answer the service gave are parsed by `serde_json`, whose own nesting limit caps how deep
//! a parsed document can be, and each is already capped in size by the seam that carries it
//! ([`super::aws_api::MAX_REQUEST_BYTES`], [`super::aws_api::MAX_RESPONSE_BYTES`]); no step
//! below recurses on anything else. The JSON reading and writing are `serde_json`'s and the
//! percent-encoding is `percent-encoding`'s: the only thing written here is which member
//! belongs where, which is what the model states.

use ::std::collections::BTreeMap;
use ::std::sync::OnceLock;

use ::serde_json::{Map, Value};

/// One `restJson1` service, as a call to it is built: the service's own model, and where the
/// model parsed out of it is kept.
#[derive(Debug)]
pub struct RestApi {
    /// The service's Smithy model, as this crate embeds it. It is the SAME file the service's
    /// own crate is generated from, so the wire facts a call is built from cannot drift from
    /// the wire the service serves.
    pub model: &'static str,
    /// The namespace the model's shape ids are read under.
    pub namespace: &'static str,
    /// The parsed model, built once per process on the first call to the service.
    pub parsed: OnceLock<Model>,
}

impl RestApi {
    /// The parsed model, parsing it on first use.
    pub fn model(&self) -> &Model {
        self.parsed.get_or_init(|| Model::parse(self.model, self.namespace))
    }
}

/// The shapes of one service's model, keyed by shape id.
#[derive(Debug, Default)]
pub struct Model {
    /// Every shape the model declares.
    shapes: Map<String, Value>,
    /// The namespace a short shape name is qualified with.
    namespace: String,
}

/// The empty object, for a member list a shape does not declare.
static NO_MEMBERS: OnceLock<Map<String, Value>> = OnceLock::new();

impl Model {
    /// Parse `source`, which is a Smithy model document.
    pub(crate) fn parse(source: &str, namespace: &str) -> Self {
        let shapes = ::serde_json::from_str::<Value>(source)
            .ok()
            .and_then(|document| document.get("shapes").cloned())
            .and_then(|shapes| match shapes {
                Value::Object(shapes) => Some(shapes),
                _ => None,
            })
            .unwrap_or_default();
        Self {
            shapes,
            namespace: namespace.to_string(),
        }
    }

    /// The shape `id` names, whether the id is absolute or this model's own short name.
    pub(crate) fn shape(&self, id: &str) -> Option<&Value> {
        if let Some(shape) = self.shapes.get(id) {
            return Some(shape);
        }
        self.shapes.get(&format!("{}#{id}", self.namespace))
    }

    /// The members of the shape `id` names.
    pub(crate) fn members(&self, id: &str) -> &Map<String, Value> {
        self.shape(id)
            .and_then(|shape| shape.get("members"))
            .and_then(Value::as_object)
            .unwrap_or_else(|| NO_MEMBERS.get_or_init(Map::new))
    }

    /// The type the shape `id` names declares (`structure`, `list`, `map`, …).
    ///
    /// A shape of the PRELUDE (`smithy.api#Integer`) is not declared in a service's own
    /// model at all, so its type is read from its name — which is what the prelude defines
    /// each of those shapes as. Without that a prelude-targeted member would have no type
    /// here, and a number or a boolean would be read back as the text the wire carried it
    /// as.
    pub(crate) fn kind(&self, id: &str) -> &str {
        if let Some(kind) = self.shape(id).and_then(|shape| shape.get("type")) {
            return kind.as_str().unwrap_or_default();
        }
        match id.strip_prefix("smithy.api#") {
            Some("Blob") => "blob",
            Some("Boolean" | "PrimitiveBoolean") => "boolean",
            Some("Byte" | "PrimitiveByte") => "byte",
            Some("Short" | "PrimitiveShort") => "short",
            Some("Integer" | "PrimitiveInteger") => "integer",
            Some("Long" | "PrimitiveLong") => "long",
            Some("Float" | "PrimitiveFloat") => "float",
            Some("Double" | "PrimitiveDouble") => "double",
            Some("Timestamp") => "timestamp",
            Some("Document") => "document",
            Some("String") => "string",
            _ => "",
        }
    }

    /// The operation `name` names, or `None` when this model binds no such operation.
    pub(crate) fn operation(&self, name: &str) -> Option<&Value> {
        let shape = self.shape(name)?;
        (shape.get("type").and_then(Value::as_str) == Some("operation")).then_some(shape)
    }

    /// How many operations this model binds — the one fact that holds of an embedded model
    /// whatever service it is: that it parsed, and carries a surface to bind against.
    pub fn operation_count(&self) -> usize {
        self.shapes
            .values()
            .filter(|shape| shape.get("type").and_then(Value::as_str) == Some("operation"))
            .count()
    }
}

/// The trait of a member that says where its value belongs on the wire.
const LABEL: &str = "smithy.api#httpLabel";
const QUERY: &str = "smithy.api#httpQuery";
const QUERY_PARAMS: &str = "smithy.api#httpQueryParams";
const HEADER: &str = "smithy.api#httpHeader";
const PREFIX_HEADERS: &str = "smithy.api#httpPrefixHeaders";
pub(crate) const PAYLOAD: &str = "smithy.api#httpPayload";
pub(crate) const RESPONSE_CODE: &str = "smithy.api#httpResponseCode";
const JSON_NAME: &str = "smithy.api#jsonName";

/// One member's traits, or the empty set for a member carrying none.
fn traits(member: &Value) -> Option<&Map<String, Value>> {
    member.get("traits").and_then(Value::as_object)
}

/// The value of the trait `name` on `member`, if it carries it.
pub(crate) fn member_trait<'a>(member: &'a Value, name: &str) -> Option<&'a Value> {
    traits(member).and_then(|traits| traits.get(name))
}

/// Whether a member bound to the WHOLE body, whose shape the model declares as `kind`, carries
/// what a JSON BODY carries.
///
/// A structure, a union, a list, a map or a document IS the body: [`read`] hands that member over
/// as the document the body decoded to, so every value inside it travels in the form a body
/// carries one in. Anything else — a blob, text — is the body's own bytes or text, which that
/// member's value then IS.
///
/// Read here, in the module that binds a `restJson1` call, so the seam that reads such an answer
/// back and the plan of the values an SDK materialises out of it ([`super::blob_members`]) cannot
/// disagree about one binding.
pub(crate) fn payload_is_the_body(kind: &str) -> bool {
    matches!(
        kind,
        "structure" | "union" | "list" | "map" | "document"
    )
}

/// The shape a member targets.
pub(crate) fn target(member: &Value) -> &str {
    member
        .get("target")
        .and_then(Value::as_str)
        .unwrap_or_default()
}

/// The name a member travels under in a JSON body: `@jsonName` when it carries one, else the
/// member's own name.
fn wire_name<'a>(name: &'a str, member: &'a Value) -> &'a str {
    member_trait(member, JSON_NAME)
        .and_then(Value::as_str)
        .unwrap_or(name)
}

/// One request, bound.
#[derive(Debug)]
pub struct Bound {
    /// The HTTP method the operation's `@http` trait names.
    pub method: String,
    /// The path and query the request is addressed to.
    pub target: String,
    /// The headers the input's own members bound.
    pub headers: Vec<(String, String)>,
    /// The body, when the operation carries one — the request's own BYTES, because a member
    /// bound to the whole body may be a blob, whose value is not text at all.
    pub body: Vec<u8>,
    /// Whether that body is a JSON document (so the request carries a JSON media type).
    pub json_body: bool,
}

/// WHERE each member of one operation's input belongs on the wire, as far as that is the
/// same for every REST protocol: the method, the addressed path and query, the headers the
/// input bound, and WHICH members are left for the body.
///
/// The HTTP bindings (`@httpLabel`, `@httpQuery`, `@httpQueryParams`, `@httpHeader`,
/// `@httpPrefixHeaders`, `@httpPayload`) are the `smithy.api` ones, so they bind one REST
/// protocol's request the way they bind another's; what differs between protocols is only
/// how the members left over are written as a DOCUMENT, which is each protocol's own
/// ([`bind`] writes the JSON one).
#[derive(Debug)]
pub(crate) struct Parts {
    /// The HTTP method the operation's `@http` trait names.
    pub method: String,
    /// The path and query the request is addressed to.
    pub target: String,
    /// The headers the input's own members bound.
    pub headers: Vec<(String, String)>,
    /// The member bound to the WHOLE body (`@httpPayload`), as its name and the shape it
    /// targets.
    pub payload: Option<(String, String)>,
    /// The members that belong IN the body, each as its name and the shape it targets, in
    /// the order the model declares them.
    pub body: Vec<(String, String)>,
    /// The operation's input shape id, which the body members are read against.
    pub input_shape: String,
}

/// Bind the parts of `operation`'s request that every REST protocol binds the same way.
pub(crate) fn bind_parts(
    model: &Model,
    operation: &str,
    input: &Value,
) -> Result<Parts, String> {
    let shape = model
        .operation(operation)
        .ok_or_else(|| format!("{operation} is not an operation of this service"))?;
    let http = member_trait(shape, "smithy.api#http")
        .ok_or_else(|| format!("{operation} binds no HTTP request"))?;
    let method = http
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or("POST")
        .to_string();
    let uri = http.get("uri").and_then(Value::as_str).unwrap_or("/");
    let input_shape = shape
        .get("input")
        .and_then(|input| input.get("target"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();

    let supplied = input.as_object().cloned().unwrap_or_default();
    let mut labels: BTreeMap<&str, String> = BTreeMap::new();
    let mut query: Vec<(String, String)> = Vec::new();
    let mut headers: Vec<(String, String)> = Vec::new();
    let mut payload: Option<(String, String)> = None;
    let mut body: Vec<(String, String)> = Vec::new();

    for (name, member) in model.members(&input_shape) {
        let Some(value) = supplied.get(name).filter(|value| !value.is_null()) else {
            continue;
        };
        if member_trait(member, LABEL).is_some() {
            labels.insert(name, scalar(value));
        } else if let Some(named) = member_trait(member, QUERY).and_then(Value::as_str) {
            for one in scalars(value) {
                query.push((named.to_string(), one));
            }
        } else if member_trait(member, QUERY_PARAMS).is_some() {
            for (key, one) in value.as_object().into_iter().flatten() {
                query.push((key.clone(), scalar(one)));
            }
        } else if let Some(named) = member_trait(member, HEADER).and_then(Value::as_str) {
            headers.push((named.to_string(), scalars(value).join(", ")));
        } else if let Some(prefix) = member_trait(member, PREFIX_HEADERS).and_then(Value::as_str) {
            for (key, one) in value.as_object().into_iter().flatten() {
                headers.push((format!("{prefix}{key}"), scalar(one)));
            }
        } else if member_trait(member, PAYLOAD).is_some() {
            payload = Some((name.clone(), target(member).to_string()));
        } else {
            body.push((name.clone(), target(member).to_string()));
        }
    }

    let (path, literal) = split_uri(uri, &labels)?;
    let mut rendered = path;
    // The template's OWN query parameters travel exactly as the model writes them; only the
    // values the input bound are encoded, because only those came from the caller.
    let mut parameters: Vec<String> = Vec::new();
    if !literal.is_empty() {
        parameters.push(literal);
    }
    parameters.extend(
        query
            .iter()
            .map(|(name, value)| format!("{}={}", encoded(name), encoded(value))),
    );
    if !parameters.is_empty() {
        rendered.push('?');
        rendered.push_str(&parameters.join("&"));
    }
    Ok(Parts {
        method,
        target: rendered,
        headers,
        payload,
        body,
        input_shape,
    })
}

/// Bind `operation` of `api`, carrying `input` as the operation's input document keyed by its
/// own member names, to the HTTP request the service's model describes.
pub fn bind(api: &RestApi, operation: &str, input: &Value) -> Result<Bound, String> {
    let model = api.model();
    let parts = bind_parts(model, operation, input)?;
    let supplied = input.as_object().cloned().unwrap_or_default();
    let members = model.members(&parts.input_shape);
    let mut document = Map::new();
    for (name, shape) in &parts.body {
        let Some(value) = supplied.get(name) else {
            continue;
        };
        let member = members.get(name).cloned().unwrap_or(Value::Null);
        document.insert(
            wire_name(name, &member).to_string(),
            to_wire(model, shape, value),
        );
    }
    let payload = parts.payload.as_ref().and_then(|(name, shape)| {
        let value = supplied.get(name)?;
        // A payload that is itself a structure travels as that structure's JSON; one
        // that is text or a blob travels as its own bytes.
        Some(match (model.kind(shape), value) {
            // A BLOB bound to the whole body IS the request's bytes, and the SDK the guest
            // calls carries a blob as the base64 text of them ([`super::aws_sdk_guest`]), so
            // those bytes are what travels — a text body the guest wrote as text travels as
            // its own UTF-8, which is what a client sends for one.
            ("blob", Value::String(text)) => (payload_bytes(text), false),
            (_, Value::String(text)) => (text.clone().into_bytes(), false),
            (_, other) => (to_wire(model, shape, other).to_string().into_bytes(), true),
        })
    });
    let (body, json_body) = match payload {
        Some((bytes, json)) => (bytes, json),
        None if document.is_empty() => (Vec::new(), false),
        None => (Value::Object(document).to_string().into_bytes(), true),
    };
    Ok(Bound {
        method: parts.method,
        target: parts.target,
        headers: parts.headers,
        body,
        json_body,
    })
}

/// The bytes a blob bound to the whole body carries, as the SDK the guest calls hands them
/// over: the base64 text of those bytes, which is how every protocol this runtime's SDKs speak
/// carries a blob. Text that is not that encoding at all is carried as its own UTF-8, so a
/// handler writing a text body is never sent bytes it did not write.
fn payload_bytes(text: &str) -> Vec<u8> {
    use ::base64::Engine as _;

    ::base64::engine::general_purpose::STANDARD
        .decode(text)
        .unwrap_or_else(|_not_base64| text.as_bytes().to_vec())
}

/// The output member of one operation that carries the ANSWER's own BYTES: the member bound to
/// the whole response body (`@httpPayload`) whose shape is a `blob`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PayloadBlob {
    /// The member's own name, which is what the answer's document is keyed by.
    pub member: String,
    /// Whether the blob it targets carries `@streaming`.
    ///
    /// This is what decides the FORM the SDK the guest calls hands those bytes over in. A
    /// streaming blob is handed over UNREAD — the caller is given a stream and reads it — so
    /// every SDK answers one as a stream. A blob the model does not mark streaming is a body
    /// the SDK COLLECTS: the JavaScript SDK hands the caller the collected bytes themselves (a
    /// byte array), and the Python SDK still wraps any blob payload member in a
    /// `botocore.response.StreamingBody`, because it reads the payload of every output whose
    /// payload shape is a blob as a stream whatever the shape's traits say.
    ///
    /// Which trait that is, is [`super::blob_members`]'s to say: the seam that leaves a
    /// streaming blob out of its byte plan and this one decide it from one reading.
    pub streaming: bool,
}

/// The payload blob of `operation`'s answer, when its output binds one.
pub fn blob_payload(api: &RestApi, operation: &str) -> Option<PayloadBlob> {
    let model = api.model();
    let shape = model.operation(operation)?;
    let output = shape
        .get("output")
        .and_then(|output| output.get("target"))
        .and_then(Value::as_str)?;
    model.members(output).iter().find_map(|(name, member)| {
        let shape = target(member);
        (member_trait(member, PAYLOAD).is_some() && model.kind(shape) == "blob").then(|| {
            PayloadBlob {
                member: name.clone(),
                streaming: super::blob_members::is_streaming_shape(model.shape(shape)),
            }
        })
    })
}

/// The answer's document with the bytes of its blob payload in place of the text the body
/// decoded to, and the payload those bytes belong to — `None` for an operation whose answer
/// binds no blob payload, or a document that is not one this seam built.
///
/// The bytes travel to the guest as base64, which is how the one call seam carries bytes in
/// either direction, and the SDK the guest calls hands them over in the form that payload's
/// own shape says ([`PayloadBlob::streaming`]).
pub fn carried_payload(
    api: &RestApi,
    operation: &str,
    document: &str,
    bytes: &[u8],
) -> Option<(String, PayloadBlob)> {
    use ::base64::Engine as _;

    let carried = blob_payload(api, operation)?;
    let mut read = ::serde_json::from_str::<Map<String, Value>>(document).ok()?;
    read.insert(
        carried.member.clone(),
        Value::String(::base64::engine::general_purpose::STANDARD.encode(bytes)),
    );
    Some((Value::Object(read).to_string(), carried))
}

/// The output document of one answered call, keyed by the output shape's OWN member names —
/// which is what an SDK client hands a caller, whatever the wire called each one.
pub fn read(
    api: &RestApi,
    operation: &str,
    status: u16,
    headers: &dyn HeaderSource,
    body: &str,
) -> String {
    let model = api.model();
    let Some(shape) = model.operation(operation) else {
        return body.to_string();
    };
    let output_shape = shape
        .get("output")
        .and_then(|output| output.get("target"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    let members = model.members(output_shape);
    if members.is_empty() {
        return "{}".to_string();
    }
    let document: Value = ::serde_json::from_str(body).unwrap_or(Value::Null);
    let mut read = Map::new();
    for (name, member) in members {
        // On the way back only the bindings an ANSWER carries are read: a member's
        // `@httpQuery` or `@httpLabel` trait binds a request, and says nothing about where
        // the answer carries it — which is a member of the body, like any other.
        if let Some(named) = member_trait(member, HEADER).and_then(Value::as_str) {
            if let Some(value) = headers.header(named) {
                read.insert(name.clone(), header_value(model, target(member), &value));
            }
        } else if let Some(prefix) = member_trait(member, PREFIX_HEADERS).and_then(Value::as_str) {
            let mut collected = Map::new();
            for (header, value) in headers.headers_with(prefix) {
                collected.insert(header, Value::String(value));
            }
            if !collected.is_empty() {
                read.insert(name.clone(), Value::Object(collected));
            }
        } else if member_trait(member, RESPONSE_CODE).is_some() {
            read.insert(name.clone(), Value::from(status));
        } else if member_trait(member, PAYLOAD).is_some() {
            let carried = match payload_is_the_body(model.kind(target(member))) {
                true => from_wire(model, target(member), &document),
                false => Value::String(body.to_string()),
            };
            read.insert(name.clone(), carried);
        } else if let Some(carried) = document.get(wire_name(name, member)) {
            read.insert(name.clone(), from_wire(model, target(member), carried));
        }
    }
    Value::Object(read).to_string()
}

/// Where the answer's headers are read from, so this module does not depend on one HTTP
/// type.
pub trait HeaderSource {
    /// The value of the header `name`, if the answer carries it.
    fn header(&self, name: &str) -> Option<String>;
    /// Every header whose name starts with `prefix`, as `(name without the prefix, value)`.
    fn headers_with(&self, prefix: &str) -> Vec<(String, String)>;
}

/// A header-bound value, read as the member's own type: a list member carries a
/// comma-separated list, a number or a boolean its own text.
pub(crate) fn header_value(model: &Model, shape: &str, text: &str) -> Value {
    match model.kind(shape) {
        "list" => Value::Array(
            text.split(',')
                .map(|one| Value::String(one.trim().to_string()))
                .collect(),
        ),
        "integer" | "long" | "short" | "byte" => text
            .parse::<i64>()
            .map_or_else(|_| Value::String(text.to_string()), Value::from),
        "float" | "double" => text
            .parse::<f64>()
            .ok()
            .and_then(::serde_json::Number::from_f64)
            .map_or_else(|| Value::String(text.to_string()), Value::Number),
        "boolean" => match text {
            "true" => Value::Bool(true),
            "false" => Value::Bool(false),
            other => Value::String(other.to_string()),
        },
        _ => Value::String(text.to_string()),
    }
}

/// One value as the text a label, a query parameter or a header carries it as.
pub(crate) fn scalar(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// One value as the texts it carries: a list binds one parameter per element, everything else
/// one.
pub(crate) fn scalars(value: &Value) -> Vec<String> {
    match value {
        Value::Array(elements) => elements.iter().map(scalar).collect(),
        other => vec![scalar(other)],
    }
}

/// The URI with its labels filled in, and the query its own template states, verbatim.
///
/// A greedy label (`{name+}`) carries a path, so its separators are kept; a plain one is a
/// single segment, so every reserved character in it is encoded.
///
/// The template is walked by SPLITTING it rather than by indexing into it: no step here can
/// panic, whatever the model holds and whatever the guest supplied as a label's value.
fn split_uri(uri: &str, labels: &BTreeMap<&str, String>) -> Result<(String, String), String> {
    let (template, query) = uri.split_once('?').map_or((uri, ""), |(a, b)| (a, b));
    let mut path = String::new();
    let mut rest = template;
    loop {
        let Some((literal, after)) = rest.split_once('{') else {
            path.push_str(rest);
            break;
        };
        path.push_str(literal);
        let Some((name, after)) = after.split_once('}') else {
            return Err(format!("the URI template {uri} is malformed"));
        };
        let (name, greedy) = name
            .strip_suffix('+')
            .map_or((name, false), |plain| (plain, true));
        let value = labels
            .get(name)
            .ok_or_else(|| format!("{name} is required to address this operation"))?;
        path.push_str(&if greedy {
            value
                .split('/')
                .map(encoded)
                .collect::<Vec<String>>()
                .join("/")
        } else {
            encoded(value)
        });
        rest = after;
    }
    Ok((path, query.to_string()))
}

/// The characters a value may carry into a URI without being encoded: the RFC 3986
/// unreserved set, which is what an AWS SDK leaves alone. Everything else — a separator, a
/// space, a reserved character — is percent-encoded, including the `/` of a plain label,
/// which is one segment rather than a path.
const UNRESERVED: ::percent_encoding::AsciiSet = ::percent_encoding::NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~');

/// `text` with every character a URI path segment or a query parameter may not carry
/// percent-encoded, which is what an SDK does to a value before it reaches a URI.
fn encoded(text: &str) -> String {
    ::percent_encoding::utf8_percent_encode(text, &UNRESERVED).to_string()
}

/// One value, renamed from the output shape's member names to the names the wire carries.
fn to_wire(model: &Model, shape: &str, value: &Value) -> Value {
    renamed(model, shape, value, true)
}

/// The same walk the other way: from the names the wire carries to the shape's own members.
fn from_wire(model: &Model, shape: &str, value: &Value) -> Value {
    renamed(model, shape, value, false)
}

/// Walk `value` against the shape `shape` names, renaming each structure member between its
/// own name and the name `@jsonName` gives it on the wire.
///
/// A value whose shape this model does not hold is carried as it is: a document member has no
/// shape of its own to rename against.
fn renamed(model: &Model, shape: &str, value: &Value, outbound: bool) -> Value {
    match (model.kind(shape), value) {
        ("structure" | "union", Value::Object(carried)) => {
            let members = model.members(shape);
            let mut built = Map::new();
            for (name, member) in members {
                let wire = wire_name(name, member);
                let (read, write) = if outbound {
                    (name.as_str(), wire)
                } else {
                    (wire, name.as_str())
                };
                if let Some(one) = carried.get(read) {
                    built.insert(
                        write.to_string(),
                        renamed(model, target(member), one, outbound),
                    );
                }
            }
            Value::Object(built)
        }
        ("list" | "set", Value::Array(elements)) => {
            let element = model
                .shape(shape)
                .and_then(|shape| shape.get("member"))
                .map(target)
                .unwrap_or_default()
                .to_string();
            Value::Array(
                elements
                    .iter()
                    .map(|one| renamed(model, &element, one, outbound))
                    .collect(),
            )
        }
        ("map", Value::Object(entries)) => {
            let value_shape = model
                .shape(shape)
                .and_then(|shape| shape.get("value"))
                .map(target)
                .unwrap_or_default()
                .to_string();
            Value::Object(
                entries
                    .iter()
                    .map(|(key, one)| {
                        (key.clone(), renamed(model, &value_shape, one, outbound))
                    })
                    .collect(),
            )
        }
        _ => value.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tiny model standing for the shapes a `restJson1` binding reads: one operation per
    /// class of binding the protocol has.
    const MODEL: &str = r#"{
      "smithy": "2.0",
      "shapes": {
        "probe#Labelled": {
          "type": "operation",
          "input": { "target": "probe#LabelledRequest" },
          "output": { "target": "probe#Thing" },
          "traits": { "smithy.api#http": { "method": "POST", "uri": "/things/{id}/parts/{path+}", "code": 201 } }
        },
        "probe#Queried": {
          "type": "operation",
          "input": { "target": "probe#QueriedRequest" },
          "output": { "target": "probe#Things" },
          "traits": { "smithy.api#http": { "method": "GET", "uri": "/things?mode=all", "code": 200 } }
        },
        "probe#LabelledRequest": {
          "type": "structure",
          "members": {
            "id": { "target": "smithy.api#String", "traits": { "smithy.api#httpLabel": {} } },
            "path": { "target": "smithy.api#String", "traits": { "smithy.api#httpLabel": {} } },
            "note": { "target": "smithy.api#String" },
            "tag": { "target": "smithy.api#String", "traits": { "smithy.api#jsonName": "label" } },
            "trace": { "target": "smithy.api#String", "traits": { "smithy.api#httpHeader": "X-Trace" } }
          }
        },
        "probe#QueriedRequest": {
          "type": "structure",
          "members": {
            "position": { "target": "smithy.api#String", "traits": { "smithy.api#httpQuery": "position" } },
            "limit": { "target": "smithy.api#Integer", "traits": { "smithy.api#httpQuery": "limit" } }
          }
        },
        "probe#Thing": {
          "type": "structure",
          "members": {
            "id": { "target": "smithy.api#String" },
            "tag": { "target": "smithy.api#String", "traits": { "smithy.api#jsonName": "label" } },
            "code": { "target": "smithy.api#Integer", "traits": { "smithy.api#httpResponseCode": {} } },
            "trace": { "target": "smithy.api#String", "traits": { "smithy.api#httpHeader": "X-Trace" } }
          }
        },
        "probe#Things": {
          "type": "structure",
          "members": {
            "items": { "target": "probe#ThingList", "traits": { "smithy.api#jsonName": "item" } },
            "position": { "target": "smithy.api#String", "traits": { "smithy.api#httpQuery": "position" } }
          }
        },
        "probe#ThingList": { "type": "list", "member": { "target": "probe#Thing" } }
      }
    }"#;

    /// The model above, as a service a call is bound against.
    fn api() -> RestApi {
        RestApi {
            model: MODEL,
            namespace: "probe",
            parsed: OnceLock::new(),
        }
    }

    /// The headers of one answer, for the reading below.
    struct Answered(Vec<(String, String)>);

    impl HeaderSource for Answered {
        fn header(&self, name: &str) -> Option<String> {
            self.0
                .iter()
                .find(|(held, _)| held.eq_ignore_ascii_case(name))
                .map(|(_, value)| value.clone())
        }

        fn headers_with(&self, prefix: &str) -> Vec<(String, String)> {
            self.0
                .iter()
                .filter_map(|(held, value)| {
                    held.strip_prefix(prefix)
                        .map(|rest| (rest.to_string(), value.clone()))
                })
                .collect()
        }
    }

    /// Each input member reaches the place its own traits bind it to — one row per binding
    /// the protocol has, plus the row that is refused because a label has no value.
    #[test]
    fn each_input_member_is_bound_where_its_own_traits_say() {
        // One row per binding class: the input the guest sent, and the request it binds to.
        struct Bind {
            case: &'static str,
            /// The operation the call addresses.
            operation: &'static str,
            /// The input document, keyed by the operation's own member names.
            input: Value,
            /// The method and path the binding must produce, without its query.
            expected: &'static str,
            /// The query parameters it must carry, in any order.
            query: &'static [&'static str],
            /// The body it must carry.
            body: &'static str,
        }
        let cases: Vec<Bind> = vec![
            Bind {
                case: "labels, a renamed body member and a header",
                operation: "Labelled",
                input: ::serde_json::json!({
                    "id": "a/b",
                    "path": "one/two",
                    "note": "plain",
                    "tag": "renamed",
                    "trace": "t-1",
                }),
                expected: "POST /things/a%2Fb/parts/one/two",
                query: &[],
                body: r#"{"label":"renamed","note":"plain"}"#,
            },
            Bind {
                case: "query parameters beside the URI's own literal one",
                operation: "Queried",
                input: ::serde_json::json!({ "position": "p 1", "limit": 7 }),
                expected: "GET /things",
                query: &["mode=all", "limit=7", "position=p%201"],
                body: "",
            },
            Bind {
                case: "an input carrying nothing at all",
                operation: "Queried",
                input: ::serde_json::json!({}),
                expected: "GET /things",
                query: &["mode=all"],
                body: "",
            },
        ];
        let api = api();
        for row in &cases {
            let (case, expected, body) = (row.case, row.expected, row.body);
            let bound = bind(&api, row.operation, &row.input)
                .unwrap_or_else(|why| panic!("case {case:?}: expected a bound request, got {why}"));
            // The method and the PATH are fixed; the query's parameters carry no order of
            // their own, so they are compared as the set they are.
            let (path, query) = bound
                .target
                .split_once('?')
                .map_or((bound.target.as_str(), ""), |(path, query)| (path, query));
            let actual = format!("{} {path}", bound.method);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
            let mut carried_query: Vec<&str> =
                query.split('&').filter(|one| !one.is_empty()).collect();
            carried_query.sort_unstable();
            let mut wanted_query: Vec<&str> = row.query.to_vec();
            wanted_query.sort_unstable();
            assert_eq!(
                carried_query, wanted_query,
                "case {case:?}: expected the query {wanted_query:?}, got {carried_query:?}"
            );
            // The body is compared as the DOCUMENT it is: a JSON object's members carry no
            // order, so what the binding has to get right is which members it wrote under
            // which names.
            let carried: Value = if bound.body.is_empty() {
                Value::Null
            } else {
                ::serde_json::from_slice(&bound.body).unwrap_or_else(|why| {
                    panic!(
                        "case {case:?}: expected a JSON body, got {:?} ({why})",
                        String::from_utf8_lossy(&bound.body)
                    )
                })
            };
            let wanted: Value = if body.is_empty() {
                Value::Null
            } else {
                ::serde_json::from_str(body).expect("the row's body is JSON")
            };
            assert_eq!(
                carried, wanted,
                "case {case:?}: expected the body {wanted}, got {carried}"
            );
        }
        let refused = bind(&api, "Labelled", &::serde_json::json!({ "id": "a" }));
        assert!(
            refused.is_err(),
            "case a label with no value: expected the call not to be made, got {refused:?}"
        );
        let unknown = bind(&api, "Frobnicate", &::serde_json::json!({}));
        assert!(
            unknown.is_err(),
            "case an operation the model does not bind: expected no request, got {unknown:?}"
        );
    }

    /// One row of the reading table: an answer, and the document the guest must read out of
    /// it.
    struct Read {
        case: &'static str,
        /// The operation the answer is of.
        operation: &'static str,
        /// The status it answered.
        status: u16,
        /// The headers it carried.
        headers: &'static [(&'static str, &'static str)],
        /// Its body, as the wire carries it.
        body: &'static str,
    }

    /// The answer is read back under the output's OWN member names, whatever the wire called
    /// each one, and the members an answer binds elsewhere are read from there.
    #[test]
    fn the_answer_is_read_under_the_outputs_own_member_names() {
        let api = api();
        let cases: &[(Read, Value)] = &[
            (
                Read {
                    case: "a renamed member, the status and a header",
                    operation: "Labelled",
                    status: 201,
                    headers: &[("x-trace", "t-9")],
                    body: r#"{"id":"abc","label":"renamed"}"#,
                },
                ::serde_json::json!({ "id": "abc", "tag": "renamed", "code": 201, "trace": "t-9" }),
            ),
            (
                Read {
                    case: "a renamed list member, read element by element",
                    operation: "Queried",
                    status: 200,
                    headers: &[],
                    body: r#"{"item":[{"id":"one","label":"first"}],"position":"p"}"#,
                },
                ::serde_json::json!({
                    "items": [{ "id": "one", "tag": "first" }],
                    "position": "p",
                }),
            ),
            (
                Read {
                    case: "an answer carrying none of the output's members",
                    operation: "Queried",
                    status: 200,
                    headers: &[],
                    body: "{}",
                },
                ::serde_json::json!({}),
            ),
        ];
        for (row, expected) in cases {
            let answered = Answered(
                row.headers
                    .iter()
                    .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
                    .collect(),
            );
            let read = read(&api, row.operation, row.status, &answered, row.body);
            let actual: Value = ::serde_json::from_str(&read).expect("the document is JSON");
            assert_eq!(
                actual, *expected,
                "case {:?}: expected {expected}, got {actual}",
                row.case
            );
        }
    }
}
