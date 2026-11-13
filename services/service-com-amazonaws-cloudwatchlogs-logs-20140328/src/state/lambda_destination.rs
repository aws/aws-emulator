// SPDX-License-Identifier: CC0-1.0

//! Whether the Lambda function a subscription filter names as its destination lets CloudWatch
//! Logs invoke it for that log group.
//!
//! `PutSubscriptionFilter` takes a destination it will deliver to, and a Lambda function is the
//! one destination kind it needs no `roleArn` for ("To perform a PutSubscriptionFilter
//! operation for any destination except a Lambda function, you must also have the iam:PassRole
//! permission"): a function is INVOKED rather than written to under an assumed role, so what
//! admits the delivery is the function's own resource-based policy — the document
//! `lambda:AddPermission` writes — naming the CloudWatch Logs service principal and
//! [`INVOKE_FUNCTION_ACTION`]. A client scopes that grant to the log group it is subscribing
//! with the source-ARN condition key, which is why a permission written for ANOTHER log group
//! grants this subscription nothing.
//!
//! So the put RESOLVES the destination before it configures anything: a function whose policy
//! does not let this service invoke it for this log group could never receive a single event,
//! and the service refuses the configuration rather than storing a subscription that can only
//! be observed as silent non-delivery.
//!
//! AWS Lambda is reached the way the framework documents a handler reaching another emulated
//! service ([`smithy_server_runtime::OperationContext::callback`]): an AWS SDK client over the
//! in-process callback's HTTP client and credentials, so nothing here opens a socket or leaves
//! the host, and the only caller-supplied value that reaches the client's authority is the
//! signing Region, narrowed first ([`service_common::arn::region_label`]).
//!
//! An answer that is not Lambda reporting on the function — an emulator with no Lambda mounted
//! beside this service, a fault on its side — establishes NOTHING about it, so it refuses
//! nothing: a refusal has to be grounded in the document Lambda answered with. A function that
//! Lambda reports NO policy for is a different thing: that is Lambda stating there is no grant,
//! which is exactly the condition this gate refuses.

use ::service_common::iam_authorization::{self as iam, Decision};

use super::callback_client::callback_client;

/// The IAM action an invocation performs, which is what a grant for this service has to name
/// ("Actions defined by AWS Lambda", IAM Service Authorization Reference).
pub const INVOKE_FUNCTION_ACTION: &str = "lambda:InvokeFunction";

/// The service principal CloudWatch Logs invokes a subscription destination as, in both
/// spellings a function permission may be written for ([`super::SERVICE_PRINCIPAL`]): a policy
/// granting either names this service.
fn service_principals(region: &str) -> [String; 2] {
    [
        super::SERVICE_PRINCIPAL.to_string(),
        super::regional_service_principal(region),
    ]
}

/// The condition key naming the resource a cross-service invocation comes FROM — the log group
/// whose events the filter forwards — which is what scopes a permission to one log group.
const SOURCE_ARN_CONDITION_KEY: &str = "aws:SourceArn";

/// The condition key naming the ACCOUNT that log group belongs to.
const SOURCE_ACCOUNT_CONDITION_KEY: &str = "aws:SourceAccount";

/// The error code AWS Lambda's `GetPolicy` answers when there is no policy to report: the same
/// answer a name no function resolves to earns, and for a subscription those are the same thing
/// — there is no document granting this service the invocation.
const POLICY_ABSENT_CODE: &str = "ResourceNotFoundException";

/// The codes the in-process transport answers when no Lambda is mounted beside this service, or
/// when more than one service claims a Lambda request. Either way the call reached no Lambda,
/// so it says nothing about the function.
const TRANSPORT_REFUSALS: [&str; 2] = ["UnknownService", "AmbiguousService"];

/// The refusal CloudWatch Logs answers when the Lambda function a filter names cannot be
/// invoked by this service: fixed text for the class, naming neither the function nor the log
/// group, as the service words a destination it was not permitted to execute.
pub const INVOKE_NOT_PERMITTED_MESSAGE: &str =
    "Could not execute the lambda function. Make sure you have given CloudWatch Logs permission \
     to execute your function.";

