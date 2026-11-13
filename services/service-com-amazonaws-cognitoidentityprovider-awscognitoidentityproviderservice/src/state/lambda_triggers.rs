// SPDX-License-Identifier: CC0-1.0

//! The user-pool Lambda-trigger dispatch: the one place a pool's `LambdaConfig` member
//! becomes an invocation of the function that member names.
//!
//! "A collection of user pool Lambda triggers. Amazon Cognito invokes triggers at several
//! possible stages of user pool operations. Triggers can modify the outcome of the
//! operations that invoked them." (`LambdaConfigType`, this service's model.) A configured
//! trigger is therefore not decoration an operation may ignore: the operation that reaches
//! the stage a member is configured for invokes that member's function with the event for
//! that stage, and reports what the function answered.
//!
//! Every member is dispatched the same way, through the two steps this module splits the
//! work into:
//!
//! 1. a builder ([`post_confirmation`], [`pre_authentication`], [`pre_token_generation`])
//!    resolves the member and builds the event, from the pool and the user read under the
//!    state lock;
//! 2. [`invoke`] performs the invocation, after the operation has RELEASED that lock, so
//!    no other request of this service waits on a function.
//!
//! What a trigger's answer DECIDES is the stage it sits at. A trigger at a stage AFTER the
//! outcome it could have modified — "this trigger can take custom actions after a user
//! confirms their user account" (`LambdaConfigType$PostConfirmation`) — runs once that
//! outcome is settled, so an invocation that reached no function leaves the operation's own
//! answer standing. A trigger at a stage BEFORE it — one that "can evaluate and modify user
//! sign-in events" (`LambdaConfigType$PreAuthentication`) — GATES the operation: the
//! operation it gates continues only on a function that returned, so [`invoke_gating`]
//! turns every other outcome into the operation's own modelled Lambda error.
//!
//! The function is invoked over the in-process callback into this same emulator
//! ([`lambda_client`]), so a trigger reaches only a function this emulator itself holds,
//! and every invocation is bounded by [`TRIGGER_INVOCATION_BUDGET`].

use std::collections::HashMap;

use ::serde_json::{Map, Value};

use super::{User, UserPool};

/// The custom email sender trigger, whose configured function every code-bearing message of
/// a pool that configures one is routed to instead of being mailed.
pub mod custom_email;

/// The encrypted message a custom sender's code travels in.
pub mod esdk;

/// What a pre-token-generation function's answer changes about the tokens that generation
/// mints: the claims, the scopes and the group configuration it asked them to carry.
pub mod token_overrides;

/// The migrate-user trigger, which a pool that configures one resolves a username it does
/// not hold through before the request naming it is refused.
pub mod user_migration;

pub use token_overrides::TokenOverrides;

/// This service's state, as the operation that fires a trigger holds it under the lock.
type State = super::ComAmazonawsCognitoidentityproviderAwscognitoidentityproviderserviceState;

/// The ARN namespace of AWS Lambda, whose functions a user-pool trigger names.
const LAMBDA_ARN_NAMESPACE: &str = "lambda";

/// A user-pool trigger event's `version` where the trigger has only one: the version
/// Amazon Cognito documents the event shape of. A trigger whose configuration names a
/// trigger VERSION is delivered the event version that configuration asks for instead
/// ([`event_version`]).
const EVENT_VERSION: &str = "1";

/// `callerContext.clientId` of an event fired by an operation that names no app client —
/// an `Admin*` operation, which IAM credentials authorize rather than an app client. The
/// value Amazon Cognito reports in that member's place.
const CLIENT_ID_NOT_APPLICABLE: &str = "CLIENT_ID_NOT_APPLICABLE";

/// `callerContext.awsSdkVersion` of an event fired by a request that says which SDK made
/// it nowhere: the placeholder Amazon Cognito reports for an SDK it cannot name.
const UNKNOWN_AWS_SDK_VERSION: &str = "aws-sdk-unknown-unknown";

/// The reserved name a trigger event's `request.userAttributes` carries the user's STATUS
/// under. The status is not one of the user's stored attributes — it is rendered beside
/// them, from the status the pool holds for the user.
const USER_STATUS_ATTRIBUTE: &str = "cognito:user_status";

