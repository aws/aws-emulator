// SPDX-License-Identifier: CC0-1.0

//! Rotating a secret: whether this service may invoke the rotation function at all, and
//! the four steps it drives that function through.
//!
//! `RotateSecret` "configures and starts the asynchronous process of rotating the secret"
//! (this service's Smithy model), and the process itself is the rotation FUNCTION's: "Secrets
//! Manager invokes the Lambda rotation function" once per step, and the function does the work
//! of each — `createSecret` stages a new value under `AWSPENDING`, `setSecret` changes the
//! credential in the service that owns it, `testSecret` proves the new value works, and
//! `finishSecret` moves `AWSCURRENT` onto the staged version. Each invocation carries the
//! event AWS documents for it: the secret, the step, and the `ClientRequestToken` naming the
//! version the rotation creates.
//!
//! Two things follow, and they are the whole of this module.
//!
//! * **The invocation needs permission.** "You also need `lambda:InvokeFunction` permissions
//!   on the rotation function" (`RotateSecret`, this service's Smithy model), and the call is
//!   made by this service as its own service principal — so what has to grant it is the
//!   FUNCTION's resource-based policy, read through Lambda's own `GetPolicy` and decided by
//!   the shared evaluation ([`::service_common::iam_authorization`]) for the principal
//!   [`SERVICE_PRINCIPAL`], under the condition keys such a grant is scoped by
//!   (`aws:SourceArn` naming the secret being rotated, `aws:SourceAccount` its account). A
//!   function whose policy does not admit this service cannot be invoked, so the rotation
//!   cannot be started and the call is refused rather than accepted and silently abandoned.
//! * **The steps run off the response path.** The call ANSWERS while the rotation is in
//!   flight — it reports the id of the version the rotation will create, which does not exist
//!   yet — so the four invocations are handed to a task of its own ([`drive`]), exactly as the
//!   work behind any other asynchronous submission in this tree is. A rotation is in progress
//!   while it runs, which is what makes a second `RotateSecret` refused, and the secret is
//!   stamped as rotated only once the last step has returned.
//!
//! Nothing here invents a rotation: every version a rotation produces is one the rotation
//! function itself wrote through this service's own operations. A rotation whose function did
//! not complete a step is driven again from the first step — "if rotation fails, Secrets
//! Manager retries the entire rotation process multiple times"
//! (`DescribeSecretResponse$NextRotationDate`, this service's Smithy model) — and one whose
//! function keeps raising is left in the state the real service leaves a failed rotation in:
//! the current value still readable, `AWSPENDING` still staked, and `LastRotatedDate` unset.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use ::service_common::iam_authorization::{self as iam, Decision};
use ::smithy_server_runtime::EmbeddedEmulator;

/// The service principal Secrets Manager invokes a rotation function as: "Ensure that the
/// function policy grants access to the principal `secretsmanager.amazonaws.com`" (Secrets
/// Manager User Guide, *Permissions for rotation*).
pub const SERVICE_PRINCIPAL: &str = "secretsmanager.amazonaws.com";

/// The IAM action an invocation of the rotation function is authorized under: "You also need
/// lambda:InvokeFunction permissions on the rotation function" (`RotateSecret`, this
/// service's Smithy model).
const INVOKE_FUNCTION_ACTION: &str = "lambda:InvokeFunction";

/// What this service answers when the rotation function's policy does not let it invoke the
/// function: AWS's own sentence for that condition (Secrets Manager User Guide,
/// *Troubleshoot rotation*), whose only variable part is the principal it names.
#[must_use]
pub fn invoke_not_permitted_message() -> String {
    format!(
        "Secrets Manager cannot invoke the specified Lambda function. Ensure that the function \
         policy grants access to the principal {SERVICE_PRINCIPAL}."
    )
}

/// The steps a rotation drives the rotation function through, in the order AWS invokes them:
/// "Secrets Manager calls the Lambda rotation function four times, each time with a different
/// value for the Step parameter" (Secrets Manager User Guide, *How rotation works*).
pub const ROTATION_STEPS: [&str; 4] = ["createSecret", "setSecret", "testSecret", "finishSecret"];

/// The member of the rotation event naming the secret being rotated.
const EVENT_SECRET_ID: &str = "SecretId";

