// SPDX-License-Identifier: CC0-1.0

//! The endpoint a `custom-resource` scalable target IS, read and written where it lives.
//!
//! Every other namespace this service scales names a resource of another AWS service, whose
//! capacity this service moves through that service ([`super::policy_execution`]). The
//! `custom-resource` namespace names no AWS resource at all: "for a resource provided by your
//! own application or service, use custom-resource instead"
//! (`ServiceNamespace`), and the target's `ResourceId` is the HTTP endpoint that application
//! serves for it — "custom resources are not supported with a resource type […] the unique
//! identifier is defined by the service provider" (the model's own `ResourceId`
//! documentation). So the capacity of such a target is not a value this service holds: it is
//! read from that endpoint and written to it.
//!
//! The contract is the one Application Auto Scaling publishes for a custom resource, and it
//! is two requests on the endpoint the resource id names:
//!
//! * `GET` answers the resource's own capacity, as the `actualCapacity` it is at (falling
//!   back to the `desiredCapacity` it was asked for, for an endpoint that reports only
//!   that); and
//! * `PATCH` with a body of `{"desiredCapacity": <N>}` asks it to move to `N`, which is what
//!   every scaling decision taken for such a target does — a scheduled action firing, a step
//!   policy's alarm breaching, a target-tracking policy's high alarm breaching.
//!
//! Both are made over the IN-PROCESS callback transport
//! (`::smithy_server_runtime::EmbeddedEmulator`), signed as every in-process caller signs
//! (`::service_common::in_process_sigv4`) under the signing name an invoke endpoint
//! authenticates: nothing here opens a socket or leaves the host.
//!
//! Being able to make those two requests is the PRECONDITION of registering such a target,
//! because they are the whole of what scaling one does — so [`probe_capacity`] reports which
//! of three things asking the endpoint for its capacity learnt, and the registration answers
//! each differently ([`super::remote_resources`]): an endpoint that served the document
//! resolves its capacity, one that answered the invocation with a refusal refuses the
//! registration ([`missing_invoke_permissions_message`]), and an address nothing on this
//! engine serves resolves nothing at all — the same answer this service gives for any other
//! namespace whose owning store it cannot reach.

use ::serde_json::Value;

/// The signing name an invoke of an API endpoint is signed under, which is also what an
/// endpoint served by this emulator authenticates a caller's signature against.
const INVOKE_SIGNING_NAME: &str = "execute-api";

/// The media type the capacity document is carried in, both ways.
const APPLICATION_JSON: &str = "application/json";

/// The member of the endpoint's document that reports the capacity the resource IS at.
const ACTUAL_CAPACITY: &str = "actualCapacity";

/// The member that reports the capacity it was last asked for, and the one a request to move
/// it carries. Read as the resource's capacity only where it reports no [`ACTUAL_CAPACITY`].
const DESIRED_CAPACITY: &str = "desiredCapacity";

/// The host and path a `custom-resource` `ResourceId` addresses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    /// The authority the endpoint is served on, as a `Host` header carries it.
    pub host: String,
    /// The path, with its query, the capacity document is served at.
    pub path: String,
}

/// The endpoint a `custom-resource` `ResourceId` names, or `None` for an id that is no
/// endpoint at all — which is a target whose capacity nothing can read or move.
///
/// The id is an absolute URL, parsed with `::http::Uri` rather than by splitting the string:
/// the authority it names is what the request is addressed to and the path, with its query, is
/// the capacity document. An id that names no authority, or a scheme nothing is served over,
/// addresses no endpoint; one that names no path of its own addresses the root of that
/// endpoint, which is how a URI with an empty path normalises.
pub fn endpoint_of(resource_id: &str) -> Option<Endpoint> {
    let uri: ::http::Uri = resource_id.parse().ok()?;
    if !matches!(uri.scheme_str(), Some("http" | "https")) {
        return None;
    }
    let authority = uri.authority()?.as_str().to_string();
    let path = uri
        .path_and_query()
        .map_or("/", ::http::uri::PathAndQuery::as_str)
        .to_string();
    (!authority.is_empty()).then_some(Endpoint {
        host: authority,
        path,
    })
}

