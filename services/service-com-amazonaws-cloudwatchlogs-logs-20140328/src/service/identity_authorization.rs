// SPDX-License-Identifier: CC0-1.0

//! Whether IAM lets the principal that SIGNED a request perform the action it asked for —
//! the authorization step taken in FRONT of every operation, before anything is read or
//! written.
//!
//! Every operation of this API is signed with IAM credentials (`sigv4` is the only auth
//! scheme the model declares, `smithy_query(query="service_info")`), and AWS decides such a
//! call at the IAM layer before it is dispatched: a caller whose identity-based policies do
//! not allow the action on the resource the request names is told it is not authorized
//! rather than told anything about what the account holds, and nothing of the account
//! changes on the way to that refusal. The decision is taken per request, so a statement
//! `iam:PutRolePolicy` wrote refuses the next call and one `iam:DeleteRolePolicy` removed
//! stops granting it — which is what makes a client's least-privilege test, and a
//! revocation drill, mean anything here.
//!
//! No error shape of this model carries that refusal for most of its operations — the error
//! sets in the crate root's `OPERATIONS.md` declare `AccessDeniedException` on a minority of
//! them — because the refusal belongs to the authorization layer ahead of the service's own
//! code rather than to any one operation: it is the AWS-common `AccessDeniedException`,
//! answered here through this service's own scaffold channel
//! ([`crate::scaffolded::ACCESS_DENIED_CODE`], HTTP
//! [`crate::scaffolded::ACCESS_DENIED_STATUS`]) so a caller reads the same code and status
//! whichever operation it called.
//!
//! Three pieces, each held in exactly one place:
//!
//! 1. **who it is, what it may do, and the decision** —
//!    [`crate::state::identity_authorization`], the one place this service resolves the
//!    principal an access key id speaks for, reads the identity-based documents and the
//!    permissions boundary in force on it, and asks
//!    [`::service_common::iam_authorization`] about an `(action, resource)` pair. The
//!    callers it does not bound — the account's own published credential, which every
//!    in-process call another emulated service makes to this one presents, a principal
//!    carrying a grant this emulator cannot read, and an engine with no STS or IAM store —
//!    are named there, as is the sentence a refusal carries. Nothing is cached, so a policy
//!    a write to IAM changed decides the next call;
//! 2. **what the request asks for** — the action is the operation's own name under this
//!    service's namespace ([`crate::state::identity_authorization::action_for`]) and the
//!    resource is read off the request alone here ([`addressed_resource`]) before anything
//!    is looked up, so the decision cannot depend on whether that resource exists and a
//!    refused caller learns nothing about what the account holds;
//! 3. **where the decision is taken** — this module's [`IamAuthorized`], in front of the
//!    dispatch [`crate::service::EmulatedService::override_operation`] hands the router.
//!    Wrapping is what makes it ONE decision: every operation this service serves is judged
//!    by the same rule before its handler, so no handler can forget it.
//!
//! ## The resource a request is judged against
//!
//! CloudWatch Logs names the resource of an API call of its own as the log GROUP and log
//! STREAM that call addresses: `arn:<partition>:logs:<region>:<account>:log-group:<group>:log-stream:<stream>`,
//! with a segment left EMPTY where the request names none — which is the resource its
//! denials state, and the form a grant written `log-group:<group>:*` covers. So:
//!
//! * a request names the group it addresses in a member of its own: `logGroupName`, the
//!   name, or `logGroupIdentifier`, "the name or ARN of the log group" (the two members this
//!   model names a log group by), and the stream in `logStreamName`;
//! * an identifier that already carries an ARN names that group itself, whichever account
//!   and Region holds it, so it is judged as carried;
//! * a request that names an ARN OF THIS SERVICE in `resourceArn` addresses that resource —
//!   a tagging call, whose `AmazonResourceName` is the resource being tagged — so it too is
//!   judged as carried. An ARN of ANOTHER service carried there is not a resource of this
//!   one (a delivery source's `resourceArn` is the resource that SENDS the logs), so it
//!   decides nothing;
//! * a request that names none of them — a listing, a call addressed by a query id or a task
//!   token, a call about one of this service's other resource kinds — is judged against that
//!   same ARN with both segments empty, which is the account's log groups rather than one
//!   record: a statement covering them allows such a call and one scoped to a single group
//!   does not.
//!
//! Every part comes from the request and from the credential scope it was signed under, so
//! nothing is looked up to decide a call and a refused caller learns nothing about what the
//! account holds.
//!
//! ## The operations that take the decision themselves
//!
//! [`DECIDED_BY_THE_OPERATION`] names the operations this gate does not judge — because the
//! call acts on SEVERAL resources, or on one that is not a member of the request, so the
//! handler is the only place that knows which ARNs the decision is about. This gate defers to
//! them rather than judging the same call twice against a resource their handler would not
//! use. Where such a handler can name the resource, it asks
//! [`crate::state::identity_authorization::refusal`] itself and answers the same sentence,
//! from the same evaluator, through its own modelled `AccessDeniedException`; where the
//! request carries no resolvable resource at all, there is nothing to decide and the handler
//! refuses the request for what IS wrong with it — the order AWS answers in, since a call
//! whose target cannot be resolved is not a call on any resource.