/// The member naming the version the rotation creates, which is the `ClientRequestToken`
/// `RotateSecret` staked `AWSPENDING` on.
const EVENT_CLIENT_REQUEST_TOKEN: &str = "ClientRequestToken";

/// The member naming which step of the rotation this invocation is.
const EVENT_STEP: &str = "Step";

/// How long after the call that started it a rotation begins invoking the rotation function.
///
/// `RotateSecret` "configures and STARTS the asynchronous process of rotating the secret"
/// (this service's Smithy model), so the first step is the next thing that happens: the
/// rotation is driven straight through its four steps, and what a client waits for is the
/// FUNCTION's own work rather than a span this service adds ahead of it. That matters to a
/// client that watches a rotation by one of its side effects — a backup copy an early step
/// writes, a credential a later step changes — because every span added here is added
/// between that side effect and the rotation being finished.
///
/// The span is therefore the smallest one that still leaves the rotation IN FLIGHT for a
/// caller acting immediately: the call answers with `AWSPENDING` staked on a version that
/// does not exist yet, and this keeps that state readable — and a second `RotateSecret`
/// refused for a rotation still in progress — for a caller whose next request follows at
/// once. The band this tree gives a transition it owes a caller is not this sub-span's to
/// meet: it is the ROTATION that is the transition, and it stays in flight for the whole of
/// its four invocations, which is where that band is spent. This is only the lead-in, so it
/// is this emulator's own figure (AWS states none), orders of magnitude below one invocation
/// of a rotation function.
const ROTATION_START_DELAY: Duration = Duration::from_millis(250);

/// How long one step of a rotation may hold the task that drives it.
///
/// The invocation itself is bounded by Lambda — the function's own configured timeout, and
/// the concurrency the function has room for — so this is the bound on the WAIT rather than
/// on the run: a step whose answer does not come inside it is given up on, and the rotation
/// ends unfinished rather than keeping a task of its own for as long as the function likes.
/// It is this emulator's own figure, chosen well above any rotation function's run and far
/// below a span a developer would call stuck.
const ROTATION_STEP_BUDGET: Duration = Duration::from_secs(30);

/// How many times a rotation whose function did not complete every step is driven, the first
/// attempt included.
///
/// "If rotation fails, Secrets Manager retries the entire rotation process multiple times"
/// (`DescribeSecretResponse$NextRotationDate`, this service's Smithy model) — the ENTIRE
/// process, so a retry starts again at the first step, which is what makes a rotation whose
/// function could not be reached at all finish once it can be.
///
/// AWS publishes neither the number of attempts nor their spacing, so both are this
/// emulator's own: enough for an obstacle in front of the function that clears by itself — a
/// function still inside the provisioning window its creation opened, a concurrency limit
/// momentarily reached — and few enough that a rotation whose function always raises stops
/// being driven rather than being retried for as long as this process runs.
const ROTATION_ATTEMPTS: u32 = 4;

/// How long after an attempt that did not finish the rotation the next one begins. See
/// [`ROTATION_ATTEMPTS`]: it is this emulator's own span, above the windows a service of this
/// emulator opens on a resource it is still provisioning.
const ROTATION_RETRY_DELAY: Duration = Duration::from_secs(1);

/// The resource type a Lambda function's ARN names:
/// `arn:aws:lambda:<region>:<account>:function:<name>`.
const FUNCTION_RESOURCE_TYPE: &str = "function";

/// Lambda's ARN namespace, the vendor field of that ARN.
const LAMBDA_ARN_NAMESPACE: &str = "lambda";

/// Whether `arn` names a Lambda function, which is what a rotation function has to be:
/// a rotation drives it by INVOKING it, so a value that is not the ARN of a function names
/// nothing this service can invoke.
///
/// The account and Region fields are not compared: a `RotationLambdaARN` names the function
/// as the caller spelled it, and whether that function can be invoked is the FUNCTION's own
/// answer ([`invoke_permission_refusal`]) rather than a reading of its ARN.
#[must_use]
pub fn names_a_function(arn: &str) -> bool {
    ::service_common::arn::parse_of(arn, LAMBDA_ARN_NAMESPACE).is_some_and(|parsed| {
        parsed.resource_type() == FUNCTION_RESOURCE_TYPE
            && parsed
                .resource
                .strip_prefix(FUNCTION_RESOURCE_TYPE)
                .is_some_and(|tail| tail.len() > 1)
    })
}