/// What asking the endpoint behind a `custom-resource` target for its capacity learnt.
///
/// The three outcomes are three different facts, and a registration answers each of them
/// differently ([`super::remote_resources`]), so they are not collapsed into one `Option`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reached {
    /// The endpoint SERVED the capacity document, carrying the capacity it reports when it
    /// reports one this reader can read.
    Served(Option<i32>),
    /// The endpoint refused to be INVOKED at all: the address serves nothing to this caller,
    /// which is the one answer that says the contract's calls cannot be made.
    Refused,
    /// Nothing was learnt: the resource id addresses no endpoint, nothing on this engine
    /// serves that address, or the endpoint answered something that is neither the document
    /// nor a refusal of the invocation.
    Unserved,
}

/// The actions the Application Auto Scaling custom-resource contract exercises on the
/// endpoint: the `PATCH` that moves its capacity and the `GET` that reads it, as the
/// `execute-api` service publishes an invoke permission
/// (`execute-api:Invoke`, per method).
const REQUIRED_INVOKE_ACTIONS: [&str; 2] = ["execute-api:Invoke:PATCH", "execute-api:Invoke:GET"];

/// How a registration refuses a `custom-resource` target whose endpoint it could not invoke.
///
/// Invoking the endpoint is the whole of what registering such a target does, so an endpoint
/// that answers the invocation with a refusal is one the caller cannot exercise the contract's
/// actions on — and the refusal says which actions those are. They are the contract's, not the
/// request's, so the sentence carries no value of the call.
pub fn missing_invoke_permissions_message() -> String {
    format!(
        "User is missing the following permissions: {}",
        REQUIRED_INVOKE_ACTIONS.join(", ")
    )
}

/// Ask the endpoint behind a `custom-resource` target for its capacity, reporting WHICH of
/// the three outcomes the invocation had.
pub async fn probe_capacity(
    callback: &::smithy_server_runtime::EmbeddedEmulator,
    region: &str,
    resource_id: &str,
) -> Reached {
    let Some(endpoint) = endpoint_of(resource_id) else {
        return Reached::Unserved;
    };
    let Some(answer) = call(callback, region, "GET", &endpoint, Vec::new()).await else {
        // The request could not be made at all, which is a fact about this process rather
        // than about the endpoint.
        return Reached::Unserved;
    };
    if answer.is_success() {
        return Reached::Served(reported_capacity(&answer.payload));
    }
    // ONE class of answer says the contract's calls cannot be made: the endpoint declining to
    // serve this caller at all, which is the status an invoke of an api, a stage or a method
    // that does not serve it earns and the one an unauthorized invoke earns
    // ([`NOT_INVOCABLE_STATUSES`]). Every other answer leaves the registration answered on
    // its own terms — the emulator's own front door refusing a call no mounted service served
    // (`::service_common::in_process_refusal`), a fault of the endpoint's own backend behind a
    // served invocation, and anything else it chose to answer, none of which is a statement
    // about the caller's permission to invoke it.
    if !answer.unserved() && NOT_INVOCABLE_STATUSES.contains(&answer.status) {
        return Reached::Refused;
    }
    Reached::Unserved
}

/// The statuses an endpoint answers a caller it does not serve: the `403` an invoke endpoint
/// gives for an api, a stage or a method it does not serve and for a caller not authorized to
/// invoke it, and the `401` an endpoint that authenticates nobody of this caller's gives.
const NOT_INVOCABLE_STATUSES: [u16; 2] = [401, 403];

