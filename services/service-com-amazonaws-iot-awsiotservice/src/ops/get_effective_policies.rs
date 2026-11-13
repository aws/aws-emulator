// SPDX-License-Identifier: CC0-1.0

//! @op GetEffectivePolicies
//!
//! The policies that govern one device's authorization, computed from the persisted
//! POLICY ATTACHMENT relation ([`crate::state::policy_attachments`]) and the policy
//! registry — a pure read: nothing about this answer is recorded, so two identical calls
//! answer identically.
//!
//! The operation "Gets a list of the policies that have an effect on the authorization
//! behavior of the specified device when it connects to the IoT device gateway", so the
//! answer is the UNION of the attachments of every target the request's identity resolves
//! to, with each policy reported once:
//!
//! * `principal` — a certificate ARN, a thing group ARN or a Cognito identity, the forms
//!   the modelled member documents. A thing group's effective policies include those
//!   attached to the groups ABOVE it, because a policy attached to a parent group governs
//!   the groups beneath it (the scope
//!   [`crate::state::ComAmazonawsIotAwsiotserviceState::policy_attachment_scope`] walks).
//! * `cognitoIdentityPoolId` — the `<region>:<id>` form a Cognito identity pool is named
//!   by, resolved no further than its form, because identity pools live in another
//!   service. It is MUTUALLY EXCLUSIVE with `principal`. A pool and an identity are named
//!   in the SAME form, so both share one key space in the relation: a policy attached to
//!   `<region>:<id>` governs whoever is named by that string, which is what
//!   `AttachPolicy` records.
//! * `thingName` — the device the question is about. It must name a thing this account
//!   holds; the policies it adds are those of the relations a thing takes part in — the
//!   thing groups it belongs to and the principals attached to it — and nothing in this
//!   service records either relation yet (`AddThingToThingGroup`,
//!   `UpdateThingGroupsForThing` and `AttachThingPrincipal` are still scaffolds), so a
//!   thing adds no policy of its own until one of them does.
//!
//! Each reported policy carries the registry's own `policyName` and `policyArn` and the
//! document of the version in effect, which is the document `GetPolicy` reports — one
//! policy, one document, whichever operation reads it. A name the registry no longer
//! holds governs nothing and is left out rather than reported without a document.
//!
//! ---
//!
//! # Modeled contract for `GetEffectivePolicies`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-iot-awsiotservice/smithy-model/com-amazonaws-iot-awsiotservice.json")
//! ```
//!
//! Gets a list of the policies that have an effect on the authorization behavior of the
//! specified device when it connects to the IoT device gateway. Requires permission to
//! access the GetEffectivePolicies action.
//!
//! ## Input
//!
//! * `cognitoIdentityPoolId` (CognitoIdentityPoolId) — The Cognito identity pool ID.
//! * `principal` (Principal) — The principal. Valid principals are CertificateArn
//!   (arn:aws:iot:region:accountId:cert/certificateId), thingGroupArn
//!   (arn:aws:iot:region:accountId:thinggroup/groupName) and CognitoId (region:id).
//! * `thingName` (ThingName, len 1..128, pattern ^[a-zA-Z0-9:_-]+$) — The thing name.
//!
//! ## Output
//!
//! * `effectivePolicies` (EffectivePolicies) — The effective policies.
//!
//! ## Errors
//!
//! * `InternalFailureException` (HTTP 500) — An unexpected error has occurred.
//! * `InvalidRequestException` (HTTP 400) — The request is not valid.
//! * `LimitExceededException` (HTTP 410) — A limit has been exceeded.
//! * `ResourceNotFoundException` (HTTP 404) — The specified resource does not exist.
//! * `ServiceUnavailableException` (HTTP 503) — The service is temporarily unavailable.
//! * `ThrottlingException` (HTTP 400) — The rate exceeds the limit.
//! * `UnauthorizedException` (HTTP 401) — You are not authorized to perform this operation.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `GetEffectivePoliciesError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-iot-awsiotservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `GetEffectivePoliciesError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.iot#GetEffectivePolicies")]
#[derive(Debug)]
pub enum GetEffectivePoliciesErrorModeled {
    InternalFailureException(InternalFailureException),
    InvalidRequestException(InvalidRequestException),
    LimitExceededException(LimitExceededException),
    ResourceNotFoundException(ResourceNotFoundException),
    ServiceUnavailableException(ServiceUnavailableException),
    ThrottlingException(ThrottlingException),
    UnauthorizedException(UnauthorizedException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::get_effective_policies::GetEffectivePoliciesError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type GetEffectivePoliciesError = crate::scaffolded::Scaffolded<GetEffectivePoliciesErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<GetEffectivePoliciesErrorModeled> for GetEffectivePoliciesError {
    fn from(modeled: GetEffectivePoliciesErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `GetEffectivePolicies` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct GetEffectivePolicies<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> GetEffectivePolicies<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for GetEffectivePolicies<Eng>
{
    type Input = GetEffectivePoliciesInput;
    type Output = GetEffectivePoliciesOutput;
    type Error = GetEffectivePoliciesError;

    const NAME: &'static str = "GetEffectivePolicies";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_EFFECTIVE_POLICIES_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_EFFECTIVE_POLICIES_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // Every policy this read reports carries the ARN minted when it was created, so
        // the answer needs no request metadata of its own.
        let _ = ctx;

        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            GetEffectivePoliciesErrorModeled::InternalFailureException(InternalFailureException {
                message: Some("the IoT emulator has no state store attached".to_string()),
            })
        })?;

        // The two identities name the SAME thing two ways — who is connecting — so a
        // request that supplies both states two answers to one question and is refused
        // with the modelled `InvalidRequestException` (HTTP 400) rather than one of them
        // being picked silently.
        if input.principal.is_some() && input.cognito_identity_pool_id.is_some() {
            return Err(invalid_request(CONFLICTING_IDENTITY_MESSAGE.to_string()));
        }

        // A request that names no identity at all asks about no device: there is nothing
        // to compute an authorization scope from, which is a bad request and not an empty
        // answer.
        if input.principal.is_none()
            && input.cognito_identity_pool_id.is_none()
            && input.thing_name.is_none()
        {
            return Err(invalid_request(MISSING_IDENTITY_MESSAGE.to_string()));
        }

        let store = state.lock();

        // The targets whose attachments govern this device, nearest first. A certificate
        // or a Cognito identity contributes itself; a thing group contributes itself and
        // every group above it.
        let mut scope: Vec<String> = Vec::new();

        if let Some(principal) = input.principal.as_deref() {
            // Resolved against this account's state, so the answer is only ever about a
            // resource that exists: a principal of no supported form is the modelled
            // `InvalidRequestException` (HTTP 400) and a well-formed one naming a resource
            // this account does not hold the modelled `ResourceNotFoundException`
            // (HTTP 404) — never an empty list standing in for "no such principal".
            let resolved = match store.resolve_policy_target(principal) {
                Ok(resolved) => resolved,
                Err(crate::state::PolicyTargetRefusal::Invalid(message)) => {
                    return Err(invalid_request(message));
                }
                Err(crate::state::PolicyTargetRefusal::NotFound(message)) => {
                    return Err(not_found(message));
                }
            };
            // Asked RECURSIVELY: a policy attached to a parent thing group has an effect
            // on the authorization of everything beneath it, and "effective" is exactly
            // the set that has an effect.
            scope.extend(store.policy_attachment_scope(&resolved, true));
        }

        if let Some(pool_id) = input.cognito_identity_pool_id.as_deref() {
            // An identity pool is named in the `<region>:<id>` form, and this service
            // holds no record of one — identity pools live in Amazon Cognito — so the id
            // is resolved no further than its form, exactly as a Cognito identity is
            // ([`crate::state::parse_policy_target`]). Anything else is not a pool id.
            match crate::state::parse_policy_target(pool_id) {
                Ok(crate::state::PolicyTarget::CognitoIdentity(identity)) => {
                    scope.push(crate::state::PolicyTarget::CognitoIdentity(identity).key());
                }
                _ => {
                    return Err(invalid_request(
                        crate::state::invalid_cognito_identity_pool_id_message(pool_id),
                    ));
                }
            }
        }

        if let Some(thing_name) = input.thing_name.as_deref() {
            // The device the question is about: a name no thing is registered under is
            // the modelled `ResourceNotFoundException` (HTTP 404), worded the way every
            // reader of one thing words the miss.
            if !store.things.contains_key(thing_name) {
                return Err(not_found(crate::state::missing_thing_message(thing_name)));
            }
        }

        // One entry per policy, whatever path brought it into scope: a policy attached
        // both to the principal and to a group above it has ONE effect, so it is reported
        // once, in one stable order — and a name the registry does not hold governs
        // nothing, so it is left out rather than reported with a document this account
        // never stored. Which policies the relation holds for a scope is ONE computation
        // ([`crate::state::ComAmazonawsIotAwsiotserviceState::policies_in_effect`]), shared
        // with the operation that decides an authorization question from it, so a reader of
        // the set and a decision made from it cannot disagree. This read overrides nothing:
        // the two name lists that operation's request may override the set with are empty
        // here.
        let effective_policies: Vec<EffectivePolicy> = store
            .policies_in_effect(&scope, &[], &[])
            .into_iter()
            .map(|policy| EffectivePolicy {
                policy_name: Some(policy.policy_name),
                policy_arn: Some(policy.policy_arn),
                // "The IAM policy document" of the version in effect — the same document
                // `GetPolicy` reports for this policy.
                policy_document: Some(policy.document),
            })
            .collect();

        Ok(GetEffectivePoliciesOutput {
            // `EffectivePolicies` carries no length constraint, so a device no policy is
            // attached for answers with an EMPTY list rather than an absent member:
            // "nothing governs it" is an answer, not a missing one.
            effective_policies: Some(effective_policies),
        })
    }
}

/// How this operation states that `principal` and `cognitoIdentityPoolId` were both
/// supplied.
///
/// FIXED text with no variable part: the refusal is about the PAIR of members and names
/// them, so neither caller-supplied value is echoed back.
const CONFLICTING_IDENTITY_MESSAGE: &str =
    "Principal must be null if CognitoIdentityPoolId is provided.";

/// How this operation states that the request named no identity and no device, so there
/// is nothing whose authorization could be computed. It names the members one of which
/// has to be supplied, for the same reason the sentence above names both of its.
const MISSING_IDENTITY_MESSAGE: &str =
    "Principal, CognitoIdentityPoolId or thingName must be provided.";

/// The modelled `InvalidRequestException` (HTTP 400), lifted into this operation's error
/// channel.
fn invalid_request(message: String) -> GetEffectivePoliciesError {
    GetEffectivePoliciesErrorModeled::InvalidRequestException(InvalidRequestException {
        message: Some(message),
    })
    .into()
}

/// The modelled `ResourceNotFoundException` (HTTP 404), lifted into this operation's error
/// channel.
fn not_found(message: String) -> GetEffectivePoliciesError {
    GetEffectivePoliciesErrorModeled::ResourceNotFoundException(ResourceNotFoundException {
        message: Some(message),
    })
    .into()
}