/// The trigger whose configured function every post-confirmation event is delivered to,
/// as a failure of that trigger names it ([`failure_message`]).
const POST_CONFIRMATION_TRIGGER: &str = "PostConfirmation";

/// The trigger whose configured function every pre-authentication event is delivered to.
const PRE_AUTHENTICATION_TRIGGER: &str = "PreAuthentication";

/// The one `triggerSource` a pre-authentication event carries: the trigger's name and the
/// stage that reached it, which for this trigger is the sign-in itself.
const PRE_AUTHENTICATION_SOURCE: &str = "PreAuthentication_Authentication";

/// The trigger whose configured function every pre-token-generation event is delivered to.
const PRE_TOKEN_GENERATION_TRIGGER: &str = "PreTokenGeneration";

/// The trigger version a pool that configures the LEGACY `PreTokenGeneration` member alone
/// asks for: `PreTokenGenerationVersionConfigType.LambdaVersion` is "the user pool trigger
/// version of the request that Amazon Cognito sends to your Lambda function. Higher-numbered
/// versions add fields that support new features", and the legacy member predates every
/// higher one (`PreTokenGenerationLambdaVersionType` in this service's model).
const PRE_TOKEN_GENERATION_V1: &str = "V1_0";

/// Amazon Cognito's budget for ONE trigger invocation: the service's documented Lambda
/// invocation timeout for user pool triggers is five seconds, after which the invocation is
/// given up on rather than waited out, whatever the function's own configured timeout is.
const TRIGGER_INVOCATION_BUDGET: ::std::time::Duration = ::std::time::Duration::from_secs(5);

/// The error an invocation that outlived [`TRIGGER_INVOCATION_BUDGET`] carries: the
/// function was not waited for any longer, so it gave no verdict.
const TRIGGER_TIMEOUT_ERROR: &str = "Socket timeout while invoking Lambda function";

/// Which completed confirmation fires a post-confirmation event, which is what the
/// event's `triggerSource` reports: the trigger's name and the operation that reached it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Confirmation {
    /// A sign-up confirmed — by `ConfirmSignUp` with the code the sign-up sent, or by
    /// `AdminConfirmSignUp` without one.
    SignUp,
    /// A forgotten password reset and the reset code answered, by
    /// `ConfirmForgotPassword`.
    ForgotPassword,
}

impl Confirmation {
    /// This confirmation's `triggerSource`.
    #[must_use]
    pub fn trigger_source(self) -> &'static str {
        match self {
            Self::SignUp => "PostConfirmation_ConfirmSignUp",
            Self::ForgotPassword => "PostConfirmation_ConfirmForgotPassword",
        }
    }
}

/// Which token generation fires a pre-token-generation event, which is what the event's
/// `triggerSource` reports: the trigger's name and the stage that reached it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenGeneration {
    /// A completed authentication: a password sign-in, or a challenge answered with the
    /// factor the sign-in asked for.
    Authentication,
    /// A refresh token exchanged for fresh tokens.
    RefreshTokens,
    /// The `NEW_PASSWORD_REQUIRED` challenge answered, which both completes the sign-in
    /// and sets the password the user chose.
    NewPasswordChallenge,
}

impl TokenGeneration {
    /// This generation's `triggerSource`.
    #[must_use]
    pub fn trigger_source(self) -> &'static str {
        match self {
            Self::Authentication => "TokenGeneration_Authentication",
            Self::RefreshTokens => "TokenGeneration_RefreshTokens",
            Self::NewPasswordChallenge => "TokenGeneration_NewPasswordChallenge",
        }
    }
}

/// What the request that fired a trigger contributes to its event's `callerContext`.
#[derive(Debug, Clone, Copy)]
pub struct Caller<'request> {
    /// The app client the request named, or `None` for an operation that names none.
    pub client_id: Option<&'request str>,
    /// The SDK the request announced itself as.
    pub aws_sdk_version: Option<&'request str>,
}

