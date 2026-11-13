// SPDX-License-Identifier: CC0-1.0

//! Calling another emulated service IN-PROCESS: the four wires a resource provider reaches
//! an owning service's own handlers over, and how a refusal it answers is read.
//!
//! Every per-type arm of [`super`] creates, changes or deletes its resource by CALLING the
//! service that owns it, and what differs between them is only the wire that service
//! publishes: an awsJson document ([`call_json_rpc`]), an awsQuery form body
//! ([`call_query`]), a REST-JSON request ([`call_rest_json`]) or a REST-XML one
//! ([`call_rest_xml`]). Each of them ends at the one [`dispatch`] below, which signs the
//! request with the emulator's own credentials — the emulator verifies every request
//! signature, and the credential scope is also what routes the call — and serves it on a task
//! of its own.
//!
//! Reading what the called service ANSWERED lives here too, for the same reason: an awsQuery
//! or REST-XML answer is an XML document ([`xml_element`] and its siblings), and a refusal is
//! the code and the message that service worded ([`refusal_of`], which words the message as
//! that service's own SDK would raise it).
//!
//! Split out of [`super`] unchanged, so that file stays inside the length a change to this
//! repository may touch; the algorithm that makes the calls is still there.

use super::*;

/// Call one awsJson operation of another emulated service in-process and read its
/// answer document. `Err(refusal)` is the service's own refusal: its code, and the message
/// it worded.
///
/// `content_type` is the awsJson version that service's model declares
/// ([`AWS_JSON_1_0`] / [`AWS_JSON_1_1`]), which is what its answers are rendered in.
///
/// A 501 — the owning operation is still one of this emulator's stubs — is a refusal like
/// any other, so a resource of a type this emulator DOES provision is never passed off as
/// created when nothing created it; the type's own entry in [`owner_for`] is what decides
/// whether a create is attempted at all.
pub(crate) async fn call_json_rpc(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    signing_name: &str,
    region: &str,
    content_type: &str,
    target: &str,
    request: &Value,
) -> Result<Value, Refusal> {
    // The call is made under the stack's service role, so what that role is DENIED is not
    // made at all: the operation an awsJson request invokes is the part of its `X-Amz-Target`
    // after the service shape's name ([`super::service_role::refusal`]).
    if let Some(denied) = denied_by_the_service_role(signing_name, target.rsplit('.').next())
    {
        return Err(denied);
    }
    let body = serde_json::to_vec(request).map_err(|error| Refusal {
        code: "SerializationException".to_string(),
        message: error.to_string(),
    })?;
    let headers = vec![
        ("content-type", content_type.to_string()),
        ("x-amz-target", target.to_string()),
    ];
    let (status, _answered, payload) =
        dispatch(ctx, signing_name, region, "POST", "/", headers, body, &[])
            .await
            .map_err(|message| Refusal {
                code: "InternalFailure".to_string(),
                message,
            })?;
    let document: Value = serde_json::from_slice(&payload).unwrap_or(Value::Null);
    if (200..300).contains(&status) {
        return Ok(document);
    }
    Err(refusal_of(signing_name, status, &document))
}

/// The media type an awsQuery request carries: its parameters are a form body, not a
/// document ("the request is sent as `application/x-www-form-urlencoded`"). Read only by
/// [`call_query`], which is the one place this module writes such a body.
pub(super) const FORM_URLENCODED: &str = "application/x-www-form-urlencoded";

/// Call one awsQuery operation of another emulated service in-process and read the XML
/// document it answers with. `Err(refusal)` is the service's own refusal: the
/// `awsQueryError` code it carries, and the message it worded.
///
/// `version` is the API version that service's model declares (`2010-03-31` for Amazon
/// SNS), which an awsQuery request names beside the action. `parameters` are the request's
/// members in the awsQuery spelling — `Attributes.entry.<n>.key`, `Tags.member.<n>.Key` —
/// percent-encoded here, so a value carrying `&` or `=` cannot be read as a second
/// parameter.
///
/// A 501 is a refusal like any other, exactly as it is for [`call_json_rpc`]: the
/// difference between the two is only the wire the called service publishes.
pub(crate) async fn call_query(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    signing_name: &str,
    region: &str,
    version: &str,
    action: &str,
    parameters: &[(String, String)],
) -> Result<String, Refusal> {
    call_query_carrying(ctx, signing_name, region, version, action, parameters, Vec::new())
        .await
}