/// What this service answers for a `RotationLambdaARN` that names no Lambda function.
///
/// `RotationLambdaARNType` states no pattern and a length of 0..2048 (this service's Smithy
/// model), so which values name a rotation function is the handler's judgement — and a value
/// that is not a Lambda function's ARN leaves the secret with no rotation function
/// associated with it, which is the documented `InvalidRequestException` cause "You tried to
/// enable rotation on a secret that doesn't already have a Lambda function ARN configured and
/// you didn't include such an ARN as a parameter in this call." So the sentence is the one
/// this service words that cause with, [`crate::state::NO_ROTATION_FUNCTION_MESSAGE`], rather
/// than one of this emulator's own: fixed text, interpolating no part of the request.
#[must_use]
pub fn not_a_function_message() -> &'static str {
    super::NO_ROTATION_FUNCTION_MESSAGE
}

/// Why this service cannot invoke the rotation function `lambda_arn` for the secret
/// `secret_arn`, or `None` when it can.
///
/// The decision is the FUNCTION's own resource-based policy, read through Lambda's `GetPolicy`
/// — the same document `AddPermission` writes and that operation answers with — and evaluated
/// for the service principal this invocation is made as. A policy that allows
/// `lambda:InvokeFunction` to that principal for this secret permits the rotation; one that
/// does not, and a function that holds no policy at all, refuses it.
///
/// `None`, so nothing is refused on evidence that is not there, when the request reached no
/// Lambda at all: an emulator assembled without it, where the TRANSPORT refused the call and
/// nothing was said about the function.
pub async fn invoke_permission_refusal(
    callback: &EmbeddedEmulator,
    region: &str,
    lambda_arn: &str,
    secret_arn: &str,
) -> Option<String> {
    let answered = lambda_client(callback, region)
        .get_policy()
        .function_name(lambda_arn)
        .send()
        .await;
    let policy = match answered {
        Ok(answer) => answer.policy().unwrap_or_default().to_string(),
        Err(error) => {
            if reached_no_lambda(&error) {
                ::tracing::debug!(
                    function = %lambda_arn,
                    "no Lambda answered for this rotation function, so its policy is not read"
                );
                return None;
            }
            // Lambda answered: the function holds no policy, or there is no such function.
            // Either way nothing grants this service the invocation.
            ::tracing::debug!(
                function = %lambda_arn,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "the rotation function's policy could not be read, so the rotation is refused"
            );
            return Some(invoke_not_permitted_message());
        }
    };
    (!permits_invocation(&policy, lambda_arn, secret_arn)).then(invoke_not_permitted_message)
}

/// Whether `policy` lets [`SERVICE_PRINCIPAL`] invoke the function `lambda_arn` for the secret
/// `secret_arn`.
///
/// The condition keys a grant of this kind is scoped by travel with the request: a statement
/// written by `AddPermission` with a `SourceArn` names the ONE secret whose rotation may use
/// the function, and one with a `SourceAccount` the account that secret belongs to, so a grant
/// made for another secret does not admit this rotation.
fn permits_invocation(policy: &str, lambda_arn: &str, secret_arn: &str) -> bool {
    let principals = [SERVICE_PRINCIPAL.to_string()];
    let account = ::service_common::arn::parse(secret_arn)
        .map(|arn| arn.account.to_string())
        .unwrap_or_else(|| ::engine::emulator_account_id().to_string());
    let context = [
        (SOURCE_ARN_KEY.to_string(), secret_arn.to_string()),
        (SOURCE_ACCOUNT_KEY.to_string(), account),
    ];
    let request = iam::Request::new(INVOKE_FUNCTION_ACTION, lambda_arn, &principals)
        .with_context(&context)
        .with_published_keys(&[SOURCE_ARN_KEY, SOURCE_ACCOUNT_KEY]);
    iam::resource_decision(policy, &request) == Decision::Allowed
}