impl<'request> Caller<'request> {
    /// The caller of the request `parts`, which named the app client `client_id`.
    ///
    /// `awsSdkVersion` is read from the request's own `User-Agent`, the only place a
    /// request says which SDK made it; a request carrying none, or one whose value is not
    /// text, contributes none and the event carries
    /// [`UNKNOWN_AWS_SDK_VERSION`] in its place.
    #[must_use]
    pub fn of(parts: &'request ::http::request::Parts, client_id: Option<&'request str>) -> Self {
        Self {
            client_id,
            aws_sdk_version: parts
                .headers
                .get(::http::header::USER_AGENT)
                .and_then(|value| value.to_str().ok()),
        }
    }
}

/// One invocation an operation owes: the function to invoke, the Region whose Lambda
/// endpoint holds it, the trigger it serves and the event to deliver.
#[derive(Debug, Clone)]
pub struct Dispatch {
    function_arn: String,
    region: String,
    trigger: &'static str,
    event: Value,
}

impl Dispatch {
    /// The event this invocation delivers to the function.
    #[must_use]
    pub fn event(&self) -> &Value {
        &self.event
    }

    /// The function this invocation is of, as the pool's `LambdaConfig` names it.
    #[must_use]
    pub fn function_arn(&self) -> &str {
        &self.function_arn
    }
}

/// The invocation a completed confirmation of `user` in `pool` owes, or `None` when the
/// pool configures no post-confirmation function and so nothing is invoked.
///
/// Built while the caller still holds the state lock — the event reports the pool and the
/// user as the completed operation left them — and invoked by [`invoke`] once that lock is
/// released.
///
/// The Region is the one the configured function's own ARN names, so a trigger function in
/// another Region is reached through that Region's Lambda endpoint; a member that names no
/// Region label falls back to the pool's own Region.
#[must_use]
pub fn post_confirmation(
    pool: &UserPool,
    user: &User,
    confirmation: Confirmation,
    caller: Caller<'_>,
    client_metadata: Option<&HashMap<String, String>>,
) -> Option<Dispatch> {
    let function_arn = pool.lambda_config.post_confirmation.clone()?;
    let event = event(
        pool,
        user,
        confirmation.trigger_source(),
        EVENT_VERSION,
        caller,
        client_metadata,
        Map::new(),
    );
    Some(dispatch(
        pool,
        function_arn,
        POST_CONFIRMATION_TRIGGER,
        event,
    ))
}

/// The pre-authentication invocation a sign-in of `username` in `user_pool_id` owes, or
/// `None` when the pool configures no pre-authentication function, or holds no such user,
/// and so nothing is invoked.
///
/// "The configuration of a pre authentication trigger in a user pool. This trigger can
/// evaluate and modify user sign-in events" (`LambdaConfigType$PreAuthentication`): the
/// sign-in it evaluates has not happened yet, so the operation that fires it is GATED on
/// what the function answers ([`invoke_gating`]).
///
/// Built while the caller still holds the state lock, and invoked by [`invoke`] once that
/// lock is released.
#[must_use]
pub fn pre_authentication(
    state: &State,
    user_pool_id: &str,
    username: &str,
    caller: Caller<'_>,
    client_metadata: Option<&HashMap<String, String>>,
) -> Option<Dispatch> {
    let pool = state.pool(user_pool_id)?;
    let user = pool.user(username)?;
    let function_arn = pool.lambda_config.pre_authentication.clone()?;
    let mut members = Map::new();
    // "validationData — One or more key-value pairs containing the validation data in the
    // user's sign-in request. You can pass this data to your Lambda function by using the
    // ClientMetadata parameter in the InitiateAuth and AdminInitiateAuth API actions"
    // (Amazon Cognito's pre-authentication event), so it is the metadata THIS request
    // assigned, and a request that assigned none carries none.
    if let Some(metadata) = client_metadata.filter(|metadata| !metadata.is_empty()) {
        members.insert(
            "validationData".to_string(),
            Value::Object(metadata_members(metadata)),
        );
    }
    // The stage this event reports resolved a stored user, so the sign-in is not one of a
    // user the pool does not hold.
    members.insert("userNotFound".to_string(), Value::Bool(false));
    let event = event(
        pool,
        user,
        PRE_AUTHENTICATION_SOURCE,
        EVENT_VERSION,
        caller,
        client_metadata,
        members,
    );
    Some(dispatch(
        pool,
        function_arn,
        PRE_AUTHENTICATION_TRIGGER,
        event,
    ))
}

