// SPDX-License-Identifier: CC0-1.0

//! The documents the Amazon SimpleDB 2009-04-15 API answers in, and the refusals it
//! states.
//!
//! An answer on this wire is an XML document in the API's own namespace
//! ([`SDB_NAMESPACE`]), named after the action that was called:
//!
//! ```xml
//! <CreateDomainResponse xmlns="http://sdb.amazonaws.com/doc/2009-04-15/">
//!   <ResponseMetadata><RequestId>…</RequestId><BoxUsage>…</BoxUsage></ResponseMetadata>
//! </CreateDomainResponse>
//! ```
//!
//! An action that reports something carries it in a `<<Action>Result>` element ahead of
//! the `ResponseMetadata`, which is the element an AWS SDK unwraps the result from.
//!
//! A refusal is the API's own error document — the `<Response><Errors><Error>` envelope,
//! NOT the `<ErrorResponse>` of the awsQuery protocols — carrying the error's published
//! `<Code>`, its `<Message>` and the same `<BoxUsage>`:
//!
//! ```xml
//! <Response><Errors><Error><Code>NoSuchDomain</Code>
//!   <Message>The specified domain does not exist.</Message><BoxUsage>…</BoxUsage>
//! </Error></Errors><RequestID>…</RequestID></Response>
//! ```
//!
//! Every interpolated string is escaped with the framework's own XML escaper
//! (`smithy_server_runtime::xml_escape`), so no caller-supplied text can change the shape
//! of a document; element NAMES are never caller text at all — they come from this
//! service's own action catalogue.

use aws_smithy_runtime_api::http::{Response as RtResponse, StatusCode};
use aws_smithy_types::body::SdkBody;
use smithy_server_runtime::xml_escape;

/// The XML namespace every document of this API carries: the Amazon SimpleDB
/// 2009-04-15 namespace, which is also the one the API's own documents declare.
pub const SDB_NAMESPACE: &str = "http://sdb.amazonaws.com/doc/2009-04-15/";

/// The media type this API answers in.
const XML_CONTENT_TYPE: &str = "text/xml";

/// The `BoxUsage` every answer reports.
///
/// `BoxUsage` is the machine utilisation Amazon SimpleDB metered for the call, and this
/// emulator meters nothing: the work it did is not the work Amazon SimpleDB would have
/// done, so there is no figure to measure. One nominal value is therefore reported for
/// every call rather than a different number invented per call. It has the form the API
/// publishes — a decimal fraction of an hour — and is a per-run report, not a
/// requirement: no documented behaviour of this API depends on it.
const BOX_USAGE: &str = "0.0000219907";

/// One refusal of this API: the HTTP status, the published `<Code>` a client branches on,
/// and the `<Message>` it shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueryApiError {
    /// The HTTP status the API answers this code with.
    pub status: u16,
    /// The error code, as the API's error list publishes it.
    pub code: &'static str,
    /// The sentence the API states for it.
    pub message: String,
}

/// The status a request-side refusal of this API carries.
const CLIENT_ERROR_STATUS: u16 = 400;

/// The status a refusal about the STATE the request would leave behind carries — a quota
/// reached, a conditional check that did not hold.
const CONFLICT_STATUS: u16 = 409;

/// The status a refusal naming an attribute the item does not have carries.
const NOT_FOUND_STATUS: u16 = 404;

/// The status an action this emulator does not serve carries.
const NOT_IMPLEMENTED_STATUS: u16 = 501;

/// The status a failure of the server itself carries.
const SERVER_ERROR_STATUS: u16 = 500;

/// The refusal for a failure of this emulator rather than of the request: the engine this
/// surface was built over holds no store for this service.
///
/// Stated as the server's own fault, with the sentence this API publishes for one, rather
/// than as a client error — nothing about the request would make a retry of it fail.
pub fn internal_error() -> QueryApiError {
    QueryApiError {
        status: SERVER_ERROR_STATUS,
        code: "InternalError",
        message: "Request could not be executed due to an internal service error.".to_string(),
    }
}

/// The refusal for a request that named no action at all.
pub fn missing_action() -> QueryApiError {
    QueryApiError {
        status: CLIENT_ERROR_STATUS,
        code: "MissingAction",
        message: "No action was supplied with this request.".to_string(),
    }
}

/// The refusal for an `Action` this API does not have, worded as the API words it: the
/// action named, and the statement that it is not one of this web service's.
pub fn invalid_action(action: &str) -> QueryApiError {
    QueryApiError {
        status: CLIENT_ERROR_STATUS,
        code: "InvalidAction",
        message: format!("The action {action} is not valid for this web service."),
    }
}