/// `Some(message)` when `destination_arn` names a Lambda function whose own resource-based
/// policy does not let CloudWatch Logs invoke it for the log group `log_group_arn`, and `None`
/// when it does — or when the destination is not a function at all, or Lambda did not report on
/// it (see the module docs).
///
/// `region` is the Region the request was signed for, which is where a destination whose own ARN
/// names none is reached.
pub async fn refusal_for(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    destination_arn: &str,
    log_group_arn: &str,
) -> Option<String> {
    let function = function_arn(destination_arn)?;
    // The Region ends up in the endpoint URL this hop is addressed on, so it is read under the
    // Region grammar rather than as arbitrary text; the function's OWN ARN names where it
    // lives, and the request's Region is where one that names none is reached.
    let addressed = ::service_common::arn::parse(function)
        .map(|parsed| parsed.region)
        .filter(|region| !region.is_empty())
        .unwrap_or(region);
    let narrowed = ::service_common::arn::region_label(addressed)
        .or_else(|| ::service_common::arn::region_label(region))
        .unwrap_or(super::DEFAULT_REGION);
    let client = callback_client!(aws_sdk_lambda, ctx.callback(), "lambda", narrowed);
    let policy = function_policy(&client, function).await?;
    if permits_invocation(policy.as_deref(), narrowed, function, log_group_arn) {
        return None;
    }
    Some(INVOKE_NOT_PERMITTED_MESSAGE.to_string())
}

/// The ARN of the Lambda FUNCTION `destination_arn` names, or `None` for a destination of any
/// other kind: the three other kinds `PutSubscriptionFilter$destinationArn` documents carry
/// their own authorisation and are not invoked.
fn function_arn(destination_arn: &str) -> Option<&str> {
    let parsed = ::service_common::arn::parse(destination_arn)?;
    (parsed.vendor == LAMBDA_VENDOR && parsed.resource_type() == FUNCTION_RESOURCE_TYPE)
        .then_some(destination_arn)
}

/// The ARN vendor of a Lambda function, and the ARN resource type that tells a function's ARN
/// from any other of that vendor (a layer, an event source mapping).
const LAMBDA_VENDOR: &str = "lambda";
const FUNCTION_RESOURCE_TYPE: &str = "function";

/// Whether `policy` lets CloudWatch Logs invoke the function for this log group.
///
/// The decision is the ordinary IAM question — may this principal perform this action on this
/// resource? — answered by the shared evaluation of policy documents
/// ([`::service_common::iam_authorization`]) over the document the function itself reports. The
/// request carries the keys such a grant scopes its statements by, so a permission written for
/// ANOTHER log group is decided rather than left undecided; a function reporting no document at
/// all grants nothing.
///
/// A log group is named by TWO ARN forms — the `arn` the group reports, which ends in the `:*`
/// that covers the streams under it (the form the documented permission for this wiring is
/// scoped to), and the `logGroupArn` without it — and a permission may be written against
/// either, so the grant is decided under both and permits the invocation when either names it.
fn permits_invocation(
    policy: Option<&str>,
    region: &str,
    function_arn: &str,
    log_group_arn: &str,
) -> bool {
    let principals = service_principals(region);
    let published = [SOURCE_ARN_CONDITION_KEY, SOURCE_ACCOUNT_CONDITION_KEY];
    let account = super::arn_account_id(log_group_arn).map(str::to_string);
    source_arn_forms(log_group_arn).iter().any(|source_arn| {
        let mut context = vec![(SOURCE_ARN_CONDITION_KEY.to_string(), source_arn.clone())];
        if let Some(account) = account.clone() {
            context.push((SOURCE_ACCOUNT_CONDITION_KEY.to_string(), account));
        }
        let request = iam::Request::new(INVOKE_FUNCTION_ACTION, function_arn, &principals)
            .with_context(&context)
            .with_published_keys(&published);
        policy.is_some_and(|policy| iam::resource_decision(policy, &request) == Decision::Allowed)
    })
}

/// The two forms of a log group's ARN a permission may name it by: the one that ends in the
/// `:*` covering the group's streams, and the one that does not.
fn source_arn_forms(log_group_arn: &str) -> [String; 2] {
    let bare = log_group_arn.strip_suffix(":*").unwrap_or(log_group_arn);
    [format!("{bare}:*"), bare.to_string()]
}