/// The pre-token-generation invocation the token `generation` for `username` in
/// `user_pool_id` owes, or `None` when the pool configures no pre-token-generation
/// function, or holds no such user, and so nothing is invoked.
///
/// The function is the one `PreTokenGenerationConfig` names, and the LEGACY
/// `PreTokenGeneration` member names it for a pool that carries only that one: "this
/// parameter and the `PreTokenGeneration` property of `LambdaConfig` have the same value"
/// (`PreTokenGenerationVersionConfigType$LambdaArn` in this service's model). The event's
/// `version` is the major number of the trigger version that configuration asks for, since
/// "higher-numbered versions add fields that support new features"
/// (`PreTokenGenerationVersionConfigType$LambdaVersion`).
///
/// The tokens it precedes have not been minted yet, so the operation that fires it is GATED
/// on what the function answers ([`invoke_gating`]).
#[must_use]
pub fn pre_token_generation(
    state: &State,
    user_pool_id: &str,
    username: &str,
    generation: TokenGeneration,
    caller: Caller<'_>,
    client_metadata: Option<&HashMap<String, String>>,
) -> Option<Dispatch> {
    let pool = state.pool(user_pool_id)?;
    let user = pool.user(username)?;
    let configured = &pool.lambda_config;
    let (trigger_version, function_arn) = match configured.pre_token_generation_config.as_ref() {
        Some(versioned) => (
            versioned.lambda_version.clone(),
            versioned.lambda_arn.clone(),
        ),
        None => (
            PRE_TOKEN_GENERATION_V1.to_string(),
            configured.pre_token_generation.clone()?,
        ),
    };

    let mut members = Map::new();
    // "groupConfiguration — The input object containing the current group configuration":
    // the groups the pool holds this user in, the roles those groups grant and the one role
    // they prefer — the very claims the tokens this generation mints would otherwise carry
    // (`crate::state::UserPool::user_group_names` and `user_group_role_claims`).
    let role_claims = pool.user_group_role_claims(user);
    let mut group_configuration = Map::new();
    group_configuration.insert(
        "groupsToOverride".to_string(),
        text_list(pool.user_group_names(user)),
    );
    group_configuration.insert(
        "iamRolesToOverride".to_string(),
        text_list(role_claims.roles),
    );
    if let Some(preferred) = role_claims.preferred_role {
        group_configuration.insert("preferredRole".to_string(), Value::String(preferred));
    }
    members.insert(
        "groupConfiguration".to_string(),
        Value::Object(group_configuration),
    );
    // The scopes member arrived with the second trigger version, so only an event of that
    // version or later carries it: it reports the scopes the access token this generation
    // mints is authorized for, read where that token's own claim reads them.
    if trigger_version.as_str() != PRE_TOKEN_GENERATION_V1 {
        members.insert(
            "scopes".to_string(),
            text_list(vec![super::USER_ADMIN_SCOPE.to_string()]),
        );
    }

    let event = event(
        pool,
        user,
        generation.trigger_source(),
        &event_version(&trigger_version),
        caller,
        client_metadata,
        members,
    );
    Some(dispatch(
        pool,
        function_arn,
        PRE_TOKEN_GENERATION_TRIGGER,
        event,
    ))
}

/// The event `version` a trigger version asks for: its major number, so `V2_0` is answered
/// with the second version of the event. A version this service does not recognise is
/// delivered the first one, the only version every trigger has.
fn event_version(trigger_version: &str) -> String {
    trigger_version
        .strip_prefix('V')
        .and_then(|version| version.split('_').next())
        .filter(|major| !major.is_empty() && major.chars().all(char::is_numeric))
        .unwrap_or(EVENT_VERSION)
        .to_string()
}

