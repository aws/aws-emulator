// SPDX-License-Identifier: CC0-1.0

//! The INTEGRATION TARGET an association points at: what kind of resource the
//! `IntegrationArn` names, whether the service that owns it really holds it, and what
//! associating it takes in that service.
//!
//! One concern, so one file: an association is a reference to another service's resource,
//! and everything that has to be asked of that service — does it hold the resource, and
//! does the instance now have the access the integration needs — is one hop, written once
//! here rather than in each operation that stores or reads an association.
//!
//! An integration whose target is an AWS Lambda function is the case this hop serves. Such
//! an integration exists so the INSTANCE can invoke that function, and a function is
//! invoked only where its own resource-based policy allows it ("grants a principal
//! permission to use a function", `AddPermission`), so associating one takes two things of
//! Lambda: the function has to BE there, and the service principal the instance invokes it
//! as has to be granted `lambda:InvokeFunction` on it, scoped to that instance. Both are
//! asked over the emulator's IN-PROCESS callback (`ctx.callback()`), so what is read and
//! written is what Lambda itself holds; nothing here opens a socket or leaves the host.
//!
//! A target of any other kind is stored as the reference it is: no other service states
//! what associating its resource takes, and a reference is not a claim that the resource
//! exists.

/// The service principal an Amazon Connect instance invokes an integration's function as.
///
/// Amazon Connect's own service principal, which is also the principal its service-linked
/// role is created for (`super::instance_service_role`).
pub const CONNECT_SERVICE_PRINCIPAL: &str = "connect.amazonaws.com";

/// The action an invocation of a function performs, as the IAM policy language names it.
pub const INVOKE_FUNCTION_ACTION: &str = "lambda:InvokeFunction";

/// The ARN vendor field and resource type of an AWS Lambda function.
const LAMBDA_VENDOR: &str = "lambda";
const FUNCTION_RESOURCE_TYPE: &str = "function";

/// The AWS Lambda function `integration_arn` names, or `None` when the integration's
/// target is a resource of some other kind.
///
/// Read off the ARN itself — its vendor field and the resource type it addresses
/// (`service_common::arn`) — rather than off the integration TYPE, because it is the
/// target's kind that decides what associating it takes: every integration whose target is
/// a function needs the instance to be able to invoke that function, whichever integration
/// type names it.
#[must_use]
pub fn integration_function(integration_arn: &str) -> Option<&str> {
    let arn = ::service_common::arn::parse(integration_arn)?;
    (arn.vendor == LAMBDA_VENDOR && arn.resource_type() == FUNCTION_RESOURCE_TYPE)
        .then_some(integration_arn)
}

/// What Lambda said about the function an integration names — the ONE reading of that
/// answer, so every operation that resolves an integration target agrees on which answers
/// establish what.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FunctionAnswer {
    /// Lambda holds the function.
    Held,
    /// Lambda STATED it holds no function the ARN names.
    NoSuchFunction,
    /// Lambda did not state the function's facts at all: an emulator with no Lambda
    /// mounted beside this service, a transport that carried nothing, any failure to ask.
    /// Evidence of nothing, so nothing may be refused for it.
    Unanswered,
}

/// Ask Lambda whether it holds the function `function_arn`, in `region`.
///
/// `GetFunction` states one fault about a function it cannot show — "the resource
/// specified in the request does not exist"
/// ([`::aws_sdk_lambda::operation::get_function::GetFunctionError::ResourceNotFoundException`])
/// — and under it the account holds no such function. Any other failure says nothing about
/// the function ([`FunctionAnswer::Unanswered`]).
///
/// Reading a function changes nothing a later caller of Lambda sees.
///
/// `region` is the Region the REQUEST is signed for, which is where the instance being
/// associated lives: no text of a request chooses which endpoint is asked.
pub async fn function_answer(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    function_arn: &str,
) -> FunctionAnswer {
    let answered = lambda_client(ctx, region)
        .get_function()
        .function_name(function_arn)
        .send()
        .await;
    match answered {
        Ok(_) => FunctionAnswer::Held,
        Err(failure) => {
            let stated = matches!(
                failure.as_service_error(),
                Some(
                    ::aws_sdk_lambda::operation::get_function::GetFunctionError::
                        ResourceNotFoundException(_)
                )
            );
            if stated {
                FunctionAnswer::NoSuchFunction
            } else {
                unanswered(function_arn, &failure);
                FunctionAnswer::Unanswered
            }
        }
    }
}

/// Grant the instance `instance_arn` the invocation of the function `function_arn`, under
/// the statement id `statement_id`, reporting whether Lambda wrote the grant.
///
/// One statement, exactly as a client would write it with `AddPermission`: the service
/// principal Amazon Connect invokes as, the invoke action, and the instance's own ARN as
/// the `SourceArn` the grant is scoped to — so the function is left invocable by THAT
/// instance and by nothing else this grant admits. `statement_id` is the association's own
/// identifier, which is what lets the grant be told from any other statement the function
/// carries.
pub async fn grant_instance_invoke(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    function_arn: &str,
    statement_id: &str,
    instance_arn: &str,
) -> bool {
    let answered = lambda_client(ctx, region)
        .add_permission()
        .function_name(function_arn)
        .statement_id(statement_id)
        .action(INVOKE_FUNCTION_ACTION)
        .principal(CONNECT_SERVICE_PRINCIPAL)
        .source_arn(instance_arn)
        .send()
        .await;
    match answered {
        Ok(_) => true,
        Err(failure) => {
            unanswered(function_arn, &failure);
            false
        }
    }
}