/// The capacity the resource behind a `custom-resource` target reports right now, or `None`
/// when this emulator could not read it.
pub async fn read_capacity(
    callback: &::smithy_server_runtime::EmbeddedEmulator,
    region: &str,
    resource_id: &str,
) -> Option<i32> {
    match probe_capacity(callback, region, resource_id).await {
        Reached::Served(capacity) => capacity,
        Reached::Refused | Reached::Unserved => None,
    }
}

/// The capacity a served document reports, when it reports one this reader can read.
///
/// The document is the CALLER's own application's, so a capacity it spells as text is the
/// number that text spells: the member is read as a number first and as the decimal it is
/// written as otherwise, which is how the same value reaches this reader from an endpoint that
/// renders its document with the quotes a JSON template puts round every value.
fn reported_capacity(payload: &[u8]) -> Option<i32> {
    let document: Value = ::serde_json::from_slice(payload).ok()?;
    let capacity = reported_number(&document, ACTUAL_CAPACITY)
        .or_else(|| reported_number(&document, DESIRED_CAPACITY))?;
    (capacity.is_finite() && capacity >= 0.0 && capacity <= f64::from(i32::MAX))
        .then(|| capacity.round() as i32)
}

/// The number `member` of `document` carries, as a number or as the decimal it spells.
fn reported_number(document: &Value, member: &str) -> Option<f64> {
    let value = document.get(member)?;
    value.as_f64().or_else(|| {
        value
            .as_str()
            .map(str::trim)
            .and_then(|text| text.parse::<f64>().ok())
    })
}

/// Ask the resource behind a `custom-resource` target to move to `capacity`, answering
/// whether the endpoint accepted it.
///
/// The body is the one member the contract carries, as the number the endpoint reports its
/// capacity as.
pub async fn set_capacity(
    callback: &::smithy_server_runtime::EmbeddedEmulator,
    region: &str,
    resource_id: &str,
    capacity: i32,
) -> bool {
    let Some(endpoint) = endpoint_of(resource_id) else {
        return false;
    };
    let body = capacity_document(capacity);
    let answered = call(callback, region, "PATCH", &endpoint, body.into_bytes()).await;
    answered.is_some_and(|answer| answer.is_success())
}

/// The document a request to MOVE the resource's capacity carries: the one member the
/// contract names, as the number the endpoint reports its capacity as.
fn capacity_document(capacity: i32) -> String {
    ::serde_json::json!({DESIRED_CAPACITY: f64::from(capacity)}).to_string()
}