/// One invocation of `function_arn` for `trigger`, delivering `event`.
///
/// The Region is the one the configured function's own ARN names, so a trigger function in
/// another Region is reached through that Region's Lambda endpoint; a member that names no
/// Region label falls back to the pool's own Region.
fn dispatch(
    pool: &UserPool,
    function_arn: String,
    trigger: &'static str,
    event: Value,
) -> Dispatch {
    let region = ::service_common::arn::region_of(&function_arn, LAMBDA_ARN_NAMESPACE)
        .unwrap_or(pool.region.as_str())
        .to_string();
    Dispatch {
        function_arn,
        region,
        trigger,
        event,
    }
}

/// The event a user-pool trigger is invoked with: which trigger fired, in which pool, for
/// which user, on behalf of which caller, with the user's attributes and the stage's own
/// `members` as the request half of the event.
#[allow(clippy::too_many_arguments)]
fn event(
    pool: &UserPool,
    user: &User,
    trigger_source: &str,
    version: &str,
    caller: Caller<'_>,
    client_metadata: Option<&HashMap<String, String>>,
    members: Map<String, Value>,
) -> Value {
    let mut request = members;
    request.insert(
        "userAttributes".to_string(),
        Value::Object(user_attributes(user)),
    );
    // `ClientMetadata` is "a map of custom key-value pairs that you can provide as input
    // for any custom workflows that this action triggers […] This payload contains a
    // clientMetadata attribute that provides the data that you assigned to the
    // ClientMetadata parameter in your request" (every operation that models it), so the
    // member appears only for a request that assigned one.
    if let Some(metadata) = client_metadata.filter(|metadata| !metadata.is_empty()) {
        request.insert(
            "clientMetadata".to_string(),
            Value::Object(metadata_members(metadata)),
        );
    }
    ::serde_json::json!({
        "version": version,
        "triggerSource": trigger_source,
        // The pool's own Region and id, not the request's: the event names the resource
        // the trigger is configured on.
        "region": pool.region,
        "userPoolId": pool.id,
        "userName": user.username,
        "callerContext": {
            "awsSdkVersion": caller.aws_sdk_version.unwrap_or(UNKNOWN_AWS_SDK_VERSION),
            "clientId": caller.client_id.unwrap_or(CLIENT_ID_NOT_APPLICABLE),
        },
        "request": Value::Object(request),
        // The response half is the function's to fill in, so the event is delivered with
        // none of it: a post-confirmation function is handed nothing to amend at all — the
        // trigger "can take custom actions after a user confirms", which is after the
        // outcome it would otherwise modify is already settled — and a gating function's
        // amendments are what its answer carries back.
        "response": Value::Object(Map::new()),
    })
}

/// A `ClientMetadata` map as a trigger event's members.
fn metadata_members(metadata: &HashMap<String, String>) -> Map<String, Value> {
    metadata
        .iter()
        .map(|(name, value)| (name.clone(), Value::String(value.clone())))
        .collect()
}

/// Text values as a trigger event's JSON list, in the order the state holds them.
fn text_list(values: Vec<String>) -> Value {
    Value::Array(values.into_iter().map(Value::String).collect())
}

/// The user's attributes as a trigger event's `request.userAttributes`: every attribute
/// the pool stores for the user, rendered the way every other read of this service renders
/// them, with the user's status beside them under [`USER_STATUS_ATTRIBUTE`].
fn user_attributes(user: &User) -> Map<String, Value> {
    let mut attributes: Map<String, Value> = user
        .attribute_list()
        .into_iter()
        .map(|attribute| {
            (
                attribute.name,
                Value::String(attribute.value.unwrap_or_default()),
            )
        })
        .collect();
    attributes.insert(
        USER_STATUS_ATTRIBUTE.to_string(),
        Value::String(user.status.as_str().to_string()),
    );
    attributes
}

/// What a trigger invocation leaves the operation that owed it to report, when the
/// operation's own success is no longer what it answers.
///
/// Both variants carry the message the refusal states, built by [`failure_message`]; the
/// operation wraps it in its OWN modelled error, because the error channel belongs to the
/// operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    /// The function RAN and reported an error. The operation's
    /// `UserLambdaValidationException` — "a user validation exception with the Lambda
    /// service" (this service's model) — is what that amounts to.
    Reported(String),
    /// The function did not run: Lambda reports the configured function as one it cannot
    /// invoke at all, or it did not answer inside [`TRIGGER_INVOCATION_BUDGET`]. The
    /// operation's `UnexpectedLambdaException` — "an unexpected exception with Lambda"
    /// (this service's model) — is what that amounts to.
    NotInvokable(String),
}

