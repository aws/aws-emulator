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
//!   by. Its FORM is read here, and whether the pool EXISTS is the holder's answer: Amazon
//!   Cognito Identity creates, describes and deletes identity pools, so a pool it does not
//!   hold is refused as missing ([`crate::state::holds_identity_pool`]). A pool and an
//!   identity are named in the SAME form, so both share one key space in the relation: a
//!   policy attached to `<region>:<id>` governs whoever is named by that string, which is
//!   what `AttachPolicy` records.
//! * `thingName` — the device the question is about. It must name a thing this account
//!   holds; the policies it adds are those of the relation a thing takes part in — the
//!   thing GROUPS it belongs to, with the groups above them, because a policy attached to a
//!   group governs the devices in it
//!   ([`crate::state::ComAmazonawsIotAwsiotserviceState::thing_membership_policy_scope`]) —
//!   so a thing in no group adds no policy of its own.
//!
//! ONE of the two identity members has to be supplied — `thingName` is context about the
//! device and not an identity, so it satisfies nothing — and the requirement is read
//! before any of the three is resolved. Which FORM the principal takes then decides
//! whether the pool id belongs beside it: a principal that is no ARN is the Cognito
//! identity form, which is named by the pool it belongs to, so the request has to carry
//! that pool id; an ARN names its resource by itself, so a pool id beside it states a
//! second identity for one question. Each of the three refusals is this operation's own
//! fixed sentence ([`MISSING_IDENTITY_MESSAGE`], [`COGNITO_IDENTITY_POOL_REQUIRED_MESSAGE`],
//! [`CONFLICTING_IDENTITY_MESSAGE`]).
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

        // An IDENTITY is what this operation answers about, so one of the two identity
        // members is required and the requirement is read FIRST — before the principal is
        // resolved and before the `thingName` is looked up, because a thing name is
        // context about the device rather than an identity and satisfies nothing. A
        // request that names neither identity is the modelled `InvalidRequestException`
        // (HTTP 400) whatever else it carries: never a device's policy list, and never a
        // miss of the thing it happened to name.
        if input.principal.is_none() && input.cognito_identity_pool_id.is_none() {
            return Err(invalid_request(MISSING_IDENTITY_MESSAGE.to_string()));
        }

        // Which FORM the principal takes decides whether the pool id belongs beside it.
        // The member documents three forms — a certificate ARN, a thing group ARN and a
        // Cognito identity — and a value that is no ARN at all is the Cognito identity
        // one: an identity is named by the POOL it belongs to, which this service holds no
        // record of, so such a principal is answerable only with its
        // `cognitoIdentityPoolId` beside it. An ARN names its resource by itself, so a
        // pool id beside one states two identities for one question and is refused rather
        // than one of them being picked silently. Both refusals are the modelled
        // `InvalidRequestException` (HTTP 400).
        if let Some(principal) = input.principal.as_deref() {
            if crate::state::names_an_arn(principal) {
                if input.cognito_identity_pool_id.is_some() {
                    return Err(invalid_request(CONFLICTING_IDENTITY_MESSAGE.to_string()));
                }
            } else if input.cognito_identity_pool_id.is_none() {
                return Err(invalid_request(
                    COGNITO_IDENTITY_POOL_REQUIRED_MESSAGE.to_string(),
                ));
            }
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
            // An identity pool is named in the `<region>:<id>` form, and this service holds
            // no record of one — identity pools live in Amazon Cognito — so the FORM is read
            // here ([`crate::state::parse_policy_target`]) and anything else is not a pool
            // id at all.
            match crate::state::parse_policy_target(pool_id) {
                Ok(crate::state::PolicyTarget::CognitoIdentity(identity)) => {
                    // ... and whether the pool EXISTS is the holder's answer: Amazon Cognito
                    // Identity is the service that creates, describes and deletes identity
                    // pools, so a pool it does not hold is a pool there is none of, and the
                    // question asked about it is the modelled `ResourceNotFoundException`
                    // (HTTP 404) in the holder's own wording — never an authorization answer
                    // about an identity nothing could be federated into.
                    if !crate::state::holds_identity_pool(&*self.engine, pool_id) {
                        return Err(not_found(crate::state::missing_identity_pool_message(
                            pool_id,
                        )));
                    }
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
            // the modelled `ResourceNotFoundException` (HTTP 404), stated in this
            // operation's own wording ([`thing_miss_message`]).
            if !store.things.contains_key(thing_name) {
                return Err(not_found(thing_miss_message(thing_name)));
            }
            // A thing is CONTEXT, so what it adds is the policies of the relation it takes
            // part in: a policy attached to a thing group has an effect on the authorization
            // of the devices in that group, and one attached to a group above it governs the
            // groups beneath, so the thing's groups come into scope WITH their ancestors —
            // the same scope `ListAttachedPolicies` of one of those groups reads recursively
            // ([`crate::state::ComAmazonawsIotAwsiotserviceState::thing_membership_policy_scope`]).
            scope.extend(store.thing_membership_policy_scope(thing_name));
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

/// How this operation states that an ARN `principal` was supplied with a
/// `cognitoIdentityPoolId` beside it.
///
/// FIXED text with no variable part: the refusal is about the PAIR of members and names
/// them, so neither caller-supplied value is echoed back.
const CONFLICTING_IDENTITY_MESSAGE: &str =
    "Principal must be null if CognitoIdentityPoolId is provided.";

/// How this operation states that NEITHER identity member was supplied, so there is
/// nothing whose authorization could be computed. It names the two members, for the same
/// reason the sentence above names both of its, and a `thingName` the request may also
/// carry is not among them because a thing name is not an identity.
const MISSING_IDENTITY_MESSAGE: &str = "Both principal and CognitoIdentityPoolId can not be null.";

/// How this operation states that the `principal` is the Cognito identity form — a value
/// that is no ARN — and the pool that names it was not supplied.
///
/// FIXED text naming the member that is missing and the form that requires it; the
/// offending principal is NOT echoed, for the same reason the two sentences above echo
/// nothing.
const COGNITO_IDENTITY_POOL_REQUIRED_MESSAGE: &str =
    "CognitoIdentityPoolId must be provided if your principal is CognitoIdentityId.";

/// How this operation states that `thingName` names no thing this account holds.
///
/// The device is addressed by the NAME the request sent, so that name is the sentence's
/// one variable part and nothing else of the request or of this account's state — no
/// account id, no prefix — is part of it. Every other reader of one thing words its own
/// miss ([`crate::state::missing_thing_message`]); this is the wording of the operation
/// that resolves a thing as context for an authorization question.
fn thing_miss_message(thing_name: &str) -> String {
    format!("{thing_name} thing doesn't exists")
}

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
