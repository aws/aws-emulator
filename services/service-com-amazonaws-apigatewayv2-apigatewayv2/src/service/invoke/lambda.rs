// SPDX-License-Identifier: CC0-1.0

//! The ONE in-process hop this data plane makes to AWS Lambda: how the function an
//! `AWS_PROXY` route or a `REQUEST` authorizer names is RUN, and how what came back is
//! told from the call never reaching a function at all.
//!
//! An api's two Lambda-backed steps — the integration of an `AWS_PROXY` route ("for
//! integrating the route or method request with the Lambda function-invoking action with
//! the client request passed through as-is", `Integration$IntegrationType`) and the
//! decision of a `REQUEST` authorizer ("For REQUEST authorizers, this must be a
//! well-formed Lambda function URI", `Authorizer$AuthorizerUri`) — are both one synchronous
//! invocation of a function this emulator holds. The invocation is made THROUGH THE
//! EMULATOR this service runs within, over the in-process transport the emulator hands this
//! crate (`OperationContext::callback` for a request served through the front door, the
//! clone of it a WEBSOCKET api's socket was opened with for a frame served on that socket):
//! nothing is invented and no connection is opened, so what the route answers is what the
//! function itself returned for the event it was given.
//!
//! Both halves of this service reach it the same way, which is why the transport is taken as
//! the in-process handle itself rather than as a request's context: a WebSocket api's data
//! plane is carried on a socket the core owns, where there is no request and no
//! `OperationContext` at all.
//!
//! Three outcomes are told apart here, because the api answers each differently:
//!
//! * the function RAN — its payload, plus the `FunctionError` Lambda reports for a handler
//!   that raised ("X-Amz-Function-Error … indicates whether an error occurred while
//!   executing the Lambda function"), which is what the gateway reads to decide that the
//!   integration produced no usable response;
//! * the function could not be invoked — Lambda itself refused the invocation (no such
//!   function, a function that cannot be invoked yet, an invocation this emulator cannot
//!   perform), which is an integration that produced no response;
//! * the call reached NO Lambda at all, because this emulator mounts none
//!   ([`::service_common::in_process_refusal`]). Mapping that as a backend answer would
//!   report a status and a body for a call nothing made, so it is reported as the same
//!   "produced no response" class with its own reason logged.
//!
//! The DEADLINE is the integration's own: `Integration$TimeoutInMillis` is "custom timeout
//! between 50 and 29,000 milliseconds for WebSocket APIs and between 50 and 30,000
//! milliseconds for HTTP APIs", and a run that overruns it is an integration that RAN and
//! did not answer in time — a different class from one that answered badly, which is why
//! the caller is told which of the two happened ([`Unreached::TimedOut`]).
//!
//! What never travels is the CALLER'S own credential: the invocation is signed with the
//! credentials the in-process callback hands this handler, and neither they nor any
//! `Authorization` header the client presented are reported or logged.
//!
//! WHO the invocation is made as is therefore not the caller either: an api invokes a route's
//! function as API Gateway's own service principal, so what admits the invocation is the
//! FUNCTION's resource-based policy ([`invocation_admitted`]) — read from Lambda itself, so a
//! permission added or removed there takes effect on the next request.
//!
//! The hop is made over the CALLBACK HANDLE rather than over the whole request context
//! (`OperationContext::callback`), because the two data planes reach a function from
//! different places: an HTTP invoke is an operation of this service and holds a context,
//! while a WEBSOCKET api's socket is a core-owned listener whose handler holds none. The
//! handle is the one piece either needs, so it is what this module takes.

use std::time::Duration;

use ::aws_smithy_types::error::metadata::ProvideErrorMetadata;
use smithy_server_runtime::EmbeddedEmulator;

/// The `aws.api#service$arnNamespace` a function ARN carries, which is what
/// [`::service_common::arn::region_of`] matches an integration's or an authorizer's URI
/// against.
const LAMBDA_ARN_VENDOR: &str = "lambda";