/// What an invocation that was not a failure of the trigger left behind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// The function ANSWERED, with this document: the event it was handed, carrying
    /// whatever the function amended in its `response` half. `Value::Null` for a function
    /// that answered with no payload at all.
    Answered(Value),
    /// The invocation did not reach the function, so the function gave no verdict and none
    /// is invented for it. Carries the message an operation that cannot proceed without one
    /// states ([`invoke_gating`]).
    NoVerdict(String),
}

/// How Amazon Cognito states a user-pool trigger's failure: the trigger's name, then the
/// error that trigger produced, terminated once — an error that already ends in a
/// terminator keeps its own rather than being given a second.
#[must_use]
fn failure_message(trigger: &str, error: &str) -> String {
    let error = error.trim();
    if error.ends_with('.') {
        format!("{trigger} failed with error {error}")
    } else {
        format!("{trigger} failed with error {error}.")
    }
}

/// Fire the pre-authentication trigger of a sign-in of `username` in `user_pool_id`, and
/// report what it leaves the sign-in to answer.
///
/// For an operation that holds NO lock: the pool and the user are read under a lock of its
/// own, which is released before the invocation, so no other request of this service waits
/// on the function. A pool that configures no such function is answered by nothing being
/// invoked at all.
pub async fn fire_pre_authentication(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    store: &::engine::ServiceState<State>,
    user_pool_id: &str,
    username: &str,
    caller: Caller<'_>,
    client_metadata: Option<&HashMap<String, String>>,
) -> Result<(), Failure> {
    let dispatch = pre_authentication(&store.lock(), user_pool_id, username, caller, client_metadata);
    match dispatch {
        Some(dispatch) => invoke_gating(ctx, &dispatch).await.map(|_| ()),
        None => Ok(()),
    }
}

/// Fire the pre-token-generation trigger of the token `generation` for `username` in
/// `user_pool_id`, and report what it leaves the generating operation to answer.
///
/// What a RETURNING function answered is what the tokens of that generation carry: the
/// trigger sits before the triple it can "modify the outcome of" (`LambdaConfigType`), so
/// its answer is handed back as the overrides the minting applies ([`TokenOverrides`]) —
/// and a generation that invoked nothing, because the pool configures no such function,
/// overrides nothing.
///
/// Locks exactly as [`fire_pre_authentication`] does, and for the same reason.
pub async fn fire_pre_token_generation(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    store: &::engine::ServiceState<State>,
    user_pool_id: &str,
    username: &str,
    generation: TokenGeneration,
    caller: Caller<'_>,
    client_metadata: Option<&HashMap<String, String>>,
) -> Result<TokenOverrides, Failure> {
    let dispatch = pre_token_generation(
        &store.lock(),
        user_pool_id,
        username,
        generation,
        caller,
        client_metadata,
    );
    match dispatch {
        Some(dispatch) => invoke_gating(ctx, &dispatch)
            .await
            .map(|answered| TokenOverrides::read(&answered)),
        None => Ok(TokenOverrides::none()),
    }
}

/// Invoke the function of a trigger that GATES its operation — one at a stage before the
/// outcome it can modify — and report what the function answered.
///
/// Such an operation continues only on a function that RETURNED: a trigger that "can
/// evaluate and modify user sign-in events" (`LambdaConfigType$PreAuthentication`) decides
/// an outcome that has not happened yet, so an invocation that reached no function leaves
/// the operation nothing to proceed on, and Amazon Cognito answers it with the operation's
/// own Lambda error rather than the success the function never allowed.
pub async fn invoke_gating(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    dispatch: &Dispatch,
) -> Result<Value, Failure> {
    match invoke(ctx, dispatch).await? {
        Verdict::Answered(answer) => Ok(answer),
        Verdict::NoVerdict(message) => Err(Failure::NotInvokable(message)),
    }
}