/// The refusal for an action this API DOES have that this emulator does not serve yet.
///
/// Not `InvalidAction`: the action exists on this endpoint, so saying it does not would
/// be a false statement about the service. This is the emulator's own
/// "modelled-but-not-implemented" answer, which is what every unimplemented operation of
/// every service here answers, and it is deliberately not a 2xx.
pub fn not_implemented(action: &str) -> QueryApiError {
    QueryApiError {
        status: NOT_IMPLEMENTED_STATUS,
        code: "NotImplemented",
        message: format!("action {action} is not implemented in this emulator"),
    }
}

/// The refusal for a `Version` no Amazon SimpleDB API declares.
pub fn no_such_version(version: &str) -> QueryApiError {
    QueryApiError {
        status: CLIENT_ERROR_STATUS,
        code: "NoSuchVersion",
        message: format!("The requested version ({version}) is not valid."),
    }
}

/// The refusal for a required parameter the request left out, worded as the API words it:
/// the parameter the request must contain.
pub fn missing_parameter(parameter: &str) -> QueryApiError {
    QueryApiError {
        status: CLIENT_ERROR_STATUS,
        code: "MissingParameter",
        message: format!("The request must contain the parameter {parameter}"),
    }
}

/// The refusal for a parameter whose value this API does not admit, worded as the API
/// words it: the value, the parameter it was given for, and why it is not valid.
///
/// `reason` is a whole sentence (it ends the message), because what is wrong with a value
/// differs per parameter — a length, an alphabet, a range.
pub fn invalid_parameter_value(parameter: &str, value: &str, reason: &str) -> QueryApiError {
    QueryApiError {
        status: CLIENT_ERROR_STATUS,
        code: "InvalidParameterValue",
        message: format!("Value (\"{value}\") for parameter {parameter} is invalid. {reason}"),
    }
}

/// The refusal for a `DomainName` that names no domain this account holds.
///
/// The same sentence the v2 model documents for its own `NoSuchDomainException`, which is
/// the one wording this endpoint publishes for a domain that does not exist.
pub fn no_such_domain() -> QueryApiError {
    QueryApiError {
        status: CLIENT_ERROR_STATUS,
        code: "NoSuchDomain",
        message: crate::state::refusal_message::no_such_resource(
            crate::state::refusal_message::DOMAIN_RESOURCE,
        ),
    }
}

/// The refusal for a `CreateDomain` past the account's domain quota.
pub fn number_domains_exceeded() -> QueryApiError {
    QueryApiError {
        status: CONFLICT_STATUS,
        code: "NumberDomainsExceeded",
        message: "Number of domains limit exceeded.".to_string(),
    }
}

/// The refusal for a write that would leave more attributes in one item than the API
/// admits.
pub fn number_item_attributes_exceeded() -> QueryApiError {
    QueryApiError {
        status: CONFLICT_STATUS,
        code: "NumberItemAttributesExceeded",
        message: "Too many attributes in this item.".to_string(),
    }
}

/// The refusal for a single call submitting more attributes than the API admits.
pub fn number_submitted_attributes_exceeded() -> QueryApiError {
    QueryApiError {
        status: CONFLICT_STATUS,
        code: "NumberSubmittedAttributesExceeded",
        message: "Too many attributes in a single call.".to_string(),
    }
}

/// The refusal for a write that would leave more bytes in one domain than this store
/// holds.
pub fn number_domain_bytes_exceeded() -> QueryApiError {
    QueryApiError {
        status: CONFLICT_STATUS,
        code: "NumberDomainBytesExceeded",
        message: "Too many bytes in this domain.".to_string(),
    }
}

/// The refusal for a `GetAttributes` asking for more attribute names than the API admits.
pub fn too_many_requested_attributes() -> QueryApiError {
    QueryApiError {
        status: CLIENT_ERROR_STATUS,
        code: "TooManyRequestedAttributes",
        message: "Too many attributes requested.".to_string(),
    }
}

/// The refusal for a `NextToken` this store did not issue.
pub fn invalid_next_token() -> QueryApiError {
    QueryApiError {
        status: CLIENT_ERROR_STATUS,
        code: "InvalidNextToken",
        message: crate::state::refusal_message::INVALID_NEXT_TOKEN_MESSAGE.to_string(),
    }
}

/// The refusal for a conditional write whose expected attribute holds a different value:
/// the attribute, the value it holds, and the value the request expected.
pub fn conditional_check_failed_value(
    name: &str,
    actual: &str,
    expected: &str,
) -> QueryApiError {
    QueryApiError {
        status: CONFLICT_STATUS,
        code: "ConditionalCheckFailed",
        message: format!(
            "Conditional check failed. Attribute ({name}) value is ({actual}) but was \
             expected ({expected})"
        ),
    }
}