/// The path form an `AuthorizerUri` names a function with: "arn:aws:apigateway:{region}:
/// lambda:path/2015-03-31/functions/{function-arn}/invocations" — the function ARN is
/// EMBEDDED in that path, so the ARN is read out of the value wherever in it it sits.
/// A function ARN carries no `/`, so the wrapper's own path segments end it.
const ARN_PATH_SEPARATOR: char = '/';

/// The service principal an API Gateway api invokes a route's function as, as a resource
/// policy names one: "for Amazon Web Services services, the principal is a domain-style
/// identifier that the service defines" (`AddPermission$Principal`). It is API Gateway's own
/// service identifier, the one a grant for an api's integration names.
const GATEWAY_SERVICE_PRINCIPAL: &str = "apigateway.amazonaws.com";

/// The IAM action an invocation of a function performs, which is not the operation's own
/// name: `Invoke` "requires permission for the `lambda:InvokeFunction` action".
const INVOKE_FUNCTION_ACTION: &str = "lambda:InvokeFunction";

/// The condition key a grant narrowed to ONE invoking resource names the source by, which for
/// an api's integration is the ROUTE the request was served by.
const SOURCE_ARN_KEY: &str = "aws:SourceArn";

/// The condition key a grant narrowed to the account that owns the invoking api names it by.
const SOURCE_ACCOUNT_KEY: &str = "aws:SourceAccount";

/// The function one hop addresses: the ARN to invoke, and the Region whose Lambda endpoint
/// the call is made to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionTarget {
    /// The function ARN, qualified exactly as the api's record names it — a version or
    /// alias suffix included, since `Invoke` resolves the qualifier the ARN carries.
    pub arn: String,
    /// The Region of that ARN, which is the endpoint the invocation is addressed to.
    pub region: String,
}

/// What the function RETURNED: the payload bytes, and the error signal Lambda sets for a
/// handler that raised rather than returned.
#[derive(Debug, Clone)]
pub struct Ran {
    /// The function's own response bytes — `Invoke`'s `Payload`, which for a handler that
    /// raised is Lambda's error document.
    pub payload: Vec<u8>,
    /// `FunctionError`: "If present, indicates that an error occurred during function
    /// execution. Details about the error are included in the response payload."
    pub function_error: Option<String>,
}

impl Ran {
    /// Whether the run ENDED IN AN ERROR the function did not handle, which is what makes
    /// its payload an error document rather than a response to map.
    pub fn failed(&self) -> bool {
        self.function_error
            .as_deref()
            .is_some_and(|signal| !signal.trim().is_empty())
    }
}

/// Why no function response exists to map.
#[derive(Debug, Clone)]
pub enum Unreached {
    /// Lambda ANSWERED, refusing the invocation (no such function, a function that cannot
    /// be invoked yet, an invocation it cannot perform). The reason is Lambda's own.
    Refused(String),
    /// Nothing in this emulator serves Lambda, so the invocation reached no function.
    NoLambda,
    /// The function was invoked and had not answered when the integration's timeout
    /// elapsed.
    TimedOut(Duration),
}

impl Unreached {
    /// One sentence for the emulator's own log, never for the caller: the caller is
    /// answered the gateway's own status for an integration that produced no response.
    pub fn reason(&self) -> String {
        match self {
            Unreached::Refused(detail) => format!("Lambda refused the invocation: {detail}"),
            Unreached::NoLambda => {
                "the invocation reached no AWS Lambda in this emulator, so there is no \
                 function response to map"
                    .to_string()
            }
            Unreached::TimedOut(after) => format!(
                "the function had not answered {} ms after it was invoked",
                after.as_millis()
            ),
        }
    }
}