/// The global condition key naming the resource the call is made on behalf of, which for a
/// rotation is the secret being rotated ("aws:SourceArn", IAM's global condition keys).
const SOURCE_ARN_KEY: &str = "aws:SourceArn";

/// The global condition key naming that resource's account ("aws:SourceAccount").
const SOURCE_ACCOUNT_KEY: &str = "aws:SourceAccount";

/// Drive the rotation the call that returns this started: invoke the rotation function once
/// per step of [`ROTATION_STEPS`], and stamp the secret as rotated once the last step has
/// returned.
///
/// Returns at once: the rotation runs in a task of its own, because the call that started it
/// must answer while the rotation is still in flight. The caller hands over its own clone of
/// the in-process callback, so the invocations are made exactly where the starting call was
/// made.
pub fn drive<Eng: Send + Sync + 'static>(
    engine: Arc<Eng>,
    callback: EmbeddedEmulator,
    rotation: Rotation,
) {
    ::tokio::spawn(async move {
        run(&engine, &callback, &rotation).await;
    });
}

/// One rotation in flight: which secret, under which version id, through which function.
#[derive(Debug, Clone)]
pub struct Rotation {
    /// The Region the secret lives in, which is where its rotation function is invoked.
    pub region: String,
    /// The secret's own ARN, which is what the rotation event names it by.
    pub secret_arn: String,
    /// The id `RotateSecret` staked `AWSPENDING` on: the version the rotation function
    /// creates, which the event carries as its `ClientRequestToken`.
    pub version_id: String,
    /// The rotation function this rotation invokes.
    pub lambda_arn: String,
}

/// Invoke each step in turn, stopping at the first one the function did not complete, and
/// drive the whole rotation again when an attempt ended there.
///
/// A rotation is retried from its FIRST step ([`ROTATION_ATTEMPTS`]), which is what gets one
/// past an obstacle in front of the function that clears by itself. A rotation whose function
/// keeps raising ends where the real service leaves such a rotation — `AWSPENDING` staked,
/// `AWSCURRENT` where it was and `LastRotatedDate` unset — because the only writes to the
/// secret are the ones the rotation function itself made through this service's operations.
async fn run<Eng: Send + Sync + 'static>(
    engine: &Arc<Eng>,
    callback: &EmbeddedEmulator,
    rotation: &Rotation,
) {
    ::tokio::time::sleep(ROTATION_START_DELAY).await;
    for attempt in 1..=ROTATION_ATTEMPTS {
        // A rotation the secret is no longer waiting for is not driven: "if a rotation is
        // currently in progress, cancels the rotation" (`CancelRotateSecret`, this service's
        // Smithy model), and a secret that is gone awaits nothing either.
        if !still_awaited(engine, rotation) {
            ::tracing::debug!(
                secret = %rotation.secret_arn,
                version = %rotation.version_id,
                "this rotation is no longer the one the secret awaits, so it is not driven \
                 further"
            );
            return;
        }
        if drive_steps(callback, rotation).await {
            // Every step returned, so the rotation COMPLETED: "LastRotatedDate — the last
            // date and time that Secrets Manager rotated the secret"
            // (`DescribeSecretResponse`, this service's Smithy model), and the schedule's
            // next due instant is computed from it.
            stamp_rotated(engine, rotation);
            return;
        }
        if attempt < ROTATION_ATTEMPTS {
            ::tokio::time::sleep(ROTATION_RETRY_DELAY).await;
        }
    }
}

/// Invoke the four steps in order, and report whether every one of them completed.
async fn drive_steps(callback: &EmbeddedEmulator, rotation: &Rotation) -> bool {
    for step in ROTATION_STEPS {
        if !invoke_step(callback, rotation, step).await {
            return false;
        }
    }
    true
}

/// Whether the secret is still waiting for THIS rotation: it exists, rotation is on, and the
/// version this rotation creates is the one `AWSPENDING` names.
///
/// Read under the store's guard, between invocations rather than across one, so a rotation
/// cancelled while a step was in flight is not driven again.
fn still_awaited<Eng: Send + Sync + 'static>(engine: &Arc<Eng>, rotation: &Rotation) -> bool {
    let Some(state) = super::service_state(&**engine) else {
        return false;
    };
    let store = state.lock();
    store
        .get(&rotation.region, &rotation.secret_arn)
        .is_some_and(|secret| {
            secret.rotation_enabled
                && secret.pending_version_id() == Some(rotation.version_id.as_str())
        })
}