use ::std::sync::Arc;

use ::aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use ::aws_smithy_schema_server::protocol::ServerProtocol;
use ::smithy_server_runtime::{BoxFuture, ErasedOperation, OperationContext, Service};

use crate::state::identity_authorization::refusal;

/// The member a request names the log group it addresses by.
const LOG_GROUP_NAME_MEMBER: &str = "logGroupName";

/// The member a request names the log group it addresses by when the model lets it carry
/// either the name or the ARN ("the name or ARN of the log group").
const LOG_GROUP_IDENTIFIER_MEMBER: &str = "logGroupIdentifier";

/// The member a request names the log stream it addresses by.
const LOG_STREAM_NAME_MEMBER: &str = "logStreamName";

/// The member a request names the resource it addresses by its ARN
/// (`AmazonResourceName`) — what a tagging call of this service is addressed by.
const RESOURCE_ARN_MEMBER: &str = "resourceArn";

/// The operations this gate does not judge, because the resource the call addresses is not a
/// member of the request (see the module docs).
///
/// SOURCE, for adding an entry: a handler that asks
/// [`crate::state::identity_authorization::refusal`] itself, or one whose request names no
/// resolvable resource for a decision to be about. Sorted, which
/// [`decided_by_the_operation`] relies on for `binary_search`.
pub const DECIDED_BY_THE_OPERATION: &[&str] = &[
    // "Creates a delivery … by pairing exactly one delivery source and one delivery
    // destination": the call acts on BOTH, so its handler decides it against each of them
    // in the order the request names them.
    "CreateDelivery",
    // The resource this call addresses is named by NOTHING in the request except the
    // `logObjectPointer` it carries: the pointer is the service's own handle on one large
    // log object, so which resource the call reaches is whatever the pointer decodes to. A
    // pointer this service did not issue decodes to no resource at all, so there is nothing
    // to decide the call against and the operation refuses it as the bad parameter it is,
    // reading and writing nothing of the store on the way.
    "GetLogObject",
    // The call addresses the DELIVERY its `id` names, whose ARN its handler builds from
    // that id.
    "UpdateDeliveryConfiguration",
];

/// Whether `operation` takes the identity decision in its own handler.
#[must_use]
pub fn decided_by_the_operation(operation: &str) -> bool {
    DECIDED_BY_THE_OPERATION.binary_search(&operation).is_ok()
}