/// Take back the grant `statement_id` on the function `function_arn`, reporting whether
/// Lambda removed it.
///
/// `RemovePermission` "revokes function-use permission from an Amazon Web Services service
/// or another Amazon Web Services account", which is what a grant written for an
/// association that was then NOT stored needs: the statement names an association nothing
/// holds, so it is taken back rather than left on the function.
pub async fn revoke_instance_invoke(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    function_arn: &str,
    statement_id: &str,
) -> bool {
    let answered = lambda_client(ctx, region)
        .remove_permission()
        .function_name(function_arn)
        .statement_id(statement_id)
        .send()
        .await;
    match answered {
        Ok(_) => true,
        Err(failure) => {
            unanswered(function_arn, &failure);
            false
        }
    }
}

/// Note that Lambda did not state the function's facts, so the integration's target is
/// judged against nothing.
///
/// Worth a line to an operator, because a deployment with no Lambda to ask is one where
/// this half of the association does not run. The caller's own value is rendered in its
/// escaped `Debug` form, so an ARN carrying a newline cannot forge a log line.
fn unanswered<E, R>(
    function_arn: &str,
    failure: &::aws_smithy_runtime_api::client::result::SdkError<E, R>,
) where
    E: ::std::error::Error + 'static,
    R: ::std::fmt::Debug,
{
    ::tracing::warn!(
        named = ?function_arn,
        error = %::aws_smithy_types::error::display::DisplayErrorContext(failure),
        "the integration's Lambda function could not be read, so it is not judged"
    );
}

/// Lambda's client for `region`, over the in-process callback into this same emulator.
///
/// The transport is in-process, so this disables what a network client needs and this one
/// cannot have: retries, timeouts, stalled-stream protection and identity caching each need
/// a sleep implementation the callback transport is not given, and there is nothing to retry
/// behind it anyway. The only caller-supplied value that reaches the client's authority is
/// the signing Region, which is a Region LABEL by the time it gets here
/// (`super::request_region`), so no text of a request can steer the address.
fn lambda_client(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> ::aws_sdk_lambda::Client {
    let config = ::aws_sdk_lambda::Config::builder()
        .behavior_version(::aws_sdk_lambda::config::BehaviorVersion::latest())
        .http_client(ctx.callback().http_client())
        .credentials_provider(ctx.callback().aws_credentials_provider())
        .region(::aws_sdk_lambda::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://lambda.{region}.amazonaws.com"))
        .retry_config(::aws_sdk_lambda::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_lambda::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_lambda::config::IdentityCache::no_cache())
        .stalled_stream_protection(
            ::aws_sdk_lambda::config::StalledStreamProtectionConfig::disabled(),
        )
        .build();
    ::aws_sdk_lambda::Client::from_conf(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Which integration ARNs name a function: the vendor field and the resource type
    /// decide it, over both ARN resource forms, and no other vendor's resource does.
    #[test]
    fn a_function_target_is_read_off_the_integration_arn() {
        let cases: Vec<(&str, &str, bool)> = vec![
            (
                "a function",
                "arn:aws:lambda:us-east-1:000000000000:function:message-processor",
                true,
            ),
            (
                "a function in another Region and account",
                "arn:aws:lambda:eu-west-2:123456789012:function:scanner",
                true,
            ),
            (
                "a published version of a function",
                "arn:aws:lambda:us-east-1:000000000000:function:scanner:3",
                true,
            ),
            (
                "a function of another partition",
                "arn:aws-us-gov:lambda:us-gov-west-1:000000000000:function:scanner",
                true,
            ),
            (
                "a Lambda layer, not a function",
                "arn:aws:lambda:us-east-1:000000000000:layer:shared:1",
                false,
            ),
            (
                "a Voice ID domain",
                "arn:aws:voiceid:us-east-1:000000000000:domain/EXAMPLE123",
                false,
            ),
            (
                "an Amazon Connect Cases domain",
                "arn:aws:cases:us-east-1:000000000000:domain/EXAMPLE123",
                false,
            ),
            (
                "an Amazon Pinpoint application",
                "arn:aws:mobiletargeting:us-east-1:000000000000:apps/EXAMPLE123",
                false,
            ),
            ("not an ARN at all", "message-processor", false),
            ("an empty value", "", false),
        ];
        for (case, integration_arn, names_a_function) in cases {
            let actual = integration_function(integration_arn).is_some();
            assert_eq!(
                actual, names_a_function,
                "case {case:?}: expected names_a_function={names_a_function}, got {actual}"
            );
            if names_a_function {
                assert_eq!(
                    integration_function(integration_arn),
                    Some(integration_arn),
                    "case {case:?}: the target is the ARN itself"
                );
            }
        }
    }
}