/// Stamp the secret as rotated, at the instant its rotation finished.
fn stamp_rotated<Eng: Send + Sync + 'static>(engine: &Arc<Eng>, rotation: &Rotation) {
    let Some(state) = super::service_state(&**engine) else {
        return;
    };
    let now = SystemTime::now();
    let mut store = state.lock();
    if let Some(secret) = store.get_mut(&rotation.region, &rotation.secret_arn) {
        secret.finish_rotation(now);
    }
}

/// Invoke the rotation function for one step, and report whether it completed.
///
/// `false` for an invocation that did not reach a function at all, and for one whose function
/// raised: "if present, indicates that an error occurred during function execution"
/// (`FunctionError`, Lambda's `Invoke` model), which is a step of the rotation that did not
/// happen.
async fn invoke_step(callback: &EmbeddedEmulator, rotation: &Rotation, step: &str) -> bool {
    let event = ::serde_json::json!({
        EVENT_STEP: step,
        EVENT_SECRET_ID: rotation.secret_arn,
        EVENT_CLIENT_REQUEST_TOKEN: rotation.version_id,
    });
    let answered = lambda_client(callback, &rotation.region)
        .invoke()
        .function_name(&rotation.lambda_arn)
        .invocation_type(::aws_sdk_lambda::types::InvocationType::RequestResponse)
        .payload(::aws_sdk_lambda::primitives::Blob::new(
            event.to_string().into_bytes(),
        ))
        .send();
    // The budget is this service's own, not the function's: a step still running when it
    // elapses is not waited for any longer, and the rotation it belongs to is left
    // unfinished.
    let Ok(answered) = ::tokio::time::timeout(ROTATION_STEP_BUDGET, answered).await else {
        ::tracing::warn!(
            secret = %rotation.secret_arn,
            step,
            "the rotation function did not answer this step inside its budget, so the \
             rotation did not finish"
        );
        return false;
    };
    match answered {
        Ok(output) => match output.function_error() {
            None => true,
            Some(function_error) => {
                ::tracing::warn!(
                    secret = %rotation.secret_arn,
                    step,
                    function_error,
                    "the rotation function raised on this step, so the rotation did not finish"
                );
                false
            }
        },
        Err(error) => {
            ::tracing::warn!(
                secret = %rotation.secret_arn,
                step,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "the rotation function could not be invoked for this step"
            );
            false
        }
    }
}

/// The error codes that are the in-process TRANSPORT's own refusal rather than an answer from
/// Lambda: this emulator holds no Lambda at the endpoint that was called, or more than one of
/// its services claims it. Either way the request reached no Lambda, so it says nothing about
/// the function — the same reading every service that calls another in process makes of these
/// two codes.
const TRANSPORT_REFUSALS: [&str; 2] = ["UnknownService", "AmbiguousService"];

/// Whether an SDK failure is one of those transport refusals.
fn reached_no_lambda<E, R>(error: &::aws_sdk_lambda::error::SdkError<E, R>) -> bool
where
    E: ::aws_sdk_lambda::error::ProvideErrorMetadata,
{
    ::aws_sdk_lambda::error::ProvideErrorMetadata::code(error)
        .is_some_and(|code| TRANSPORT_REFUSALS.contains(&code))
}

/// Lambda's endpoint prefix, the first label of its regional endpoint.
const LAMBDA_ENDPOINT_PREFIX: &str = "lambda";