/// The function `uri` names, or `None` when it names none.
///
/// Both documented spellings resolve, because both reach this data plane: an HTTP api's
/// `AWS_PROXY` integration carries the function ARN itself, and an authorizer carries it
/// inside the `arn:aws:apigateway:…:lambda:path/…/functions/{arn}/invocations` wrapper. The
/// ARN is found by its OWN fields rather than by stripping a wrapper — the first `arn:`
/// whose service field is `lambda` — so a wrapper spelled any other way resolves the same
/// function.
///
/// The Region is read through [`::service_common::arn::region_of`], the tree's one owner of
/// that rule, which reports the field only when it is a Region LABEL: the value ends up in
/// the authority of the endpoint the invocation is addressed to, so a field carrying
/// anything else names no endpoint and the URI addresses nothing.
pub fn function_target(uri: &str) -> Option<FunctionTarget> {
    let candidate = uri
        .match_indices("arn:")
        .map(|(start, _)| &uri[start..])
        .find_map(|rest| {
            let arn = rest.split(ARN_PATH_SEPARATOR).next()?;
            (::service_common::arn::vendor(arn) == Some(LAMBDA_ARN_VENDOR)).then_some(arn)
        })?;
    let region = ::service_common::arn::region_of(candidate, LAMBDA_ARN_VENDOR)?;
    Some(FunctionTarget {
        arn: candidate.to_string(),
        region: region.to_string(),
    })
}

/// Run `target` with `payload` as its event, giving it `timeout` to answer.
///
/// The invocation is synchronous — "RequestResponse (default) – Invoke the function
/// synchronously. Keep the connection open until the function returns a response or times
/// out" — because the route's answer IS the function's answer, so there is nothing to serve
/// until the run ends.
pub async fn invoke_function(
    callback: &EmbeddedEmulator,
    target: &FunctionTarget,
    payload: Vec<u8>,
    timeout: Duration,
) -> Result<Ran, Unreached> {
    let call = client(callback, &target.region)
        .invoke()
        .function_name(&target.arn)
        .payload(::aws_smithy_types::Blob::new(payload))
        .send();
    let answered = match ::tokio::time::timeout(timeout, call).await {
        Ok(answered) => answered,
        Err(_elapsed) => return Err(Unreached::TimedOut(timeout)),
    };
    let output = match answered {
        Ok(output) => output,
        Err(error) => {
            // The bridge keeps the refused call's BODY, which is where the front door's own
            // refusal can be told from a refusal Lambda itself answered.
            let raw = error
                .raw_response()
                .and_then(|response| response.body().bytes())
                .map(<[u8]>::to_vec)
                .unwrap_or_default();
            if ::service_common::in_process_refusal::is_transport_refusal(&raw) {
                return Err(Unreached::NoLambda);
            }
            return Err(Unreached::Refused(
                error
                    .message()
                    .map(str::to_string)
                    .unwrap_or_else(|| error.code().unwrap_or("Unknown").to_string()),
            ));
        }
    };
    Ok(Ran {
        payload: output
            .payload
            .map(::aws_smithy_types::Blob::into_inner)
            .unwrap_or_default(),
        function_error: output.function_error,
    })
}