/// Make one request on the endpoint over the in-process transport, answering what it
/// answered — or `None` when the request could not be made at all.
///
/// Everything a call of another emulated service takes is the one in-process caller this
/// service has ([`super::in_process_calls`]), under the signing name an invoke endpoint
/// authenticates; what this adds is the address the resource id names and the media type the
/// capacity document travels in.
async fn call(
    callback: &::smithy_server_runtime::EmbeddedEmulator,
    region: &str,
    method: &str,
    endpoint: &Endpoint,
    body: Vec<u8>,
) -> Option<super::in_process_calls::Answered> {
    let request = ::http::Request::builder()
        .method(method)
        .uri(&endpoint.path)
        .header("host", &endpoint.host)
        .header("content-type", APPLICATION_JSON)
        .header("accept", APPLICATION_JSON)
        .body(body)
        .ok()?;
    super::in_process_calls::call(callback, INVOKE_SIGNING_NAME, region, request).await
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RULE: a `custom-resource` resource id is the absolute URL of the capacity document
    /// the caller's own application serves; anything else addresses no document.
    #[test]
    fn a_custom_resource_id_addresses_one_capacity_document() {
        /// (case, the resource id, the endpoint it names)
        type Row<'a> = (&'a str, &'a str, Option<(&'a str, &'a str)>);
        let rows: &[Row<'_>] = &[
            (
                "the published form: an api stage and the dimension's own path",
                "https://example.execute-api.eu-west-1.amazonaws.com/prod/scalableTargetDimensions/1",
                Some((
                    "example.execute-api.eu-west-1.amazonaws.com",
                    "/prod/scalableTargetDimensions/1",
                )),
            ),
            (
                "an endpoint on an explicit port",
                "https://127.0.0.1:34991/_aws/execute-api/abc/prod/scalableTargetDimensions/7",
                Some((
                    "127.0.0.1:34991",
                    "/_aws/execute-api/abc/prod/scalableTargetDimensions/7",
                )),
            ),
            (
                "plain http",
                "http://localhost:8080/capacity",
                Some(("localhost:8080", "/capacity")),
            ),
            (
                "a query the endpoint reads",
                "https://host/capacity?dimension=1",
                Some(("host", "/capacity?dimension=1")),
            ),
            (
                "no path of its own, which addresses the endpoint's root",
                "https://host",
                Some(("host", "/")),
            ),
            ("an empty path", "https://host/", Some(("host", "/"))),
            ("no host", "https:///capacity", None),
            ("no scheme", "host/capacity", None),
            ("a scheme nothing is served over", "ftp://host/capacity", None),
            ("another namespace's resource id", "table/orders", None),
            ("nothing at all", "", None),
        ];
        for (case, resource_id, expected) in rows {
            let actual = endpoint_of(resource_id);
            let expected = expected.map(|(host, path)| Endpoint {
                host: host.to_string(),
                path: path.to_string(),
            });
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// RULE: a request to move the resource's capacity carries exactly the one member the
    /// custom-resource contract names, as the number the endpoint reports its capacity as.
    #[test]
    fn a_capacity_change_carries_the_one_member_the_contract_names() {
        /// (case, the capacity, the document sent for it)
        type Row<'a> = (&'a str, i32, &'a str);
        let rows: &[Row<'_>] = &[
            ("a capacity of one", 1, r#"{"desiredCapacity":1.0}"#),
            ("the floor a registration pulled up to", 3, r#"{"desiredCapacity":3.0}"#),
            ("no capacity at all", 0, r#"{"desiredCapacity":0.0}"#),
            ("a large capacity", 1200, r#"{"desiredCapacity":1200.0}"#),
        ];
        for (case, capacity, expected) in rows {
            let actual = capacity_document(*capacity);
            assert_eq!(
                actual, *expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// RULE: the capacity a served document reports is its `actualCapacity`, falling back to
    /// the `desiredCapacity` an endpoint that reports only that one carries — read as a number
    /// or as the decimal it spells.
    #[test]
    fn the_capacity_is_the_one_the_served_document_reports() {
        /// (case, the document, the capacity it reports)
        type Row<'a> = (&'a str, &'a str, Option<i32>);
        let rows: &[Row<'_>] = &[
            (
                "the documented body, whose capacity is a number",
                r#"{"actualCapacity":2.0,"desiredCapacity":2.0,"scalingStatus":"Successful"}"#,
                Some(2),
            ),
            (
                "a document whose values are quoted, as a JSON template renders them",
                r#"{"actualCapacity":"3","desiredCapacity":"3"}"#,
                Some(3),
            ),
            (
                "an endpoint reporting only the capacity it was asked for",
                r#"{"desiredCapacity":7}"#,
                Some(7),
            ),
            (
                "the capacity it IS wins over the one it was asked for",
                r#"{"actualCapacity":4,"desiredCapacity":9}"#,
                Some(4),
            ),
            ("a fractional capacity, which rounds", r#"{"actualCapacity":2.6}"#, Some(3)),
            ("no capacity member at all", r#"{"scalingStatus":"Pending"}"#, None),
            ("a capacity that is no number", r#"{"actualCapacity":"many"}"#, None),
            ("a negative capacity", r#"{"actualCapacity":-1}"#, None),
            ("a body that is no document", "not json", None),
        ];
        for (case, document, expected) in rows {
            let actual = reported_capacity(document.as_bytes());
            assert_eq!(
                actual, *expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }
}
