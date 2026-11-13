// SPDX-License-Identifier: CC0-1.0

//! The `AWS` integration whose `uri` names AMAZON DYNAMODB: how a method request becomes one
//! table action, made in process.
//!
//! `Integration$uri` gives the form that action is named in —
//! `arn:aws:apigateway:{region}:{subdomain.service|service}:path|action/{service_api}` — and
//! Amazon DynamoDB's own api is a JSON-RPC one (`awsJson1_0`): a request names its operation in
//! the `X-Amz-Target` header and carries every input of it in a JSON object body. So an
//! integration to a table is the simplest of the three this module makes:
//!
//! * the `action/{Action}` form of the `uri` names the OPERATION, which is the only thing the
//!   `uri` decides — this api fixes no input in a query parameter, because its inputs all
//!   travel in the body;
//! * the integration request's own BODY — the request template this call selects, RENDERED
//!   (`Integration$requestTemplates`) — IS the operation's input document, which is why an
//!   integration to a table carries a template that writes one.
//!
//! The call is made THROUGH THE EMULATOR this service runs within, over the in-process
//! transport `OperationContext::callback` hands a handler: nothing is invented and no
//! connection is opened. What the table service answered in process is what the method
//! response carries — its status, its media type and its bytes, refusals included — for the
//! integration response to map, so a template written against the service's own answer
//! (`$input.path('$.Item.desired.N')`) reads exactly what that service sent.
//!
//! Who the call is made as — the role `Integration$credentials` names — and what a role that
//! may not make it earns is the one rule the parent module states for every `AWS` integration,
//! and this module only names the action it is authorized as ([`TableCall::iam_action`]), the
//! resource it is authorized on ([`TableCall::table_arn`]) and the refusal
//! ([`access_denied`]).

use ::serde_json::Value;
use ::smithy_server_runtime::OperationContext;

use crate::ops::types::GetIntegrationOutput;

use super::super::{MethodRequest, UnservedIntegration};
use super::{
    bounded, integration_request_body, substituted, unanswered, Answered, ACTION_FORM_PREFIX,
    APIGATEWAY_VENDOR,
};

/// The subdomain field that names Amazon DynamoDB, from the form `Integration$uri` documents.
const DYNAMODB_SUBDOMAIN: &str = "dynamodb";

/// What Amazon DynamoDB is called in the sentence an integration that reached no answer
/// carries.
const DYNAMODB_SERVICE_NAME: &str = "Amazon DynamoDB";

/// The service shape an `awsJson1_0` `X-Amz-Target` names an operation under, and the signing
/// name the call is signed with (that model's service shape `DynamoDB_20120810` and
/// `sigv4Name: dynamodb`, `smithy_query(query="service_info")`).
const DYNAMODB_TARGET_PREFIX: &str = "DynamoDB_20120810";

/// The media type an `awsJson1_0` request and its answer carry.
const AWS_JSON_1_0: &str = "application/x-amz-json-1.0";

/// The member every table action names its table in, which is the resource the call is
/// authorized on (`TableName`, required on each of them).
const TABLE_NAME_MEMBER: &str = "TableName";

/// The status Amazon DynamoDB refuses a call the caller may not make with: its api answers
/// every client-side failure, `AccessDeniedException` among them, with this status and names
/// the class in the body.
const DYNAMODB_REFUSAL_STATUS: u16 = 400;

/// The shape Amazon DynamoDB names that refusal by.
const DYNAMODB_ACCESS_DENIED_TYPE: &str = "AccessDeniedException";

/// One Amazon DynamoDB call an `AWS` integration addresses: the operation, the table it
/// addresses and the document the integration request carries.
#[derive(Debug)]
pub(super) struct TableCall {
    /// The partition of the integration ARN, which the table ARN is scoped to.
    partition: String,
    /// The Region the call is made at: the integration ARN's own, else the api's.
    region: String,
    /// The operation the `action` form names, e.g. `GetItem`.
    action: String,
    /// The table the integration request names, empty for a request that names none.
    table: String,
    /// The integration request's body: the rendered request template, which IS the
    /// operation's input document.
    body: Vec<u8>,
}

impl TableCall {
    /// The Region of the service endpoint the call is made at.
    pub(super) fn region(&self) -> &str {
        &self.region
    }

