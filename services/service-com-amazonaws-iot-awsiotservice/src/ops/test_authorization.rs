// SPDX-License-Identifier: CC0-1.0

//! @op TestAuthorization
//!
//! The authorization DECISION for one principal: "Tests if a specified principal is
//! authorized to perform an IoT action on a specified resource", computed from the
//! persisted policy-attachment relation ([`crate::state::policy_attachments`]) and the
//! policies' own documents ([`crate::state::policy_evaluation`]) — a pure read, so the
//! same question asked twice of unchanged state answers the same way.
//!
//! One `authInfo` is one question — an `actionType` and the resources the principal is
//! being authorized for — and one `AuthResult` is its answer, in the order the request
//! asked them, so a caller reads `authResults[i]` as the answer to `authInfos[i]`. Each
//! question is decided against the policies in EFFECT for the principal:
//!
//! * `principal` — a certificate ARN, a thing group ARN or a Cognito identity, resolved
//!   against this account's state the way every other reader of a policy target resolves
//!   one, so a decision is only ever about a resource that exists. A thing group's
//!   policies include those attached to the groups ABOVE it, because a policy attached to
//!   a parent group governs the groups beneath it.
//! * `cognitoIdentityPoolId` — the `<region>:<id>` form an identity pool is named by,
//!   resolved no further than its form: identity pools live in another service, and a pool
//!   and an identity are named in the same form, so both share one key space in the
//!   relation.
//! * `policyNamesToAdd` / `policyNamesToSkip` — policies "treated as if they are attached
//!   to the principal being authorized" and as if they are not, so a decision can be asked
//!   about a policy set the relation does not hold. Neither list is written anywhere: the
//!   relation after this call is the relation before it.
//!
//! The decision itself is the IAM grammar's, which is what an IoT policy document is
//! written in, so it is made by the tree's one evaluation of such a document against a
//! request ([`::service_common::iam_authorization`], reached through
//! [`crate::state::policy_evaluation`]): a statement is about the question only when it
//! names the action and matches the resource, an explicit `Deny` among those overrides
//! every `Allow`, and a question the documents state nothing about is an implicit deny.
//! What is IoT's own — which policy action each `ActionType` asks about, and why the
//! `topicfilter/` and `topic/` resource namespaces then decide a question on their own —
//! is [`crate::state::policy_evaluation`]'s to say.
//!
//! ---
//!
//! # Modeled contract for `TestAuthorization`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-iot-awsiotservice/smithy-model/com-amazonaws-iot-awsiotservice.json")
//! ```
//!
//! Tests if a specified principal is authorized to perform an IoT action on a specified
//! resource. Use this to test and debug the authorization behavior of devices that connect
//! to the IoT device gateway. Requires permission to access the TestAuthorization action.
//!
//! ## Input
//!
//! * `authInfos` (AuthInfos, len 1..10) **required** — A list of authorization info objects.
//!   Simulating authorization will create a response for each authInfo object in the list.
//! * `clientId` (ClientId) — The MQTT client ID.
//! * `cognitoIdentityPoolId` (CognitoIdentityPoolId) — The Cognito identity pool ID.
//! * `policyNamesToAdd` (PolicyNames) — When testing custom authorization, the policies
//!   specified here are treated as if they are attached to the principal being authorized.
//! * `policyNamesToSkip` (PolicyNames) — When testing custom authorization, the policies
//!   specified here are treated as if they are not attached to the principal being
//!   authorized.
//! * `principal` (Principal) — The principal. Valid principals are CertificateArn
//!   (arn:aws:iot:region:accountId:cert/certificateId) and CognitoId (region:id).
//!
//! ## Output
//!
//! * `authResults` (AuthResults) — The authentication results.
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
// `TestAuthorizationError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-iot-awsiotservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `TestAuthorizationError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.iot#TestAuthorization")]
#[derive(Debug)]
pub enum TestAuthorizationErrorModeled {
    InternalFailureException(InternalFailureException),
    InvalidRequestException(InvalidRequestException),
    LimitExceededException(LimitExceededException),
    ResourceNotFoundException(ResourceNotFoundException),
    ServiceUnavailableException(ServiceUnavailableException),
    ThrottlingException(ThrottlingException),
    UnauthorizedException(UnauthorizedException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::test_authorization::TestAuthorizationError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type TestAuthorizationError = crate::scaffolded::Scaffolded<TestAuthorizationErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<TestAuthorizationErrorModeled> for TestAuthorizationError {
    fn from(modeled: TestAuthorizationErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `TestAuthorization` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct TestAuthorization<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> TestAuthorization<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for TestAuthorization<Eng>
{
    type Input = TestAuthorizationInput;
    type Output = TestAuthorizationOutput;
    type Error = TestAuthorizationError;

    const NAME: &'static str = "TestAuthorization";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::TEST_AUTHORIZATION_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::TEST_AUTHORIZATION_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // Every policy a decision names carries the ARN minted when it was created, and
        // nothing here is minted, so the answer needs no request metadata of its own.
        let _ = ctx;

        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            TestAuthorizationErrorModeled::InternalFailureException(InternalFailureException {
                message: Some("the IoT emulator has no state store attached".to_string()),
            })
        })?;

        // Each question's action is resolved BEFORE the store is read: the answer is one
        // `AuthResult` per `authInfo`, so a request carrying a question this service
        // cannot read is refused rather than answered for its other questions. A value
        // outside the modelled enum is refused by the boundary validator before this
        // handler runs; this refusal is what keeps an unnamed action from silently
        // deciding nothing were one ever to reach here.
        let mut questions: Vec<Option<&'static str>> = Vec::with_capacity(input.auth_infos.len());
        for auth_info in &input.auth_infos {
            // How much evaluation one request may ask for is the number of questions times
            // the resources each names: `authInfos` is bounded by the model and this is the
            // emulator's own ceiling on the other factor, refused with the modelled
            // `LimitExceededException` (HTTP 410) naming the limit.
            if auth_info.resources.len() > crate::state::MAX_AUTHORIZED_RESOURCES {
                return Err(limit_exceeded(too_many_resources_message(
                    auth_info.resources.len(),
                )));
            }
            questions.push(match auth_info.action_type.as_deref() {
                None => None,
                Some(action_type) => {
                    Some(crate::state::iot_action_for_action_type(action_type).ok_or_else(
                        || {
                            crate::scaffolded::Scaffolded::validation(format!(
                                "Value '{action_type}' at 'authInfos.actionType' failed to \
                                 satisfy constraint: Member must satisfy enum value set: [{}]",
                                super::types::ACTION_TYPE_VALUES.join(", ")
                            ))
                        },
                    )?)
                }
            });
        }

        // The same reasoning bounds the policy set those questions are decided against:
        // the relation's own ceilings bound what an attachment contributes, and these two
        // lists are what a request may add to that set or take out of it by name.
        for (member, names) in [
            ("policyNamesToAdd", input.policy_names_to_add.as_ref()),
            ("policyNamesToSkip", input.policy_names_to_skip.as_ref()),
        ] {
            let named = names.map_or(0, Vec::len);
            if named > crate::state::MAX_POLICIES_PER_TARGET {
                return Err(limit_exceeded(too_many_policy_names_message(member, named)));
            }
        }

        let policies = {
            let store = state.lock();

            // The targets whose attached policies are in effect for the identity the
            // request names. A certificate or a Cognito identity contributes itself; a
            // thing group contributes itself and every group above it, because a policy
            // attached to a parent group governs the groups beneath it.
            let mut scope: Vec<String> = Vec::new();

            if let Some(principal) = input.principal.as_deref() {
                // Resolved against this account's state, so a decision is only ever about
                // a resource that exists: a principal of no supported form is the modelled
                // `InvalidRequestException` (HTTP 400) and a well-formed one naming a
                // resource this account does not hold the modelled
                // `ResourceNotFoundException` (HTTP 404) — never a decision computed from
                // an empty policy set standing in for "no such principal".
                let resolved = match store.resolve_policy_target(principal) {
                    Ok(resolved) => resolved,
                    Err(crate::state::PolicyTargetRefusal::Invalid(message)) => {
                        return Err(invalid_request(message));
                    }
                    Err(crate::state::PolicyTargetRefusal::NotFound(message)) => {
                        return Err(not_found(message));
                    }
                };
                scope.extend(store.policy_attachment_scope(&resolved, true));
            }

            if let Some(pool_id) = input.cognito_identity_pool_id.as_deref() {
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

            let treat_as_attached = input.policy_names_to_add.clone().unwrap_or_default();
            // A policy "treated as if attached" decides the question with its document, so
            // a name this account holds no policy under is the modelled
            // `ResourceNotFoundException` (HTTP 404) rather than a name silently deciding
            // nothing. `policyNamesToSkip` takes names OUT of the set, which needs no
            // document and is therefore not resolved against the registry.
            for policy_name in &treat_as_attached {
                if !store.policies.contains_key(policy_name) {
                    return Err(not_found(crate::state::missing_policy_message(policy_name)));
                }
            }
            let treat_as_detached = input.policy_names_to_skip.clone().unwrap_or_default();

            store.policies_in_effect(&scope, &treat_as_attached, &treat_as_detached)
        };

        // The context the connection being authorized is simulated with: `clientId` is
        // "The MQTT client ID", the value a policy variable and a condition name the
        // connecting client by. It is the WHOLE context of the simulated connection — a key
        // it does not carry is one the question carries no value for, which is what a
        // statement conditioned on that key is decided, and reported as missing, from.
        let context: Vec<(String, String)> = input
            .client_id
            .as_deref()
            .map(|client_id| {
                vec![(
                    crate::state::CLIENT_ID_CONTEXT_KEY.to_string(),
                    client_id.to_string(),
                )]
            })
            .unwrap_or_default();

        let auth_results = input
            .auth_infos
            .iter()
            .zip(questions)
            .map(|(auth_info, action)| {
                let decided = crate::state::evaluate_authorization(
                    &policies,
                    action,
                    &auth_info.resources,
                    &context,
                );
                AuthResult {
                    // "Authorization information": the question this result answers, so a
                    // caller reading one result knows what it decided.
                    auth_info: Some(auth_info.clone()),
                    // "A list of policies that allowed the authentication" — present
                    // whatever the decision, and empty unless the decision IS allowed:
                    // nothing allowed an authorization that was denied.
                    allowed: Some(Allowed {
                        policies: Some(reported_policies(decided.allowed.clone())),
                    }),
                    denied: denied(&decided),
                    auth_decision: Some(decided.decision.to_string()),
                    missing_context_values: Some(decided.missing_context_values),
                }
            })
            .collect();

        Ok(TestAuthorizationOutput {
            auth_results: Some(auth_results),
        })
    }
}

/// The `Denied` of a decision: the branch that denied it, and nothing else.
///
/// `Denied` carries `implicitDeny` ("Information that implicitly denies the authorization.
/// When a policy doesn't explicitly deny or allow an action on a resource it is considered
/// an implicit deny") and `explicitDeny` ("Information that explicitly denies the
/// authorization"), which are the two deny decisions — so an ALLOWED decision carries no
/// `denied` at all, and a denied one carries only the branch its decision names.
fn denied(decided: &crate::state::AuthorizationDecision) -> Option<Denied> {
    match decided.decision {
        crate::state::AUTH_DECISION_EXPLICIT_DENY => Some(Denied {
            implicit_deny: None,
            explicit_deny: Some(ExplicitDeny {
                policies: Some(reported_policies(decided.explicit_deny.clone())),
            }),
        }),
        crate::state::AUTH_DECISION_IMPLICIT_DENY => Some(Denied {
            implicit_deny: Some(ImplicitDeny {
                policies: Some(reported_policies(decided.implicit_deny.clone())),
            }),
            explicit_deny: None,
        }),
        _ => None,
    }
}

/// The policies a decision names, as the `Policy` shape reports one: the registry's own
/// `policyName` and the `policyArn` minted for it.
fn reported_policies(references: Vec<crate::state::PolicyReference>) -> Vec<Policy> {
    references
        .into_iter()
        .map(|reference| Policy {
            policy_name: Some(reference.policy_name),
            policy_arn: Some(reference.policy_arn),
        })
        .collect()
}

/// How this operation states that one question named more resources than a decision is
/// computed for. The fixed text names the member and the limit; the variable part is how
/// many the request named.
fn too_many_resources_message(named: usize) -> String {
    format!(
        "An authInfo may name at most {} resources; this one names {named}",
        crate::state::MAX_AUTHORIZED_RESOURCES
    )
}

/// How this operation states that one of the policy-name lists named more policies than a
/// decision is computed from, worded the way the sentence above is.
fn too_many_policy_names_message(member: &str, named: usize) -> String {
    format!(
        "{member} may name at most {} policies; this request names {named}",
        crate::state::MAX_POLICIES_PER_TARGET
    )
}

/// The modelled `LimitExceededException` (HTTP 410), lifted into this operation's error
/// channel.
fn limit_exceeded(message: String) -> TestAuthorizationError {
    TestAuthorizationErrorModeled::LimitExceededException(LimitExceededException {
        message: Some(message),
    })
    .into()
}

/// The modelled `InvalidRequestException` (HTTP 400), lifted into this operation's error
/// channel.
fn invalid_request(message: String) -> TestAuthorizationError {
    TestAuthorizationErrorModeled::InvalidRequestException(InvalidRequestException {
        message: Some(message),
    })
    .into()
}

/// The modelled `ResourceNotFoundException` (HTTP 404), lifted into this operation's error
/// channel.
fn not_found(message: String) -> TestAuthorizationError {
    TestAuthorizationErrorModeled::ResourceNotFoundException(ResourceNotFoundException {
        message: Some(message),
    })
    .into()
}