/// [`call_query`], with HEADERS the caller adds to the request beside the ones the wire needs.
///
/// For a call whose CHAIN matters: the nested-stack provider carries the deployment a call is
/// already inside on the request, because each level is a request of its own and the bound on
/// that chain has to travel with it ([`nested_stack`]). Everything else is [`call_query`]'s.
pub(crate) async fn call_query_carrying(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    signing_name: &str,
    region: &str,
    version: &str,
    action: &str,
    parameters: &[(String, String)],
    carried: Vec<(&'static str, String)>,
) -> Result<String, Refusal> {
    // Under the stack's service role, exactly as [`call_json_rpc`] is: an awsQuery request
    // names the operation it invokes in its `Action` parameter.
    if let Some(denied) = denied_by_the_service_role(signing_name, Some(action)) {
        return Err(denied);
    }
    let mut body = format!("Action={action}&Version={version}");
    for (name, value) in parameters {
        body.push('&');
        body.push_str(&percent_encoded(name));
        body.push('=');
        body.push_str(&percent_encoded(value));
    }
    let mut headers = vec![("content-type", FORM_URLENCODED.to_string())];
    headers.extend(carried);
    let (status, _answered, payload) = dispatch(
        ctx,
        signing_name,
        region,
        "POST",
        "/",
        headers,
        body.into_bytes(),
        &[],
    )
    .await
    .map_err(|message| Refusal {
        code: "InternalFailure".to_string(),
        message,
    })?;
    let document = String::from_utf8_lossy(&payload).into_owned();
    if (200..300).contains(&status) {
        return Ok(document);
    }
    // An awsQuery error answer is `<ErrorResponse><Error><Code/><Message/></Error>…`, and
    // the code is the one the model's `awsQueryError` trait fixes for that error shape
    // (`NotFoundException` -> `NotFound`), which is what a client branches on.
    let code = xml_element(&document, "Code")
        .unwrap_or_else(|| format!("{UNNAMED_CODE_PREFIX}{status}"));
    Err(Refusal {
        message: sdk_message(
            signing_name,
            status,
            &code,
            &xml_element(&document, "Message").unwrap_or_default(),
        ),
        code,
    })
}

/// One value as a single form component, URI path segment or query value: everything
/// outside RFC 3986's unreserved set escaped ([`FORM_VALUE`]), which is what an AWS SDK
/// sends for an awsQuery parameter or a non-greedy `@httpLabel` and what the SigV4
/// canonical request assumes.
///
/// The ONE spelling of that encoding here, so a parameter of an awsQuery call
/// ([`call_query`]) and a name written into a REST URI are escaped identically.
///
/// A value that comes from the TEMPLATE — the name a declaration asks for — therefore
/// reaches the called operation as one value rather than as extra URI structure, so a name
/// outside the called model's own pattern is refused by that operation's validation instead
/// of addressing a different one.
pub(crate) fn percent_encoded(value: &str) -> String {
    ::percent_encoding::utf8_percent_encode(value, FORM_VALUE).to_string()
}

/// Call one REST-JSON operation of another emulated service in-process and read its answer
/// document. `Err(refusal)` is that service's own refusal, exactly as for
/// [`call_json_rpc`].
///
/// A `restJson1` service binds each operation to an HTTP method and a URI rather than to an
/// `X-Amz-Target` header (AWS FIS: `POST /experimentTemplates`,
/// `DELETE /experimentTemplates/{id}`), so the caller passes the binding the called
/// operation's model declares and this writes the request on it. `body` is `None` for an
/// operation whose input is entirely in the URI.
pub(crate) async fn call_rest_json(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    signing_name: &str,
    region: &str,
    method: &'static str,
    uri: &str,
    body: Option<&Value>,
) -> Result<Value, Refusal> {
    rest_json(ctx, signing_name, region, None, method, uri, body).await
}

/// [`call_rest_json`] for a caller that also states WHICH operation of that service the
/// method and URI name.
///
/// The binding alone does not say it — a REST URI is a resource, not an operation — and the
/// diagnostic the SDK of some services appends to a refused call's message NAMES the
/// operation ([`super::SdkDiagnostic::Operation`]). An arm that calls such a service passes
/// the operation its own model names, so the refusal a failed resource reports is the one
/// that service's SDK would raise.
pub(crate) async fn call_rest_json_named(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    signing_name: &str,
    region: &str,
    operation: &str,
    method: &'static str,
    uri: &str,
    body: Option<&Value>,
) -> Result<Value, Refusal> {
    rest_json(ctx, signing_name, region, Some(operation), method, uri, body).await
}

/// The one REST-JSON call both of the two above are: `operation` is the called operation's
/// own name where the caller stated it.
async fn rest_json(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    signing_name: &str,
    region: &str,
    operation: Option<&str>,
    method: &'static str,
    uri: &str,
    body: Option<&Value>,
) -> Result<Value, Refusal> {
    let payload = match body {
        Some(document) => serde_json::to_vec(document).map_err(|error| Refusal {
            code: "SerializationException".to_string(),
            message: error.to_string(),
        })?,
        None => Vec::new(),
    };
    let headers = vec![("content-type", REST_JSON.to_string())];
    let (status, _answered, answer) =
        dispatch(ctx, signing_name, region, method, uri, headers, payload, &[])
            .await
            .map_err(|message| Refusal {
                code: "InternalFailure".to_string(),
                message,
            })?;
    let document: Value = serde_json::from_slice(&answer).unwrap_or(Value::Null);
    if (200..300).contains(&status) {
        return Ok(document);
    }
    Err(refusal_of_call(signing_name, operation, status, &document))
}

/// [`call_rest_json`] for an operation whose input is an `@httpPayload`: the bytes the
/// caller passes ARE the request's body, under the media type that payload is sent as.
///
/// A REST operation that binds a member as its payload takes that member's bytes alone —
/// API Gateway's `ImportRestApi` takes an OpenAPI document that way — so the request cannot
/// be written as a JSON envelope of members. What comes back is the operation's own answer
/// document, read exactly as [`call_rest_json`] reads one, and a refusal is that service's
/// own.
pub(crate) async fn call_rest_payload(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    signing_name: &str,
    region: &str,
    method: &'static str,
    uri: &str,
    media_type: &str,
    payload: Vec<u8>,
) -> Result<Value, Refusal> {
    let headers = vec![("content-type", media_type.to_string())];
    let (status, _answered, answer) =
        dispatch(ctx, signing_name, region, method, uri, headers, payload, &[])
            .await
            .map_err(|message| Refusal {
                code: "InternalFailure".to_string(),
                message,
            })?;
    let document: Value = serde_json::from_slice(&answer).unwrap_or(Value::Null);
    if (200..300).contains(&status) {
        return Ok(document);
    }
    Err(refusal_of(signing_name, status, &document))
}

/// Call one REST-XML operation of another emulated service in-process and read the XML
/// document it answers with, with the response HEADERS the caller names beside it.
/// `Err(refusal)` is that service's own refusal: the error CODE it carries, and the message
/// it worded.
///
/// A `restXml` service (Amazon Route 53) binds each operation to a method and a URI like a
/// REST-JSON one, and takes an XML document rather than a JSON one — so the caller passes
/// the binding the called operation's model declares and the body it writes, and what comes
/// back is the answer's text, read with [`xml_element`] and [`xml_scalar_children`].
/// `body` is `None` for an operation whose input is entirely in the URI.
///
/// `sent` are the request headers to add beside the ones the wire needs, and `answered` the
/// response headers to report back: a `restXml` service can report a resource's VERSION in a
/// header rather than in the answer's body — CloudFront answers every read of a distribution
/// with an `ETag`, and the update and the delete of one take that version back as their
/// `If-Match` — so a provider that changes or removes such a resource reads that header here
/// and sends it on the next call. A header the answer does not carry is simply absent from
/// the map.
///
/// A 501 is a refusal like any other, exactly as it is for [`call_json_rpc`]: the
/// difference between the two is only the wire the called service publishes.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn call_rest_xml(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    signing_name: &str,
    region: &str,
    method: &'static str,
    uri: &str,
    body: Option<&str>,
    sent: &[(&'static str, String)],
    answered: &[&str],
) -> Result<(String, BTreeMap<String, String>), Refusal> {
    let payload = body.map(str::as_bytes).unwrap_or_default().to_vec();
    let mut request_headers = vec![("content-type", REST_XML.to_string())];
    request_headers.extend(sent.iter().cloned());
    let (status, reported, answer) = dispatch(
        ctx,
        signing_name,
        region,
        method,
        uri,
        request_headers,
        payload,
        answered,
    )
    .await
    .map_err(|message| Refusal {
        code: "InternalFailure".to_string(),
        message,
    })?;
    let document = String::from_utf8_lossy(&answer).into_owned();
    if (200..300).contains(&status) {
        return Ok((document, reported));
    }
    // A restXml error answer is `<ErrorResponse><Error><Type/><Code/><Message/></Error>…`,
    // and the code is the one the model's error shape carries (`NoSuchHostedZone`), which is
    // what a client branches on.
    let code = xml_element(&document, "Code")
        .unwrap_or_else(|| format!("{UNNAMED_CODE_PREFIX}{status}"));
    Err(Refusal {
        message: sdk_message(
            signing_name,
            status,
            &code,
            &xml_element(&document, "Message").unwrap_or_default(),
        ),
        code,
    })
}

/// One element carrying TEXT, as a restXml request body spells a scalar member:
/// `<Name>value</Name>`, with the five predefined entities escaped by the same maintained
/// XML implementation that reads the answers back ([`xml_element`]) — so a value carrying
/// `&` or `<` reaches the called operation as the text it is rather than as markup.
///
/// The element NAME is the called model's own member name, never caller text.
pub(crate) fn xml_leaf(name: &str, value: &str) -> String {
    format!("<{name}>{}</{name}>", ::quick_xml::escape::escape(value))
}

/// What an awsQuery parameter name or value is percent-encoded against: everything
/// outside the UNRESERVED set of RFC 3986 (`A-Z a-z 0-9 - _ . ~`) is escaped, which is
/// what an AWS SDK sends and what the SigV4 canonical request assumes.
pub(super) const FORM_VALUE: &::percent_encoding::AsciiSet = &::percent_encoding::NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'~');

/// The text one ENTITY REFERENCE stands for, as the reader hands it over.
///
/// The XML reader BREAKS a text run at a reference and reports `&amp;`, `&lt;`, `&gt;`,
/// `&quot;` and `&apos;` as events of their own, so a reader that handles only text events
/// drops the character every one of them escapes — and an answering service's own message
/// quoting a value (`Invalid value 'x' for …`) reaches the wire with two of them. Resolved by
/// the parser's own escaping rules rather than by a table here; a reference that stands for
/// nothing is read as no text at all, which is what it contributes.
pub(super) fn referenced_text(reference: &::quick_xml::events::BytesRef<'_>) -> String {
    let Ok(name) = reference.decode() else {
        return String::new();
    };
    ::quick_xml::escape::unescape(&format!("&{name};"))
        .map(::std::borrow::Cow::into_owned)
        .unwrap_or_default()
}

/// The text of the first `<tag>` element of an XML document, with its entities decoded, or
/// `None` where the document carries no such element.
///
/// A LEAF element's text: the character data between that element's own tags, concatenated,
/// which is what the members read through it are (`<TopicArn>`, `<Code>`, `<Message>`).
/// It keeps no element depth, so reading a member that HOLDS other elements — or one whose
/// name repeats inside itself — needs a depth-aware reader rather than this helper.
///
/// Read with the same XML reader the emulator's own restXml services use, so an escaped
/// value (`&amp;`, `&lt;`) comes back as the text the answering service wrote rather than
/// as its escaping.
pub(crate) fn xml_element(document: &str, tag: &str) -> Option<String> {
    use ::quick_xml::events::Event;

    let mut reader = ::quick_xml::Reader::from_str(document);
    reader.config_mut().trim_text(false);
    let mut inside = false;
    let mut text = String::new();
    loop {
        match reader.read_event() {
            Ok(Event::Start(start)) if start.local_name().as_ref() == tag.as_bytes() => {
                inside = true;
            }
            Ok(Event::Text(found)) if inside => {
                text.push_str(&found.xml10_content().unwrap_or_default());
            }
            Ok(Event::GeneralRef(reference)) if inside => {
                text.push_str(&referenced_text(&reference));
            }
            Ok(Event::End(end)) if inside && end.local_name().as_ref() == tag.as_bytes() => {
                return Some(text);
            }
            // An empty element (`<TopicArn/>`) is the empty value of that element.
            Ok(Event::Empty(empty)) if empty.local_name().as_ref() == tag.as_bytes() => {
                return Some(String::new());
            }
            Ok(Event::Eof) | Err(_) => return None,
            Ok(_) => {}
        }
    }
}

/// The text of EVERY `<tag>` element of an XML document, in document order, with their
/// entities decoded.
///
/// [`xml_element`] reads the first one; a LIST member of an awsQuery answer repeats its
/// element once per entry (`<Tags><member><Key/><Value/></member>…`), so a reader of one
/// such member reads them all. Two members of the same list are read as PARALLEL lists and
/// zipped, which is sound exactly where the answering service reports both on every entry.
pub(crate) fn xml_elements(document: &str, tag: &str) -> Vec<String> {
    use ::quick_xml::events::Event;

    let mut reader = ::quick_xml::Reader::from_str(document);
    reader.config_mut().trim_text(false);
    let mut found = Vec::new();
    let mut reading: Option<String> = None;
    loop {
        match reader.read_event() {
            Ok(Event::Start(start)) if start.local_name().as_ref() == tag.as_bytes() => {
                reading = Some(String::new());
            }
            Ok(Event::Text(text)) => {
                if let Some(held) = reading.as_mut() {
                    held.push_str(&text.xml10_content().unwrap_or_default());
                }
            }
            Ok(Event::GeneralRef(reference)) => {
                if let Some(held) = reading.as_mut() {
                    held.push_str(&referenced_text(&reference));
                }
            }
            Ok(Event::End(end)) if end.local_name().as_ref() == tag.as_bytes() => {
                if let Some(held) = reading.take() {
                    found.push(held);
                }
            }
            // An empty element (`<Value/>`) is the empty value of that element.
            Ok(Event::Empty(empty)) if empty.local_name().as_ref() == tag.as_bytes() => {
                found.push(String::new());
            }
            Ok(Event::Eof) | Err(_) => return found,
            Ok(_) => {}
        }
    }
}

/// The map an awsQuery answer carries under `parent`, as the JSON object the rest of this
/// module compares: one entry per `<entry><key>…</key><value>…</value></entry>`, which is
/// how the query protocol serializes a map member (`Attributes` on
/// `GetTopicAttributes`).
///
/// Only the entries of THAT parent element: the walk starts at its start tag and stops at
/// its end tag, so a second map elsewhere in the document is not folded into this one. An
/// entry with no `<key>` is not an entry and is skipped; a `<value>` the answer leaves
/// empty is the empty string, which is what the answering service reported.
pub(crate) fn xml_entry_map(document: &str, parent: &str) -> ::serde_json::Map<String, Value> {
    use ::quick_xml::events::Event;

    const ENTRY: &[u8] = b"entry";
    const KEY: &[u8] = b"key";
    const VALUE: &[u8] = b"value";

    let mut reader = ::quick_xml::Reader::from_str(document);
    reader.config_mut().trim_text(false);
    let mut entries = ::serde_json::Map::new();
    let mut inside = false;
    // The entry being read: its key and its value, each `Some` once its element opened.
    let mut key: Option<String> = None;
    let mut value: Option<String> = None;
    let mut reading: Option<&[u8]> = None;
    loop {
        match reader.read_event() {
            Ok(Event::Start(start)) => match start.local_name().as_ref() {
                name if name == parent.as_bytes() => inside = true,
                ENTRY if inside => {
                    key = None;
                    value = None;
                }
                KEY if inside => {
                    key = Some(String::new());
                    reading = Some(KEY);
                }
                VALUE if inside => {
                    value = Some(String::new());
                    reading = Some(VALUE);
                }
                _ => {}
            },
            Ok(Event::Text(found)) if reading.is_some() => {
                let text = found.xml10_content().unwrap_or_default();
                let held = match reading {
                    Some(KEY) => key.as_mut(),
                    Some(VALUE) => value.as_mut(),
                    _ => None,
                };
                if let Some(held) = held {
                    held.push_str(&text);
                }
            }
            Ok(Event::GeneralRef(reference)) if reading.is_some() => {
                let text = referenced_text(&reference);
                let held = match reading {
                    Some(KEY) => key.as_mut(),
                    Some(VALUE) => value.as_mut(),
                    _ => None,
                };
                if let Some(held) = held {
                    held.push_str(&text);
                }
            }
            Ok(Event::End(end)) => match end.local_name().as_ref() {
                name if name == parent.as_bytes() => return entries,
                ENTRY if inside => {
                    if let Some(key) = key.take() {
                        entries.insert(key, Value::String(value.take().unwrap_or_default()));
                    }
                }
                KEY | VALUE => reading = None,
                _ => {}
            },
            Ok(Event::Eof) | Err(_) => return entries,
            Ok(_) => {}
        }
    }
}

/// The ENTRIES of one LIST member of an awsQuery answer: the text of each `<member>` element
/// directly inside the first `<parent>`, in document order, with their entities decoded.
///
/// [`xml_elements`] reads every element of a name ANYWHERE in the document, which folds two
/// lists of the same answer into one — a load balancer reports both its `SecurityGroups` and
/// its `AvailabilityZones` as `<member>` lists — so a reader of ONE of them is scoped to that
/// member's own element: the walk starts at its start tag and stops at its end tag, and only
/// an entry opened at its own depth is one of its entries, so a `<member>` nested deeper
/// cannot be mistaken for one. A list the answer does not carry, and one it carries empty,
/// are both the empty list, which is what that service reported.
pub(crate) fn xml_member_list(document: &str, parent: &str) -> Vec<String> {
    use ::quick_xml::events::Event;

    const MEMBER: &[u8] = b"member";

    let mut reader = ::quick_xml::Reader::from_str(document);
    reader.config_mut().trim_text(false);
    let mut entries = Vec::new();
    let mut inside = false;
    // How many elements are open BELOW `parent`: an entry opens at 0, and only one read at
    // that level is an entry of this list.
    let mut depth = 0usize;
    let mut reading: Option<String> = None;
    loop {
        match reader.read_event() {
            Ok(Event::Start(start)) => {
                let name = start.local_name();
                if !inside {
                    inside = name.as_ref() == parent.as_bytes();
                    continue;
                }
                if depth == 0 && name.as_ref() == MEMBER {
                    reading = Some(String::new());
                }
                depth += 1;
            }
            Ok(Event::Text(found)) if reading.is_some() => {
                if let Some(held) = reading.as_mut() {
                    held.push_str(&found.xml10_content().unwrap_or_default());
                }
            }
            Ok(Event::GeneralRef(reference)) if reading.is_some() => {
                if let Some(held) = reading.as_mut() {
                    held.push_str(&referenced_text(&reference));
                }
            }
            Ok(Event::Empty(empty)) if inside && depth == 0 => {
                if empty.local_name().as_ref() == MEMBER {
                    entries.push(String::new());
                }
            }
            Ok(Event::End(end)) => {
                if !inside {
                    continue;
                }
                if depth == 0 && end.local_name().as_ref() == parent.as_bytes() {
                    return entries;
                }
                depth = depth.saturating_sub(1);
                if depth == 0 && end.local_name().as_ref() == MEMBER {
                    if let Some(held) = reading.take() {
                        entries.push(held);
                    }
                }
            }
            Ok(Event::Eof) | Err(_) => return entries,
            Ok(_) => {}
        }
    }
}

/// The ENTRIES of one list member whose entries are STRUCTURES: the scalar children of every
/// `<member>` element directly inside the first `<parent>`, in document order, one map per
/// entry.
///
/// [`xml_member_list`] reads a list whose entries are scalars and [`xml_scalar_children`] one
/// structure; a list of structures — a stack's `Outputs`, carried as
/// `<Outputs><member><OutputKey/><OutputValue/></member>…` — is read by this one. Reading each
/// entry on its own is what keeps an OPTIONAL member honest: a member one entry does not carry
/// is absent from that entry, rather than shifting the next entry's value onto it the way
/// reading two members as parallel lists would.
///
/// Scoped to that member's own element exactly as [`xml_member_list`] is: the walk starts at
/// its start tag and stops at its end tag, and only an entry opened at its own depth is one of
/// its entries, so a `<member>` nested deeper cannot be mistaken for one. A list the answer
/// does not carry, and one it carries empty, are both the empty list. A child that HOLDS other
/// elements is not a scalar and is left out of its entry, as it is for
/// [`xml_scalar_children`].
pub(crate) fn xml_member_structures(
    document: &str,
    parent: &str,
) -> Vec<::serde_json::Map<String, Value>> {
    use ::quick_xml::events::Event;

    const MEMBER: &[u8] = b"member";

    let mut reader = ::quick_xml::Reader::from_str(document);
    reader.config_mut().trim_text(false);
    let mut entries: Vec<::serde_json::Map<String, Value>> = Vec::new();
    let mut inside = false;
    // How many elements are open BELOW `parent`: an entry opens at 0 and its own members at 1.
    let mut depth = 0usize;
    // The entry being read, the direct child of it being read while that child is still a
    // candidate scalar, and that child's text.
    let mut entry: Option<::serde_json::Map<String, Value>> = None;
    let mut member: Option<String> = None;
    let mut text = String::new();
    loop {
        match reader.read_event() {
            Ok(Event::Start(start)) => {
                let name = String::from_utf8_lossy(start.local_name().as_ref()).into_owned();
                if !inside {
                    inside = name == parent;
                    continue;
                }
                match depth {
                    0 if name.as_bytes() == MEMBER => entry = Some(::serde_json::Map::new()),
                    1 if entry.is_some() => {
                        member = Some(name);
                        text.clear();
                    }
                    // The child this is nested in holds elements, so it is not a scalar.
                    _ => member = None,
                }
                depth += 1;
            }
            Ok(Event::Text(found)) if inside && depth == 2 && member.is_some() => {
                text.push_str(&found.xml10_content().unwrap_or_default());
            }
            Ok(Event::GeneralRef(reference)) if inside && depth == 2 && member.is_some() => {
                text.push_str(&referenced_text(&reference));
            }
            Ok(Event::Empty(empty)) if inside => {
                let name = String::from_utf8_lossy(empty.local_name().as_ref()).into_owned();
                match depth {
                    // An empty entry (`<member/>`) carries no member at all.
                    0 if name.as_bytes() == MEMBER => entries.push(::serde_json::Map::new()),
                    // An empty element (`<OutputValue/>`) is the empty value of that member.
                    1 => {
                        if let Some(held) = entry.as_mut() {
                            held.insert(name, Value::String(String::new()));
                        }
                    }
                    _ => {}
                }
            }
            Ok(Event::End(end)) if inside => {
                let name = String::from_utf8_lossy(end.local_name().as_ref()).into_owned();
                if depth == 0 {
                    if name == parent {
                        return entries;
                    }
                    continue;
                }
                depth -= 1;
                if depth == 0 {
                    if name.as_bytes() == MEMBER {
                        if let Some(held) = entry.take() {
                            entries.push(held);
                        }
                    }
                } else if depth == 1 {
                    if let Some(read) = member.take() {
                        if read == name {
                            if let Some(held) = entry.as_mut() {
                                held.insert(
                                    read,
                                    Value::String(::std::mem::take(&mut text)),
                                );
                            }
                        }
                    }
                }
            }
            Ok(Event::Eof) | Err(_) => return entries,
            Ok(_) => {}
        }
    }
}

/// Each DIRECT child element of `parent` that holds TEXT, as the JSON object the rest of
/// this module compares: one entry per such element, keyed by its own name.
///
/// This is how an awsQuery answer carries a resource's scalar members — Amazon RDS reports
/// a DB instance as `<DBInstance><DBInstanceClass>db.t3.micro</DBInstanceClass>…` — so it
/// is the reader for a service whose answer is a STRUCTURE rather than the entry map
/// [`xml_entry_map`] reads, beside [`xml_member_list`] for one that is a LIST.
///
/// Only the first `parent` element, and only its own children: the walk starts at that
/// element's start tag and stops at its end tag, so a second structure elsewhere in the
/// document (the next DB instance of a list) is not folded into this one. A child that
/// HOLDS other elements is not a scalar and is left out entirely rather than reported as
/// the concatenation of its descendants' text; an empty element (`<Iops/>`) is the empty
/// value of that member, which is what the answering service reported.
pub(crate) fn xml_scalar_children(
    document: &str,
    parent: &str,
) -> ::serde_json::Map<String, Value> {
    use ::quick_xml::events::Event;

    let mut reader = ::quick_xml::Reader::from_str(document);
    reader.config_mut().trim_text(false);
    let mut members = ::serde_json::Map::new();
    let mut inside = false;
    // How many elements are open BELOW `parent`: a direct child opens at 0, and only a
    // child read at that level is one of this structure's own members.
    let mut depth = 0usize;
    // The direct child being read, while it is still a candidate scalar, and its text.
    let mut member: Option<String> = None;
    let mut text = String::new();
    loop {
        match reader.read_event() {
            Ok(Event::Start(start)) => {
                let name = String::from_utf8_lossy(start.local_name().as_ref()).into_owned();
                if !inside {
                    inside = name == parent;
                    continue;
                }
                if depth == 0 {
                    member = Some(name);
                    text.clear();
                } else {
                    // The child this is nested in holds elements, so it is not a scalar.
                    member = None;
                }
                depth += 1;
            }
            Ok(Event::Text(found)) if inside && depth == 1 => {
                text.push_str(&found.xml10_content().unwrap_or_default());
            }
            Ok(Event::GeneralRef(reference)) if inside && depth == 1 => {
                text.push_str(&referenced_text(&reference));
            }
            Ok(Event::Empty(empty)) if inside && depth == 0 => {
                let name = String::from_utf8_lossy(empty.local_name().as_ref()).into_owned();
                members.insert(name, Value::String(String::new()));
            }
            Ok(Event::End(end)) if inside => {
                let name = String::from_utf8_lossy(end.local_name().as_ref()).into_owned();
                if depth == 0 {
                    if name == parent {
                        return members;
                    }
                    continue;
                }
                if depth == 1 {
                    if let Some(read) = member.take() {
                        if read == name {
                            members.insert(read, Value::String(::std::mem::take(&mut text)));
                        }
                    }
                }
                depth -= 1;
            }
            Ok(Event::Eof) | Err(_) => return members,
            Ok(_) => {}
        }
    }
}

/// A service's error answer as the code it carries and the message it worded. The code is
/// the last segment of the awsJson `__type`
/// (`com.amazonaws.firehose#ResourceInUseException` -> `ResourceInUseException`), which is
/// what a client branches on.
///
/// The message is the one an SDK of that service would raise ([`sdk_message`]), because what
/// a resource provider reports is what its own SDK call raised.
pub(super) fn refusal_of(signing_name: &str, status: u16, document: &Value) -> Refusal {
    refusal_of_call(signing_name, None, status, document)
}

/// [`refusal_of`] for a caller that states WHICH operation of that service was refused,
/// which is part of the diagnostic some SDKs word ([`call_rest_json_named`]).
pub(super) fn refusal_of_call(
    signing_name: &str,
    operation: Option<&str>,
    status: u16,
    document: &Value,
) -> Refusal {
    let code = document
        .get("__type")
        .or_else(|| document.get("code"))
        .or_else(|| document.get("Code"))
        .and_then(Value::as_str)
        .map(|code| code.rsplit('#').next().unwrap_or(code).to_string())
        .unwrap_or_else(|| format!("{UNNAMED_CODE_PREFIX}{status}"));
    let message = document
        .get("message")
        .or_else(|| document.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    Refusal {
        message: sdk_message_of(signing_name, operation, status, &code, message),
        code,
    }
}

/// Sign a request with the emulator's own credentials and dispatch it in-process to the
/// service its credential scope names.
///
/// The request is signed because the emulator verifies every request signature when
/// authentication is on; the credential scope is also what routes it, so the service that
/// owns the resource is the one that serves the call.
///
/// `method` and `uri` are the called operation's own HTTP binding: an RPC protocol binds
/// every operation to `POST /` and names what it acts on in its document — or, for
/// awsQuery, in its form body — while a REST protocol binds each operation to its own
/// method and path.
///
/// The call itself is served on a TASK of its own. It reaches the called service's whole
/// deserialize → handle → serialize chain, and for a type whose owning service is
/// CloudFormation itself — a nested stack — that chain deploys a stack, which provisions its
/// own resources through this same function: a chain of calls would otherwise pile every
/// level's frames onto the stack of the ONE request that started it, and the emulator's worker
/// stack is a fixed size. A task starts from the top of a worker's stack, so one call costs
/// one level however deeply a template nests. The answer is awaited right here, so the call is
/// still synchronous to the walk that made it.
///
/// `answered` names the response HEADERS to report back beside the status and the body. Most
/// answers say everything they say in their body; a `restXml` service reports a resource's
/// VERSION in a header instead — CloudFront's `ETag`, which its own update and delete take
/// back as an `If-Match` — so a caller that has to address the resource again names the
/// headers it needs and they are read off the answer here, where the response still exists.
/// One entry per named header the answer actually had, keyed by the name as the caller
/// spelled it.
#[allow(clippy::too_many_arguments)]
pub(super) async fn dispatch(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    signing_name: &str,
    region: &str,
    method: &'static str,
    uri: &str,
    headers: Vec<(&'static str, String)>,
    body: Vec<u8>,
    answered: &[&str],
) -> Result<(u16, BTreeMap<String, String>, Vec<u8>), String> {
    let host = format!("{signing_name}.{region}.amazonaws.com");
    let mut request = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", host);
    for (name, value) in &headers {
        request = request.header(*name, value);
    }
    let mut request = request.body(body).map_err(|error| error.to_string())?;

    let (access_key_id, secret_access_key) = callback_credentials(ctx).await?;
    sign_callback_request(
        &mut request,
        signing_name,
        region,
        &access_key_id,
        &secret_access_key,
    )?;

    let (parts, body) = request.into_parts();
    let request = ::http::Request::from_parts(
        parts,
        ::aws_smithy_types::body::SdkBody::from(::bytes::Bytes::from(body)),
    );
    let request = ::aws_smithy_runtime_api::http::Request::try_from(request)
        .map_err(|error| error.to_string())?;
    // The connector is a cloneable handle to the router, so the task owns everything it
    // needs and borrows nothing of this request's context. It is taken from the CALLBACK, so
    // the chain's recursion depth is stamped on the request exactly as it is for a call made
    // here (`smithy_server_runtime::CallbackRecursionDepth`).
    let connector = ctx.callback().http_connector();
    // ABORTED WITH THE CALLER: the task's handle is held in a guard that aborts it when
    // dropped, so a request that goes away — a client that disconnected, a wait that ran out
    // — takes the work it started with it instead of leaving it running (and spawning
    // further levels of its own).
    let mut served = AbortedWithTheCaller(::tokio::spawn(async move {
        ::aws_smithy_runtime_api::client::http::HttpConnector::call(&connector, request).await
    }));
    let response = served
        .answered()
        .await
        // The task was cancelled or panicked: nothing answered, which is what the caller is
        // told. A panic in another service's handler is that service's own fault and is not
        // swallowed — it is reported as the failure of this call.
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())?;
    let status = response.status().as_u16();
    let reported = answered
        .iter()
        .filter_map(|name| {
            response
                .headers()
                .get(*name)
                .map(|value| ((*name).to_string(), value.to_string()))
        })
        .collect();
    let payload = response
        .body()
        .bytes()
        .ok_or("the in-process answer carried a streaming body")?
        .to_vec();
    Ok((status, reported, payload))
}

/// One in-process call's task, held so that dropping this guard ABORTS it.
///
/// `tokio::spawn` detaches: dropping a `JoinHandle` leaves the task running, and for a call
/// that re-enters this walk that means a request nobody is waiting for any more goes on
/// deploying (and spawning further levels). The guard makes the task's lifetime the caller's:
/// the awaiting future owns it, and whatever drops that future — a disconnected client, a
/// ceiling that ran out, a cancelled operation — stops the work it set off.
pub(super) struct AbortedWithTheCaller<T>(pub(super) ::tokio::task::JoinHandle<T>);

impl<T> AbortedWithTheCaller<T> {
    /// What the task answered, awaited through the handle the guard holds (a `JoinHandle` is
    /// `Unpin`, so it is polled in place and the guard keeps ownership until the answer or the
    /// cancellation).
    pub(super) async fn answered(&mut self) -> Result<T, ::tokio::task::JoinError> {
        (&mut self.0).await
    }
}

impl<T> Drop for AbortedWithTheCaller<T> {
    fn drop(&mut self) {
        // A task that has already answered is finished, and aborting a finished task does
        // nothing — so this is the cancellation path only.
        self.0.abort();
    }
}

/// SigV4-sign one in-process request to another emulated service, in place.
///
/// The settings are the ones the emulator's own verifier re-signs with
/// (`vendor/server/src/request_authentication.rs`), so the signature this produces is the
/// one that verifier computes over the same request — which is what lets a stack create a
/// resource with request authentication ON. Public so a test can hold it to exactly that
/// (`tests/all/callback_signature.rs`).
pub fn sign_callback_request(
    request: &mut ::http::Request<Vec<u8>>,
    signing_name: &str,
    region: &str,
    access_key_id: &str,
    secret_access_key: &str,
) -> Result<(), String> {
    use ::aws_sigv4::http_request::{
        sign, PayloadChecksumKind, SignableBody, SignableRequest, SigningSettings,
    };

    let identity: ::aws_smithy_runtime_api::client::identity::Identity =
        ::aws_credential_types::Credentials::builder()
            .provider_name("aws-emulator-cloudformation")
            .access_key_id(access_key_id)
            .secret_access_key(secret_access_key)
            .build()
            .into();
    let mut settings = SigningSettings::default();
    settings.payload_checksum_kind = PayloadChecksumKind::NoHeader;
    settings.excluded_headers = None;
    let params = ::aws_sigv4::sign::v4::SigningParams::builder()
        .identity(&identity)
        .region(region)
        .name(signing_name)
        .settings(settings)
        .time(SystemTime::now())
        .build()
        .map_err(|error| error.to_string())?;
    let body = request.body().clone();
    let signable_headers: Vec<(String, String)> = request
        .headers()
        .iter()
        .map(|(name, value)| {
            (
                name.as_str().to_string(),
                value.to_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    let signable = SignableRequest::new(
        request.method().as_str(),
        request.uri().to_string(),
        signable_headers
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str())),
        SignableBody::Bytes(&body),
    )
    .map_err(|error| error.to_string())?;
    let (instructions, _signature) = sign(signable, &params.into())
        .map_err(|error| error.to_string())?
        .into_parts();
    instructions.apply_to_request_http1x(request);
    Ok(())
}

/// The credentials a callback signs with: the emulator's own, through the provider the
/// framework hands every handler — which falls back to its placeholder pair when the
/// router vends none (request authentication off, where nothing verifies a signature).
pub(super) async fn callback_credentials(
    ctx: &smithy_server_runtime::OperationContext<'_>,
) -> Result<(String, String), String> {
    use ::aws_credential_types::provider::ProvideCredentials;

    let credentials = ctx
        .callback()
        .aws_credentials_provider()
        .provide_credentials()
        .await
        .map_err(|error| error.to_string())?;
    Ok((
        credentials.access_key_id().to_string(),
        credentials.secret_access_key().to_string(),
    ))
}