/// The resource this request is judged against: what it ADDRESSES (see the module docs).
///
/// Read off the request's own body, which on this protocol is the operation's input as
/// JSON. A body that is not a JSON document at all addresses nothing of this service, so it
/// is judged exactly as a request that named no log group.
fn addressed_resource(ctx: &OperationContext<'_>, req: &RtRequest) -> String {
    let submitted: Option<::serde_json::Value> = req
        .body()
        .bytes()
        .and_then(|body| ::serde_json::from_slice(body).ok());
    let region = crate::state::request_region(ctx.parts());
    addressed_in(submitted.as_ref(), &region)
}

/// The resource the submitted input `submitted` addresses, in `region` (see the module
/// docs).
///
/// A member that is absent, is not a string or is empty names no resource, so such a request
/// is judged exactly as one that named none at all.
fn addressed_in(submitted: Option<&::serde_json::Value>, region: &str) -> String {
    let member = |name: &str| {
        submitted
            .and_then(|body| body.get(name))
            .and_then(::serde_json::Value::as_str)
            .filter(|value| !value.is_empty())
    };
    let named = member(LOG_GROUP_NAME_MEMBER).or_else(|| member(LOG_GROUP_IDENTIFIER_MEMBER));
    // An identifier that is already an ARN names the group itself, whichever account and
    // Region holds it, so it is judged as carried rather than rebuilt here.
    if let Some(identifier) = named.filter(|value| is_arn(value)) {
        return identifier.to_string();
    }
    // A request carrying no log group at all, but naming an ARN of THIS service as the
    // resource it addresses, is judged against that ARN (see the module docs).
    if let Some(resource) = named
        .is_none()
        .then(|| member(RESOURCE_ARN_MEMBER).filter(|value| is_own_arn(value)))
        .flatten()
    {
        return resource.to_string();
    }
    log_group_resource(
        region,
        named.unwrap_or_default(),
        member(LOG_STREAM_NAME_MEMBER).unwrap_or_default(),
    )
}

/// Whether `value` is an ARN at all.
fn is_arn(value: &str) -> bool {
    crate::state::arn_service_and_resource(value).is_some()
}

/// Whether `value` is an ARN of THIS service — one whose vendor field is this service's own
/// namespace, which is what makes it a resource this service's actions are decided against.
fn is_own_arn(value: &str) -> bool {
    crate::state::arn_service_and_resource(value)
        .is_some_and(|(service, _)| service == crate::state::identity_authorization::ACTION_PREFIX)
}

/// The resource ARN CloudWatch Logs names a call on the log group `group` and the log stream
/// `stream` by, with a segment left empty where the request named none (see the module
/// docs).
fn log_group_resource(region: &str, group: &str, stream: &str) -> String {
    ::engine::emulator_arn(
        crate::state::identity_authorization::ACTION_PREFIX,
        region,
        &format!("log-group:{group}:log-stream:{stream}"),
    )
}

/// Every operation of this service, served only to a caller IAM allows to perform it.
///
/// It holds no operation of its own beyond the dispatch it wraps: the one it serves a
/// permitted request through is the dispatch this service already answers
/// `Service::override_operation` with, so the framework still owns the whole deserialize ->
/// validate -> handle -> serialize sequence and every status, code and message it answers
/// with.
pub struct IamAuthorized<Eng> {
    /// The shared engine, so the principal's policies are read from the one store every
    /// service of this emulator shares.
    engine: Arc<Eng>,
    /// Serves the request the decision admitted.
    served: Box<dyn ErasedOperation>,
}

impl<Eng> IamAuthorized<Eng> {
    /// Put `served`, already constructed from the shared engine, behind the IAM decision.
    pub fn new(engine: Arc<Eng>, served: Box<dyn ErasedOperation>) -> Self {
        Self { engine, served }
    }