    /// The IAM action a call of it is authorized by: `dynamodb:{Action}`, the operation the
    /// `uri` names under that service's own action namespace (`arnNamespace: dynamodb`).
    pub(super) fn iam_action(&self) -> String {
        format!("{DYNAMODB_SUBDOMAIN}:{}", self.action)
    }

    /// The ARN of the table the call addresses, which is what an identity-based policy grants
    /// that action on.
    ///
    /// A request that names no table — the batch and the account-wide actions do not — is
    /// authorized on every table of the account, which is the resource such a call addresses.
    pub(super) fn table_arn(&self) -> String {
        let table = match self.table.is_empty() {
            true => "*",
            false => &self.table,
        };
        format!(
            "arn:{partition}:{DYNAMODB_SUBDOMAIN}:{region}:{account}:table/{table}",
            partition = self.partition,
            region = self.region,
            account = ::emulator_server::EMULATOR_ACCOUNT_ID,
        )
    }
}

/// What `integration`'s `uri` addresses in Amazon DynamoDB:
///
/// * `None` when the `uri` names no Amazon DynamoDB call at all — another service's action, or
///   no ARN — which leaves it to the next reader;
/// * `Some(Err(unserved))` when it DOES name this service and no call can be placed against
///   it, which is a configuration this stage cannot serve;
/// * `Some(Ok(call))` for the call to make.
pub(super) fn addressed_table(
    integration: &GetIntegrationOutput,
    default_region: &str,
    call: &MethodRequest,
) -> Option<Result<TableCall, UnservedIntegration>> {
    let uri = integration.uri.as_deref()?;
    let arn = ::service_common::arn::parse(uri)?;
    if arn.vendor != APIGATEWAY_VENDOR || arn.account != DYNAMODB_SUBDOMAIN {
        return None;
    }
    Some(resolved(integration, &arn, default_region, call))
}

/// The call the integration ARN `arn` names, with every `{placeholder}` of its resource
/// resolved against this request and the integration request's own document rendered.
fn resolved(
    integration: &GetIntegrationOutput,
    arn: &::service_common::arn::Arn<'_>,
    default_region: &str,
    call: &MethodRequest,
) -> Result<TableCall, UnservedIntegration> {
    let region = match arn.region.is_empty() {
        true => default_region,
        false => arn.region,
    };
    let resource = substituted(arn.resource, integration, &call.inputs).map_err(|reason| {
        unanswered(&reason)
    })?;
    // Only the ACTION form addresses this api: the `path` form is a service's own REST path,
    // and this one publishes none — every operation of it is reached at the service's root.
    let Some(named) = resource.strip_prefix(ACTION_FORM_PREFIX) else {
        return Err(unanswered(
            "the integration's uri does not address the action form of an Amazon DynamoDB \
             call, which is the only form that api is reached by",
        ));
    };
    // Nothing of the resource past the action name is read: the `&{parameter}={value}` tail
    // names inputs of a QUERY api, and every input of this one travels in the body.
    let action = named.split('&').next().unwrap_or_default();
    if action.is_empty() || !action.chars().all(char::is_alphanumeric) {
        return Err(unanswered(&format!(
            "the integration's uri names {action:?}, which is no Amazon DynamoDB operation"
        )));
    }
    let body = integration_request_body(integration, call)?;
    Ok(TableCall {
        partition: arn.partition.to_string(),
        region: region.to_string(),
        action: action.to_string(),
        table: named_table(&body),
        body,
    })
}