/// Whether the resource policy of `target`'s function admits the invocation THE GATEWAY makes
/// for the route whose `execute-api` ARN is `source_arn`.
///
/// A gateway invokes a route's function as its own service principal
/// ([`GATEWAY_SERVICE_PRINCIPAL`]) rather than as the client that called the route, so the
/// grant that admits the invocation is the FUNCTION's resource-based policy — the document
/// `AddPermission` writes and `GetPolicy` reports. It is read from Lambda itself rather than
/// held a second time here, and the decision is
/// [`::service_common::iam_authorization`], the tree's one evaluation of policy documents,
/// taken on [`INVOKE_FUNCTION_ACTION`] over that function with the ROUTE's own ARN as the
/// `aws:SourceArn` such a grant is narrowed by — which is how a permission added for one api
/// (or one route of it) admits the invocation for that one and for no other.
///
/// `true` also when nothing about the function could be SHOWN, which refuses nothing: a
/// function Lambda does not answer about at all, and an emulator with no Lambda mounted, are
/// not evidence of a missing grant — the invocation below is then what decides the request. A
/// function whose policy Lambda REPORTS as absent is an answer rather than a silence
/// ([`reports_no_policy`]): it grants nothing, so the invocation is not made.
pub async fn invocation_admitted(
    callback: &EmbeddedEmulator,
    target: &FunctionTarget,
    source_arn: &str,
) -> bool {
    let client = client(callback, &target.region);
    // The RESOURCE a grant is taken on is the function's own ARN as Lambda reports it, so it
    // is resolved there rather than assumed to be what the integration carried (a version or
    // alias qualifier included). A function Lambda answers nothing about is one this judges
    // nothing about.
    let resolved = match client
        .get_function_configuration()
        .function_name(&target.arn)
        .send()
        .await
    {
        Ok(reported) => reported.function_arn.unwrap_or_else(|| target.arn.clone()),
        Err(error) => {
            tracing::warn!(
                function = %target.arn,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "this route's function was not judged: nothing answered for its configuration"
            );
            return true;
        }
    };
    let document = match client.get_policy().function_name(&resolved).send().await {
        Ok(reported) => reported.policy.unwrap_or_default(),
        Err(error) => {
            if !reports_no_policy(&error) {
                tracing::warn!(
                    function = %resolved,
                    error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                    "this route's function was not judged: nothing answered for its resource \
                     policy"
                );
                return true;
            }
            // A function Lambda reports no resource policy for carries none, which grants
            // nothing.
            String::new()
        }
    };
    let principals = [GATEWAY_SERVICE_PRINCIPAL.to_string()];
    let context = [
        (SOURCE_ARN_KEY.to_string(), source_arn.to_string()),
        (
            SOURCE_ACCOUNT_KEY.to_string(),
            ::engine::emulator_account_id().to_string(),
        ),
    ];
    let request =
        ::service_common::iam_authorization::Request::new(
            INVOKE_FUNCTION_ACTION,
            &resolved,
            &principals,
        )
        .with_context(&context)
        .with_published_keys(&[SOURCE_ARN_KEY, SOURCE_ACCOUNT_KEY]);
    ::service_common::iam_authorization::resource_decision(&document, &request)
        == ::service_common::iam_authorization::Decision::Allowed
}

/// Whether `error` is LAMBDA REPORTING THAT THE FUNCTION HAS NO RESOURCE POLICY, which is an
/// answer about the function — `GetPolicy` models `ResourceNotFoundException` for "the
/// resource specified in the request does not exist", and a function carrying no policy grants
/// nothing.
///
/// Every other failure is not that answer and grounds no refusal: a failure the emulator
/// itself produced (no service of it serves the endpoint, or more than one claims it), and any
/// other error Lambda answered, leave the function unjudged and the invocation to decide.
fn reports_no_policy(
    error: &::aws_smithy_runtime_api::client::result::SdkError<
        ::aws_sdk_lambda::operation::get_policy::GetPolicyError,
        ::aws_smithy_runtime_api::client::orchestrator::HttpResponse,
    >,
) -> bool {
    let ::aws_smithy_runtime_api::client::result::SdkError::ServiceError(service) = error else {
        return false;
    };
    service.err().is_resource_not_found_exception()
}