    /// The refusal as this endpoint puts it on the wire: the AWS-common
    /// `AccessDeniedException`, rendered in the same `awsJson1_1` envelope every other error
    /// of this endpoint is rendered in.
    fn denied_response(&self, message: &str) -> RtResponse {
        let refused = crate::scaffolded::AccessDeniedException::new(message);
        let rendered = ::aws_smithy_schema_server::rpc::AwsJson11().serialize_error(
            &refused,
            &crate::scaffolded::ACCESS_DENIED_SCHEMA,
            crate::scaffolded::ACCESS_DENIED_CODE,
            crate::scaffolded::ACCESS_DENIED_STATUS,
        );
        match rendered {
            Ok(response) => response,
            // The envelope is built from this crate's own schema and a message of its own,
            // so it cannot fail to render; a failure is still answered rather than leaving
            // the caller waiting, and it is answered as a REFUSAL, never as a success.
            Err(failed) => {
                ::tracing::warn!(
                    error = %failed,
                    "the authorization refusal could not be rendered in this service's envelope"
                );
                RtResponse::new(
                    ::aws_smithy_runtime_api::http::StatusCode::try_from(
                        crate::scaffolded::ACCESS_DENIED_STATUS,
                    )
                    .expect("the client-error status of this service is a status"),
                    ::aws_smithy_types::body::SdkBody::from(
                        format!(
                            "{{\"__type\":\"{}\"}}",
                            crate::scaffolded::ACCESS_DENIED_CODE
                        )
                        .as_bytes(),
                    ),
                )
            }
        }
    }
}

impl<Eng: Send + Sync + 'static> ErasedOperation for IamAuthorized<Eng> {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            // WHICH operation this is, read off the routing key this protocol resolves — the
            // same key the generated table is keyed by, so the action asked about cannot
            // drift from the operation served. A request the key cannot be read from names
            // no operation of this service and is left to the framework, which answers it as
            // an unroutable request.
            if let Ok(operation) =
                ::aws_smithy_schema_server::rpc::routing_key_from_x_amz_target(req)
            {
                if !decided_by_the_operation(operation) {
                    let resource = addressed_resource(ctx, req);
                    if let Some(message) =
                        refusal(&*self.engine, ctx, operation, &resource)
                    {
                        ::tracing::info!(
                            operation = %operation,
                            resource = %resource,
                            "iam does not allow this caller to perform this action"
                        );
                        return self.denied_response(&message);
                    }
                }
            }
            self.served.run(svc, req, ctx).await
        })
    }

    /// The wrapped dispatch's own answer: `None` is "defer to the modelled operation the
    /// routing key names", so wrapping must not change which schemes sign an operation.
    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        self.served.auth_schemes()
    }
}

#[cfg(test)]
mod tests {
    use ::serde_json::json;

    use super::*;
    use crate::state::identity_authorization::action_for;

    /// The Region the ARNs below are read in.
    const REGION: &str = "us-west-2";