/// What the function `arn` names answered when its resource-based policy was read:
/// `Some(Some(document))` for a function carrying one, `Some(None)` for one that carries none,
/// and `None` for a function that did not answer the read at all — the three told apart because
/// only the last leaves the destination unjudged.
async fn function_policy(
    client: &::aws_sdk_lambda::Client,
    arn: &str,
) -> Option<Option<String>> {
    let error = match client.get_policy().function_name(arn).send().await {
        Ok(answer) => return Some(answer.policy().map(ToString::to_string)),
        Err(error) => error,
    };
    let code = ::aws_smithy_types::error::metadata::ProvideErrorMetadata::code(&error);
    if code == Some(POLICY_ABSENT_CODE) {
        return Some(None);
    }
    if code.is_none_or(|code| !TRANSPORT_REFUSALS.contains(&code)) {
        ::tracing::debug!(
            function = ?arn,
            error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
            "this destination's function did not report a resource-based policy, so the \
             destination is not judged against one"
        );
    }
    None
}

#[cfg(test)]
mod tests {
    use super::{function_arn, permits_invocation, INVOKE_FUNCTION_ACTION};

    /// The principal a client grants the invocation to, in the global spelling.
    const SERVICE_PRINCIPAL: &str = "logs.amazonaws.com";

    /// The Region the documents below are read in.
    const REGION: &str = "eu-west-2";

    /// The log group the subscription is configured on, and another one.
    const GROUP_ARN: &str = "arn:aws:logs:eu-west-2:000000000000:log-group:subscribed";
    const OTHER_GROUP_ARN: &str = "arn:aws:logs:eu-west-2:000000000000:log-group:elsewhere";

    /// The function the filter delivers to.
    const FUNCTION_ARN: &str = "arn:aws:lambda:eu-west-2:000000000000:function:fan-out";

    /// A function permission granting `principal` the invocation, scoped to `source_arn` when one
    /// is given — the document shape `lambda:AddPermission` writes.
    fn permission(principal: &str, source_arn: Option<&str>) -> String {
        let condition = source_arn.map_or_else(
            || "".to_string(),
            |arn| format!(r#","Condition":{{"ArnLike":{{"aws:SourceArn":"{arn}"}}}}"#),
        );
        format!(
            r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"Allow",
            "Principal":{{"Service":"{principal}"}},"Action":"{INVOKE_FUNCTION_ACTION}",
            "Resource":"{FUNCTION_ARN}"{condition}}}]}}"#
        )
    }

    /// RULE: the invocation is permitted only by a grant naming this service's principal and the
    /// invoke action, and a grant scoped to a source ARN covers only the log group it names.
    #[test]
    fn only_a_grant_covering_this_log_group_permits_the_invocation() {
        let rows: Vec<(&str, Option<String>, bool)> = vec![
            ("no policy at all", None, false),
            (
                "an unscoped grant to the service principal",
                Some(permission(SERVICE_PRINCIPAL, None)),
                true,
            ),
            (
                "a grant scoped to this log group",
                Some(permission(SERVICE_PRINCIPAL, Some(GROUP_ARN))),
                true,
            ),
            (
                "a grant scoped to this log group in the form that covers its streams",
                Some(permission(SERVICE_PRINCIPAL, Some(&format!("{GROUP_ARN}:*")))),
                true,
            ),
            (
                "a grant scoped to another log group",
                Some(permission(SERVICE_PRINCIPAL, Some(OTHER_GROUP_ARN))),
                false,
            ),
            (
                "a grant to the Regional spelling of the principal",
                Some(permission(
                    &format!("logs.{REGION}.amazonaws.com"),
                    Some(GROUP_ARN),
                )),
                true,
            ),
            (
                "a grant to another service",
                Some(permission("events.amazonaws.com", None)),
                false,
            ),
        ];
        for (case, policy, expected) in rows {
            let actual = permits_invocation(policy.as_deref(), REGION, FUNCTION_ARN, GROUP_ARN);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected permitted == {expected}, got {actual}"
            );
        }
    }

    /// RULE: only a FUNCTION destination is judged here; every other destination kind carries its
    /// own authorisation.
    #[test]
    fn only_a_function_destination_is_judged() {
        for (case, arn, judged) in [
            ("a function", FUNCTION_ARN, true),
            (
                "a function version",
                "arn:aws:lambda:eu-west-2:000000000000:function:fan-out:7",
                true,
            ),
            (
                "a Kinesis stream",
                "arn:aws:kinesis:eu-west-2:000000000000:stream/events",
                false,
            ),
            (
                "a logical destination",
                "arn:aws:logs:eu-west-2:000000000000:destination:central",
                false,
            ),
            ("a value that is no ARN", "fan-out", false),
        ] {
            let actual = function_arn(arn).is_some();
            assert_eq!(
                actual, judged,
                "case {case:?}: expected judged == {judged}, got {actual}"
            );
        }
    }
}