/// Lambda's client for `region`, over the in-process callback into this same emulator: a
/// rotation function is one this emulator holds, so the invocation never leaves the process.
///
/// The transport is in-process, so this disables what a network client needs and this one
/// cannot have: retries, timeouts, stalled-stream protection and identity caching each need a
/// sleep implementation the callback transport is not given, and there is nothing to retry
/// behind it anyway.
fn lambda_client(callback: &EmbeddedEmulator, region: &str) -> ::aws_sdk_lambda::Client {
    let config = ::aws_sdk_lambda::Config::builder()
        .behavior_version(::aws_sdk_lambda::config::BehaviorVersion::latest())
        .region(::aws_sdk_lambda::config::Region::new(region.to_string()))
        .endpoint_url(format!(
            "https://{LAMBDA_ENDPOINT_PREFIX}.{region}.amazonaws.com"
        ))
        .credentials_provider(callback.aws_credentials_provider())
        .http_client(callback.http_client())
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

    /// A function policy statement granting `lambda:InvokeFunction` to `principal`, with the
    /// `Condition` block given (none when it is empty).
    fn policy(principal: &str, resource: &str, condition: &str) -> String {
        let clause = if condition.is_empty() {
            String::new()
        } else {
            format!(r#","Condition":{condition}"#)
        };
        format!(
            r#"{{"Version":"2012-10-17","Statement":[{{"Sid":"rotate","Effect":"Allow",
             "Principal":{{"Service":"{principal}"}},"Action":"lambda:InvokeFunction",
             "Resource":"{resource}"{clause}}}]}}"#
        )
    }

    /// RULE: the invocation is permitted exactly when the FUNCTION's policy allows
    /// `lambda:InvokeFunction` to this service's principal for the secret being rotated.
    ///
    /// The rows differ along who the statement grants to, which function it names, and how it
    /// is scoped: unscoped, scoped to this secret, scoped to another secret, scoped to the
    /// account, and the shape `AddPermission` writes (`ArnLike` over `AWS:SourceArn`).
    #[test]
    fn a_rotation_is_permitted_only_by_the_functions_own_grant() {
        let function = "arn:aws:lambda:us-west-2:000000000000:function:rotator";
        let secret = "arn:aws:secretsmanager:us-west-2:000000000000:secret:db-AbCdEf";
        let other = "arn:aws:secretsmanager:us-west-2:000000000000:secret:other-GhIjKl";
        // (case, the document, whether the rotation may be invoked)
        let rows: Vec<(&str, String, bool)> = vec![
            ("no policy at all", String::new(), false),
            (
                "an unscoped grant to this service",
                policy(SERVICE_PRINCIPAL, function, ""),
                true,
            ),
            (
                "a grant to another service principal",
                policy("events.amazonaws.com", function, ""),
                false,
            ),
            (
                "a grant on another function",
                policy(
                    SERVICE_PRINCIPAL,
                    "arn:aws:lambda:us-west-2:000000000000:function:elsewhere",
                    "",
                ),
                false,
            ),
            (
                "scoped to this secret the way AddPermission writes it",
                policy(
                    SERVICE_PRINCIPAL,
                    function,
                    &format!(r#"{{"ArnLike":{{"AWS:SourceArn":"{secret}"}}}}"#),
                ),
                true,
            ),
            (
                "scoped to another secret",
                policy(
                    SERVICE_PRINCIPAL,
                    function,
                    &format!(r#"{{"ArnLike":{{"AWS:SourceArn":"{other}"}}}}"#),
                ),
                false,
            ),
            (
                "scoped to the secret's own account",
                policy(
                    SERVICE_PRINCIPAL,
                    function,
                    r#"{"StringEquals":{"AWS:SourceAccount":"000000000000"}}"#,
                ),
                true,
            ),
            (
                "scoped to another account",
                policy(
                    SERVICE_PRINCIPAL,
                    function,
                    r#"{"StringEquals":{"AWS:SourceAccount":"111122223333"}}"#,
                ),
                false,
            ),
            (
                "an explicit deny of the same action",
                policy(SERVICE_PRINCIPAL, function, "").replace("Allow", "Deny"),
                false,
            ),
        ];
        for (case, document, permitted) in rows {
            let reached = permits_invocation(&document, function, secret);
            assert_eq!(
                reached, permitted,
                "case {case:?}: expected permitted={permitted}, got {reached} for {document}"
            );
        }
    }

    /// RULE: the four steps are the ones AWS invokes a rotation function with, in that order.
    #[test]
    fn the_steps_are_the_documented_four_in_order() {
        assert_eq!(
            ROTATION_STEPS,
            ["createSecret", "setSecret", "testSecret", "finishSecret"],
            "case \"the rotation steps\": expected AWS's four steps in order, got {ROTATION_STEPS:?}"
        );
    }
}