    /// RULE: a request is judged against the log group and log stream it NAMES — by name, or
    /// by the ARN an identifier carries — against the ARN OF THIS SERVICE it names as the
    /// resource it addresses, and otherwise against that same ARN with the group and stream
    /// segments left empty.
    ///
    /// The rows span both members this model names a log group by, a name and an ARN in the
    /// member that admits either, a request naming a stream as well, a tagging call addressed
    /// by this service's own ARN, one carrying ANOTHER service's ARN there, the three ways a
    /// request names no group (no member, an empty one, one carrying another type), a group of
    /// ANOTHER account reached by ARN, and operations the issue this rule was written for did
    /// not cite.
    #[test]
    fn a_request_is_judged_against_the_log_group_it_names() {
        let group = "/aws/lambda/handler";
        let elsewhere = "arn:aws:logs:eu-central-1:210987654321:log-group:shared:*";
        let addressed_arn = |group: &str, stream: &str| {
            format!(
                "arn:aws:logs:{REGION}:{}:log-group:{group}:log-stream:{stream}",
                ::engine::emulator_account_id()
            )
        };
        // (case, the request body, the resource it addresses)
        let rows: Vec<(&str, ::serde_json::Value, String)> = vec![
            (
                "a write addressed by log group name and stream name",
                json!({ "logGroupName": group, "logStreamName": "2026/01/01/[$LATEST]a" }),
                addressed_arn(group, "2026/01/01/[$LATEST]a"),
            ),
            (
                "a group-level call naming no stream",
                json!({ "logGroupName": group, "retentionInDays": 7 }),
                addressed_arn(group, ""),
            ),
            (
                "a read addressed by log group identifier",
                json!({ "logGroupIdentifier": group }),
                addressed_arn(group, ""),
            ),
            (
                "an identifier carrying the group's own ARN",
                json!({ "logGroupIdentifier": elsewhere }),
                elsewhere.to_string(),
            ),
            (
                "a name beside an identifier",
                json!({ "logGroupName": group, "logGroupIdentifier": elsewhere }),
                addressed_arn(group, ""),
            ),
            (
                "a listing that names no log group",
                json!({ "logGroupNamePrefix": "/aws/" }),
                addressed_arn("", ""),
            ),
            (
                "a tagging call addressed by this service's own ARN",
                json!({ "resourceArn": addressed_arn(group, "") }),
                addressed_arn(group, ""),
            ),
            (
                "a call carrying another service's ARN as its resource",
                json!({ "name": "vended", "resourceArn": "arn:aws:s3:::vended-logs" }),
                addressed_arn("", ""),
            ),
            (
                "a member carrying no text at all",
                json!({ "logGroupName": "" }),
                addressed_arn("", ""),
            ),
            (
                "a member carrying a value of another type",
                json!({ "logGroupName": 7 }),
                addressed_arn("", ""),
            ),
            (
                "a call addressed by a query id",
                json!({ "queryId": "9f1b" }),
                addressed_arn("", ""),
            ),
        ];
        for (case, body, expected) in rows {
            let addressed = addressed_in(Some(&body), REGION);
            assert_eq!(
                addressed, expected,
                "case {case:?}: expected the request to address {expected:?}, got {addressed:?}"
            );
        }
        // A body that is no JSON document at all names nothing either.
        let unreadable = addressed_in(None, REGION);
        let expected = addressed_arn("", "");
        assert_eq!(
            unreadable, expected,
            "case \"a body that is not a JSON document\": expected {expected:?}, got \
             {unreadable:?}"
        );
    }

    /// RULE: this gate judges every operation of the service except the ones whose own
    /// handler takes the decision.
    #[test]
    fn the_gate_defers_only_to_the_operations_that_decide_themselves() {
        let mut sorted = DECIDED_BY_THE_OPERATION.to_vec();
        sorted.sort_unstable();
        assert_eq!(
            sorted,
            DECIDED_BY_THE_OPERATION.to_vec(),
            "the deferral list is searched with binary_search, so it must stay sorted"
        );
        for operation in DECIDED_BY_THE_OPERATION {
            assert!(
                decided_by_the_operation(operation),
                "case {operation:?}: expected the gate to defer to the operation"
            );
        }
        for operation in [
            "DescribeLogGroups",
            "PutLogEvents",
            "GetDelivery",
            "PutDeliverySource",
            "TagResource",
        ] {
            assert!(
                !decided_by_the_operation(operation),
                "case {operation:?}: expected the gate to decide the request itself"
            );
        }
    }

    /// RULE: the action asked about is the operation's own name under this service's
    /// namespace, so one statement scoped to this service's actions covers every operation
    /// of it.
    #[test]
    fn the_action_asked_about_is_the_operations_own_name() {
        for (case, operation, expected) in [
            (
                "the listing the gate was written for",
                "DescribeLogGroups",
                "logs:DescribeLogGroups",
            ),
            (
                "an operation the issue did not cite",
                "PutRetentionPolicy",
                "logs:PutRetentionPolicy",
            ),
        ] {
            let action = action_for(operation);
            assert_eq!(
                action, expected,
                "case {case:?}: expected the action {expected:?}, got {action:?}"
            );
        }
    }
}