/// The table the integration request's document names, or empty for a document that names
/// none — which is every action whose inputs name no single table.
fn named_table(body: &[u8]) -> String {
    ::serde_json::from_slice::<Value>(body)
        .ok()
        .as_ref()
        .and_then(|document| document.get(TABLE_NAME_MEMBER))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// What Amazon DynamoDB answers a call the integration's role may not make: the refusal that
/// api words an unauthorized call in — the principal the call would have been made as, the
/// action it asked for, the table it addressed and the clause IAM closes an implicit deny
/// with, through the tree's one formatter for that sentence
/// ([`::service_common::iam_authorization::not_authorized_by_identity_policy`]).
pub(super) fn access_denied(call: &TableCall, principal: &str) -> Answered {
    let action = call.iam_action();
    let body = ::serde_json::json!({
        "__type": format!("com.amazonaws.{DYNAMODB_SUBDOMAIN}#{DYNAMODB_ACCESS_DENIED_TYPE}"),
        "message": ::service_common::iam_authorization::not_authorized_by_identity_policy(
            principal,
            &action,
            &call.table_arn(),
            &::service_common::iam_authorization::no_identity_policy_allows(&action),
        ),
    });
    Answered {
        status: DYNAMODB_REFUSAL_STATUS,
        content_type: Some(AWS_JSON_1_0.to_string()),
        body: body.to_string().into_bytes(),
    }
}

/// Make the call `table` addresses against Amazon DynamoDB, in process, and report what it
/// answered.
///
/// The request is the one that api takes: its operation in the `X-Amz-Target` header, the
/// integration request's document as the body, signed for this service in the call's Region by
/// the tree's one in-process signer with the credential the callback hands this handler —
/// which is never reported or logged.
///
/// `Err` only where there is no answer FROM THE SERVICE to report at all — the request could
/// not be built or signed, it reached no Amazon DynamoDB in this emulator, or the answer is
/// larger than one method response may carry. Every answer the service DID give, refusals
/// included, is reported as [`Answered`] and mapped.
pub(super) async fn answered(
    ctx: &OperationContext<'_>,
    table: &TableCall,
) -> Result<Answered, UnservedIntegration> {
    let host = format!("{DYNAMODB_SUBDOMAIN}.{}.amazonaws.com", table.region);
    let built = ::http::Request::builder()
        .method("POST")
        .uri(format!("https://{host}/"))
        .header(::http::header::HOST, &host)
        .header(::http::header::CONTENT_TYPE, AWS_JSON_1_0)
        .header(
            "x-amz-target",
            format!("{DYNAMODB_TARGET_PREFIX}.{}", table.action),
        )
        .body(table.body.clone());
    let mut request = built.map_err(|error| {
        ::tracing::warn!(%error, "the integration's Amazon DynamoDB call does not render an \
                                  HTTP request");
        unanswered(
            "the integration's uri does not render a request to Amazon DynamoDB, so no call \
             was made",
        )
    })?;
    ::service_common::in_process_sigv4::sign_in_process_request(
        &mut request,
        DYNAMODB_SUBDOMAIN,
        &table.region,
        ctx,
    )
    .await
    .map_err(|error| {
        ::tracing::warn!(%error, "the integration's Amazon DynamoDB call could not be signed");
        unanswered(
            "the integration request to Amazon DynamoDB could not be signed, so no call was made",
        )
    })?;
    let (parts, body) = request.into_parts();
    let request = ::aws_smithy_runtime_api::http::Request::try_from(::http::Request::from_parts(
        parts,
        ::aws_smithy_types::body::SdkBody::from(::bytes::Bytes::from(body)),
    ))
    .map_err(|error| {
        ::tracing::warn!(%error, "the integration's Amazon DynamoDB call does not render an \
                                  HTTP request");
        unanswered(
            "the integration's uri does not render a request to Amazon DynamoDB, so no call \
             was made",
        )
    })?;
    let answer = ::aws_smithy_runtime_api::client::http::HttpConnector::call(
        &ctx.callback().http_connector(),
        request,
    )
    .await
    .map_err(|error| {
        ::tracing::warn!(
            error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
            "the integration request was not answered by Amazon DynamoDB"
        );
        unanswered(&format!(
            "the integration request to {DYNAMODB_SERVICE_NAME} was not answered, so there is \
             no backend answer to map"
        ))
    })?;
    let status = answer.status().as_u16();
    let content_type = answer
        .headers()
        .get(::http::header::CONTENT_TYPE.as_str())
        .map(str::to_string);
    let payload = answer.body().bytes().unwrap_or_default().to_vec();
    // The emulator's own FRONT DOOR refusal is not an answer from the service: a deployment
    // with Amazon DynamoDB not mounted answers the integration request that way, and mapping
    // it as a backend answer would hand the caller a status and a body for a call nothing
    // made. `service_common` owns the one reading of that refusal.
    if ::service_common::in_process_refusal::is_transport_refusal(&payload) {
        ::tracing::warn!(
            status,
            "the integration request reached no such service: this emulator mounts none, so \
             there is no backend answer to map"
        );
        return Err(unanswered(&format!(
            "the integration request reached no {DYNAMODB_SERVICE_NAME} in this emulator, so \
             there is no backend answer to map"
        )));
    }
    bounded(payload.len())?;
    Ok(Answered {
        status,
        content_type: content_type.or_else(|| Some(AWS_JSON_1_0.to_string())),
        body: payload,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `TableCall` of `action` on `table`, as [`resolved`] builds one.
    fn call(action: &str, table: &str) -> TableCall {
        TableCall {
            partition: "aws".to_string(),
            region: "eu-west-1".to_string(),
            action: action.to_string(),
            table: table.to_string(),
            body: Vec::new(),
        }
    }

    /// RULE: the call is authorized as the operation the `uri` names, on the table the
    /// integration request names — and on every table of the account for a request that names
    /// none.
    #[test]
    fn the_call_is_authorized_as_its_operation_on_the_table_it_names() {
        /// (case, the action, the table, the IAM action, the resource)
        type Row<'a> = (&'a str, &'a str, &'a str, &'a str, &'a str);
        let rows: &[Row<'_>] = &[
            (
                "a read of one table",
                "GetItem",
                "capacity",
                "dynamodb:GetItem",
                "arn:aws:dynamodb:eu-west-1:000000000000:table/capacity",
            ),
            (
                "a write of one table",
                "UpdateItem",
                "capacity",
                "dynamodb:UpdateItem",
                "arn:aws:dynamodb:eu-west-1:000000000000:table/capacity",
            ),
            (
                "a request that names no table",
                "ListTables",
                "",
                "dynamodb:ListTables",
                "arn:aws:dynamodb:eu-west-1:000000000000:table/*",
            ),
        ];
        for (case, action, table, expected_action, expected_resource) in rows {
            let call = call(action, table);
            assert_eq!(
                call.iam_action(),
                *expected_action,
                "case {case:?}: expected the action {expected_action:?}, got {:?}",
                call.iam_action()
            );
            assert_eq!(
                call.table_arn(),
                *expected_resource,
                "case {case:?}: expected the resource {expected_resource:?}, got {:?}",
                call.table_arn()
            );
        }
    }

    /// RULE: the table a call addresses is the one its own input document names.
    #[test]
    fn the_table_is_the_one_the_integration_request_names() {
        /// (case, the integration request body, the table it names)
        type Row<'a> = (&'a str, &'a str, &'a str);
        let rows: &[Row<'_>] = &[
            (
                "a read's input document",
                r#"{"TableName":"capacity","Key":{"id":{"S":"a"}}}"#,
                "capacity",
            ),
            (
                "a write's input document",
                r#"{"TableName":"other","Item":{"id":{"S":"a"}}}"#,
                "other",
            ),
            ("a document naming no table", r#"{"Limit":10}"#, ""),
            ("a body that is no document at all", "not json", ""),
            ("no body at all", "", ""),
        ];
        for (case, body, expected) in rows {
            let actual = named_table(body.as_bytes());
            assert_eq!(
                actual, *expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// RULE: a refusal of the integration's role carries the class Amazon DynamoDB words an
    /// unauthorized call in, naming the principal, the action and the table.
    #[test]
    fn an_unauthorized_call_is_refused_the_way_the_service_words_it() {
        const ROLE: &str = "arn:aws:iam::000000000000:role/apigateway-dynamodb";
        let refused = access_denied(&call("PutItem", "capacity"), ROLE);
        assert_eq!(
            refused.status, DYNAMODB_REFUSAL_STATUS,
            "expected the status that api refuses a client-side failure with, got {}",
            refused.status
        );
        let document: Value = ::serde_json::from_slice(&refused.body)
            .expect("the refusal carries the service's own JSON envelope");
        assert_eq!(
            document["__type"],
            Value::String(format!(
                "com.amazonaws.dynamodb#{DYNAMODB_ACCESS_DENIED_TYPE}"
            )),
            "expected the class the service names it by, got {document:?}"
        );
        let message = document["message"].as_str().unwrap_or_default();
        assert_eq!(
            message,
            format!(
                "User: {ROLE} is not authorized to perform: dynamodb:PutItem on resource: \
                 arn:aws:dynamodb:eu-west-1:000000000000:table/capacity because no \
                 identity-based policy allows the dynamodb:PutItem action"
            ),
            "expected the sentence IAM words an authorization failure in, got {message:?}"
        );
    }
}