/// The Lambda client one hop is made with.
///
/// The transport is the emulator's own, in process, so the invocation is served by the
/// Lambda this emulator mounts and no connection is opened; the credentials are the ones
/// the callback hands over, which sign this call and are never reported or logged. The
/// Region only selects the endpoint the client is configured for — the transport serves it
/// in process whatever that label says.
fn client(callback: &EmbeddedEmulator, region: &str) -> ::aws_sdk_lambda::Client {
    let config = ::aws_sdk_lambda::Config::builder()
        .behavior_version(::aws_sdk_lambda::config::BehaviorVersion::latest())
        .http_client(callback.http_client())
        .credentials_provider(callback.aws_credentials_provider())
        .region(::aws_sdk_lambda::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://lambda.{region}.amazonaws.com"))
        // The transport is in-process: there is nothing to retry, time out or watch for a
        // stalled stream, and each of those would need a sleep implementation this client
        // is not given. The integration's own `TimeoutInMillis` is the deadline, applied
        // by the caller above.
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

    /// One row: what a failing assertion calls it, the URI read, and the `(ARN, Region)`
    /// the function it names resolves to.
    type Row = (&'static str, &'static str, Option<(&'static str, &'static str)>);

    #[test]
    fn a_function_is_read_out_of_every_uri_that_names_one() {
        // Rows differ along the SPELLING of the URI: the bare ARN an HTTP api's
        // integration carries, the same qualified by a version and by an alias, the
        // authorizer path wrapper the model documents, a wrapper naming another Region
        // than the function, and the values that name no function at all.
        let cases: [Row; 9] = [
            (
                "an HTTP api integration's bare function ARN",
                "arn:aws:lambda:us-east-1:000000000000:function:orders",
                Some(("arn:aws:lambda:us-east-1:000000000000:function:orders", "us-east-1")),
            ),
            (
                "a function ARN qualified by a version",
                "arn:aws:lambda:eu-west-2:000000000000:function:orders:3",
                Some(("arn:aws:lambda:eu-west-2:000000000000:function:orders:3", "eu-west-2")),
            ),
            (
                "the authorizer URI the model documents",
                "arn:aws:apigateway:us-west-2:lambda:path/2015-03-31/functions/\
                 arn:aws:lambda:us-west-2:000000000000:function:gate/invocations",
                Some(("arn:aws:lambda:us-west-2:000000000000:function:gate", "us-west-2")),
            ),
            (
                "a wrapper whose own Region differs from the function's",
                "arn:aws:apigateway:us-west-2:lambda:path/2015-03-31/functions/\
                 arn:aws:lambda:ap-south-1:000000000000:function:gate/invocations",
                Some(("arn:aws:lambda:ap-south-1:000000000000:function:gate", "ap-south-1")),
            ),
            (
                "a value in another partition",
                "arn:aws-us-gov:lambda:us-gov-west-1:000000000000:function:orders",
                Some((
                    "arn:aws-us-gov:lambda:us-gov-west-1:000000000000:function:orders",
                    "us-gov-west-1",
                )),
            ),
            ("an HTTP_PROXY upstream URL", "https://example.test/orders", None),
            (
                "an ARN of another vendor",
                "arn:aws:states:us-east-1:000000000000:stateMachine:orders",
                None,
            ),
            (
                "a function ARN with no Region field",
                "arn:aws:lambda::000000000000:function:orders",
                None,
            ),
            (
                "a Region field that is not a Region label",
                "arn:aws:lambda:us-east-1?x=1:000000000000:function:orders",
                None,
            ),
        ];
        for (case, uri, expected) in cases {
            let actual = function_target(uri);
            let expected = expected.map(|(arn, region)| FunctionTarget {
                arn: arn.to_string(),
                region: region.to_string(),
            });
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    #[test]
    fn a_run_is_failed_only_when_lambda_signalled_an_error() {
        let cases: [(&str, Option<&str>, bool); 4] = [
            ("a handler that returned", None, false),
            ("a handler that raised", Some("Unhandled"), true),
            ("a handled error Lambda reports", Some("Handled"), true),
            ("an empty signal, which signals nothing", Some(""), false),
        ];
        for (case, signal, expected) in cases {
            let ran = Ran {
                payload: Vec::new(),
                function_error: signal.map(str::to_string),
            };
            assert_eq!(
                ran.failed(),
                expected,
                "case {case:?}: expected failed {expected}, got {}",
                ran.failed()
            );
        }
    }
}