/// Invoke the function of `dispatch` and report what the trigger leaves the operation to
/// answer.
///
/// The invocation is SYNCHRONOUS: a trigger "can modify the outcome of the operations that
/// invoked them" (`LambdaConfigType`), which the invoking operation can only honour by
/// waiting for the function's answer — for as long as
/// [`TRIGGER_INVOCATION_BUDGET`] and no longer, so a function that does not answer fails
/// the trigger instead of holding the caller.
///
/// Call it with the state lock RELEASED: the operation's own writes are already persisted,
/// and holding the lock across the invocation would make every other request of this
/// service wait on the function.
pub async fn invoke(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    dispatch: &Dispatch,
) -> Result<Verdict, Failure> {
    let invocation = lambda_client(ctx, &dispatch.region)
        .invoke()
        .function_name(&dispatch.function_arn)
        .invocation_type(::aws_sdk_lambda::types::InvocationType::RequestResponse)
        .payload(::aws_sdk_lambda::primitives::Blob::new(
            dispatch.event.to_string().into_bytes(),
        ))
        .send();
    // The budget is Amazon Cognito's own, not the function's: a function still running
    // when it elapses is not waited for any longer, and the invocation it was given up on
    // produced no verdict.
    let Ok(answer) = ::tokio::time::timeout(TRIGGER_INVOCATION_BUDGET, invocation).await else {
        return Err(Failure::NotInvokable(failure_message(
            dispatch.trigger,
            TRIGGER_TIMEOUT_ERROR,
        )));
    };
    let output = match answer {
        Ok(output) => output,
        // Lambda says this account holds no such function: the pool's trigger names a
        // function that cannot be invoked, which is the operation's failure to report.
        Err(error) if function_is_absent(&error) => {
            return Err(Failure::NotInvokable(failure_message(
                dispatch.trigger,
                &error_detail(&error),
            )));
        }
        // Any other answer is this emulator failing to ASK, not Lambda reporting a
        // function it cannot invoke — so the function's verdict is not known, and none is
        // invented for it. What that leaves the operation is the operation's own to decide
        // from the stage its trigger sits at: a post-confirmation trigger "can take custom
        // actions AFTER a user confirms their user account", so the confirmation this
        // invocation follows is already complete and is not undone by an invocation that
        // did not reach the function, while a gating trigger's operation has nothing to
        // proceed on ([`invoke_gating`]). The warning is what says the trigger's own side
        // effects did not happen.
        Err(error) => {
            ::tracing::warn!(
                function_arn = %dispatch.function_arn,
                trigger = dispatch.trigger,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "a user pool Lambda trigger could not be invoked; the operation that \
                 fired it reports no verdict from the function"
            );
            return Ok(Verdict::NoVerdict(failure_message(
                dispatch.trigger,
                &error_detail(&error),
            )));
        }
    };
    // "FunctionError: if present, indicates that an error occurred during function
    // execution. Details about the error are included in the response payload"
    // (Lambda's `Invoke` model) — so the payload is where the error the trigger
    // produced is read from, and the handling kind is the fallback when it carries none.
    if let Some(function_error) = output.function_error() {
        let detail = reported_error(output.payload())
            .unwrap_or_else(|| function_error.to_string());
        return Err(Failure::Reported(failure_message(
            dispatch.trigger,
            &detail,
        )));
    }
    // The function returned, so its answer is what it answered with: a post-confirmation
    // function's is not read — the trigger runs after the outcome it could have modified —
    // and a gating function's is handed back to the operation it gates.
    Ok(Verdict::Answered(answered_event(output.payload())))
}

/// The document a returning function answered with, or `Value::Null` for a function that
/// answered with no payload or with one that is not JSON at all.
fn answered_event(payload: Option<&::aws_sdk_lambda::primitives::Blob>) -> Value {
    payload
        .and_then(|payload| ::serde_json::from_slice(payload.as_ref()).ok())
        .unwrap_or(Value::Null)
}

/// The error a failed function reported, read from the `errorMessage` of the payload
/// Lambda returns for it. `None` for a payload that carries none, including one that is
/// not a JSON object.
fn reported_error(payload: Option<&::aws_sdk_lambda::primitives::Blob>) -> Option<String> {
    let parsed: Value = ::serde_json::from_slice(payload?.as_ref()).ok()?;
    Some(parsed.get("errorMessage")?.as_str()?.to_string())
}