/// The refusal for a conditional write that expected an attribute NOT to exist, where it
/// does.
pub fn conditional_check_failed_exists(name: &str) -> QueryApiError {
    QueryApiError {
        status: CONFLICT_STATUS,
        code: "ConditionalCheckFailed",
        message: format!("Conditional check failed. Attribute ({name}) value exists."),
    }
}

/// The refusal for a conditional write whose expected attribute the item does not hold.
pub fn attribute_does_not_exist(name: &str) -> QueryApiError {
    QueryApiError {
        status: NOT_FOUND_STATUS,
        code: "AttributeDoesNotExist",
        message: format!("Attribute ({name}) does not exist"),
    }
}

/// The refusal for a conditional write on an attribute holding more than one value.
pub fn multi_valued_attribute(name: &str) -> QueryApiError {
    QueryApiError {
        status: CONFLICT_STATUS,
        code: "MultiValuedAttribute",
        message: format!(
            "Attribute ({name}) is multi valued. Conditional check can only be performed \
             on a single-valued attribute"
        ),
    }
}

/// The refusal for a condition that both expects a value and expects the attribute not to
/// exist.
pub fn exists_and_expected_value() -> QueryApiError {
    QueryApiError {
        status: CLIENT_ERROR_STATUS,
        code: "ExistsAndExpectedValue",
        message: "Expected.Exists=false and Expected.Value cannot be specified together"
            .to_string(),
    }
}

/// The refusal for a condition that expects an attribute to exist without saying what
/// value it should hold.
pub fn incomplete_expected_expression() -> QueryApiError {
    QueryApiError {
        status: CLIENT_ERROR_STATUS,
        code: "IncompleteExpectedExpression",
        message: "If Expected.Exists=true or unspecified, then Expected.Value has to be \
                  specified"
            .to_string(),
    }
}

/// The document one action answers with: the action's own response element, the result it
/// reports (when the action reports one), and the `ResponseMetadata` every answer carries.
///
/// `action` is this service's own name for the action, never the caller's text, so the
/// element names are fixed by this crate. `result` is already-rendered XML — built by
/// [`element`] and its callers — and goes inside the `<<action>Result>` wrapper an SDK
/// unwraps.
pub fn response(action: &str, result: Option<String>, request_id: &str) -> RtResponse {
    let result = match result {
        Some(inner) => format!("<{action}Result>{inner}</{action}Result>"),
        None => String::new(),
    };
    let body = format!(
        "<?xml version=\"1.0\"?><{action}Response xmlns=\"{namespace}\">{result}\
         <ResponseMetadata><RequestId>{request_id}</RequestId>\
         <BoxUsage>{BOX_USAGE}</BoxUsage></ResponseMetadata></{action}Response>",
        namespace = xml_escape(SDB_NAMESPACE),
        request_id = xml_escape(request_id),
    );
    xml(200, body, request_id)
}

/// The document a refusal answers with: this API's `<Response><Errors><Error>` envelope,
/// at the status the refused code carries.
pub fn error_response(error: &QueryApiError, request_id: &str) -> RtResponse {
    let body = format!(
        "<?xml version=\"1.0\"?><Response><Errors><Error><Code>{code}</Code>\
         <Message>{message}</Message><BoxUsage>{BOX_USAGE}</BoxUsage></Error></Errors>\
         <RequestID>{request_id}</RequestID></Response>",
        code = xml_escape(error.code),
        message = xml_escape(&error.message),
        request_id = xml_escape(request_id),
    );
    xml(error.status, body, request_id)
}

/// One XML element carrying text: `<Name>text</Name>`, with the text escaped.
///
/// `name` is a literal from this crate; only the text is caller- or state-derived.
pub fn element(name: &str, text: &str) -> String {
    format!("<{name}>{}</{name}>", xml_escape(text))
}

/// An XML answer at `status`, naming the request it answers in the header AWS reports
/// that id in as well as in the document.
///
/// The header is written by the one writer every service here states a request id
/// through (`service_common::query_error_envelope`), with the id this document already
/// carries, so a client correlating on the header and one reading the document see the
/// same value.
fn xml(status: u16, body: String, request_id: &str) -> RtResponse {
    let mut response = RtResponse::new(
        StatusCode::try_from(status).expect("every status this module names is a valid constant"),
        SdkBody::from(body),
    );
    response
        .headers_mut()
        .insert("content-type", XML_CONTENT_TYPE);
    ::service_common::query_error_envelope::with_stated_request_id_header(response, request_id)
}