/// Whether an `Invoke` failure is Lambda reporting that the function is not there, rather
/// than this emulator failing to ask.
fn function_is_absent(
    error: &::aws_sdk_lambda::error::SdkError<
        ::aws_sdk_lambda::operation::invoke::InvokeError,
        ::aws_smithy_runtime_api::client::orchestrator::HttpResponse,
    >,
) -> bool {
    error
        .as_service_error()
        .is_some_and(::aws_sdk_lambda::operation::invoke::InvokeError::is_resource_not_found_exception)
}

/// The message a refused invocation carries, as Lambda itself worded it.
fn error_detail<E: ::aws_smithy_types::error::metadata::ProvideErrorMetadata>(
    error: &E,
) -> String {
    error
        .message()
        .unwrap_or("the function could not be invoked")
        .to_string()
}

/// Lambda's client for `region`, over the in-process callback into this same emulator: a
/// trigger function is one this emulator holds, so the invocation never leaves the
/// process.
///
/// The transport is in-process, so this disables what a network client needs and this one
/// cannot have: retries, timeouts, stalled-stream protection and identity caching each
/// need a sleep implementation the callback transport is not given, and there is nothing
/// to retry behind it anyway. The same client policy every other service's in-process
/// Lambda invocation builds (`service-com-amazonaws-s3-amazons3/src/state/notifications.rs`).
fn lambda_client(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> ::aws_sdk_lambda::Client {
    let config = ::aws_sdk_lambda::Config::builder()
        .behavior_version(::aws_sdk_lambda::config::BehaviorVersion::latest())
        .http_client(ctx.callback().http_client())
        .credentials_provider(ctx.callback().aws_credentials_provider())
        .region(::aws_sdk_lambda::config::Region::new(region.to_string()))
        .endpoint_url(format!(
            "https://{LAMBDA_ARN_NAMESPACE}.{region}.amazonaws.com"
        ))
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

    /// Every `triggerSource` this module reports names the trigger whose configured
    /// function serves it, so a function cannot be handed an event of another trigger.
    #[test]
    fn every_trigger_source_names_the_post_confirmation_trigger() {
        for confirmation in [Confirmation::SignUp, Confirmation::ForgotPassword] {
            let source = confirmation.trigger_source();
            assert!(
                source.starts_with(&format!("{POST_CONFIRMATION_TRIGGER}_")),
                "case {confirmation:?}: expected a {POST_CONFIRMATION_TRIGGER} trigger \
                 source, got {source:?}"
            );
        }
    }

    /// The two confirmations are distinct sources: one operation's event never claims to
    /// be another's.
    #[test]
    fn the_two_confirmations_report_different_sources() {
        assert_ne!(
            Confirmation::SignUp.trigger_source(),
            Confirmation::ForgotPassword.trigger_source(),
            "expected one trigger source per confirming operation, got the same for both"
        );
    }

    /// A failure message states the trigger and the error once, with one terminating
    /// period whatever the error's own punctuation.
    #[test]
    fn a_failure_message_states_the_trigger_and_the_error() {
        // (case, error as the invocation reported it, expected message)
        let rows = [
            (
                "a message with no punctuation",
                "Function not found: arn:aws:lambda:us-east-1:000000000000:function:f",
                "PostConfirmation failed with error Function not found: \
                 arn:aws:lambda:us-east-1:000000000000:function:f.",
            ),
            (
                "a message that ends in a period",
                "the handler raised KeyError.",
                "PostConfirmation failed with error the handler raised KeyError.",
            ),
            (
                "a message padded with whitespace",
                "  not authorized  ",
                "PostConfirmation failed with error not authorized.",
            ),
            (
                "a message that ends in an ellipsis",
                "the handler timed out...",
                "PostConfirmation failed with error the handler timed out...",
            ),
        ];
        for (case, error, expected) in rows {
            let message = failure_message(POST_CONFIRMATION_TRIGGER, error);
            assert_eq!(
                message, expected,
                "case {case:?}: expected {expected:?}, got {message:?}"
            );
        }
    }
}
