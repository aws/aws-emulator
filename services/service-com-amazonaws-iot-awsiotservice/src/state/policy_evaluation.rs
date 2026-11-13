// SPDX-License-Identifier: CC0-1.0

//! How an authorization QUESTION about an IoT principal is decided: which policies are in
//! effect for it, and what one action on one resource comes out of them.
//!
//! An IoT policy document is an IAM-grammar JSON policy — that is what
//! [`super::validate_policy_document`] accepts — so the DECISION is the tree's one
//! evaluation of such a document against a request,
//! [`::service_common::iam_authorization`]: a statement is about the request only when its
//! action element names the action and its resource element matches the resource, an
//! explicit `Deny` among those wins over every `Allow`, and documents that state nothing
//! about the request neither grant nor refuse. Conditions, policy variables and the two
//! IAM wildcards are that module's to read, so an IoT decision and an IAM one cannot
//! disagree about the same document. Nothing of that rule is restated here.
//!
//! What IS IoT's own, and therefore lives here:
//!
//! * which policy action each `ActionType` asks about
//!   ([`iot_action_for_action_type`]) — the one place the enum and the IAM action names
//!   meet. The resource NAMESPACES follow from it with no rule of their own: a
//!   subscription is authorized against a `topicfilter/<filter>` ARN and a publication or
//!   a delivery against a `topic/<topic>` ARN, two different strings, so a grant written in
//!   one namespace never matches a question asked in the other;
//! * which policies are in effect for a principal
//!   ([`super::ComAmazonawsIotAwsiotserviceState::policies_in_effect`]): the attachment
//!   relation's, under the two name lists `TestAuthorization` can override them with;
//! * how a decision over SEVERAL resources, and the attribution of it to the policies it
//!   came from, becomes one `AuthResult` ([`evaluate_authorization`]).
//!
//! Deciding READS the policy registry and the attachment relation and records nothing, so
//! the same question asked twice of unchanged state answers the same way.

use std::collections::BTreeSet;

use ::service_common::iam_authorization::{
    identity_outcome_prepared, prepare_documents, Decision, Request,
};

/// The `AuthDecision` of a question a statement allows and none explicitly denies.
pub const AUTH_DECISION_ALLOWED: &str = "ALLOWED";

/// The `AuthDecision` of a question a statement explicitly denies, whatever else allows
/// it.
pub const AUTH_DECISION_EXPLICIT_DENY: &str = "EXPLICIT_DENY";

/// The `AuthDecision` of a question no statement in effect states anything about.
pub const AUTH_DECISION_IMPLICIT_DENY: &str = "IMPLICIT_DENY";

/// The condition key a policy variable names the MQTT client id of the connection being
/// authorized by: the value `TestAuthorization$clientId` ("The MQTT client ID") supplies.
pub const CLIENT_ID_CONTEXT_KEY: &str = "iot:ClientId";

/// How many resources ONE question may name.
///
/// `AuthInfo$resources` carries no modelled bound, while `authInfos` is modelled
/// `@length(min: 1, max: 10)`, so without this one request could ask for unbounded
/// evaluation work: the cost of a decision is the number of questions times the resources
/// each names times the statements in effect. This is the emulator's own ceiling on the
/// middle factor — the reason [`super::MAX_POLICIES_PER_TARGET`] exists for the relation —
/// and a question past it is refused with the modelled `LimitExceededException` (HTTP 410)
/// naming the limit rather than evaluated.
pub const MAX_AUTHORIZED_RESOURCES: usize = 10;

/// The policy action one `ActionType` asks about, or `None` when the value is not one of
/// the four the enum carries.
///
/// `ActionType` is "The type of action for which the principal is being authorized"
/// (PUBLISH | SUBSCRIBE | RECEIVE | CONNECT) and an IoT policy grants the action by its
/// IAM name, so this mapping is the one place the two namings meet.
pub fn iot_action_for_action_type(action_type: &str) -> Option<&'static str> {
    match action_type {
        "CONNECT" => Some("iot:Connect"),
        "PUBLISH" => Some("iot:Publish"),
        "RECEIVE" => Some("iot:Receive"),
        "SUBSCRIBE" => Some("iot:Subscribe"),
        _ => None,
    }
}

/// One policy whose statements decide a question: its identity, and the document of the
/// version in effect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyInEffect {
    /// The name the registry holds the policy under.
    pub policy_name: String,
    /// The ARN minted for it when it was created.
    pub policy_arn: String,
    /// The document of the version in effect — the one `GetPolicy` reports.
    pub document: String,
}

/// How a decision names one of the policies that produced it: the `Policy` shape's pair
/// of members, which is all a reader of a decision is told about a policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyReference {
    /// `Policy$policyName`.
    pub policy_name: String,
    /// `Policy$policyArn`.
    pub policy_arn: String,
}

/// The answer to one authorization question: the decision, and which policies produced
/// it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthorizationDecision {
    /// [`AUTH_DECISION_ALLOWED`], [`AUTH_DECISION_EXPLICIT_DENY`] or
    /// [`AUTH_DECISION_IMPLICIT_DENY`].
    pub decision: &'static str,
    /// The policies that allowed it — "A list of policies that allowed the
    /// authentication", so it is populated only when the decision IS allowed.
    pub allowed: Vec<PolicyReference>,
    /// "The policies that denied the authorization", populated only when a statement
    /// denied it.
    pub explicit_deny: Vec<PolicyReference>,
    /// "Policies that don't contain a matching allow or deny statement for the specified
    /// action on the specified resource", populated only when the decision is the implicit
    /// deny.
    pub implicit_deny: Vec<PolicyReference>,
    /// Every condition key a statement about the question needed a value for and the
    /// question did not carry, once each and in one stable order
    /// (`AuthResult$missingContextValues`).
    pub missing_context_values: Vec<String>,
}

/// Decide whether `policies` let the principal they are in effect for perform `action` on
/// every one of `resources`, with `context` as the condition context of the connection
/// being simulated.
///
/// The question is about the resources TOGETHER — `AuthInfo` pairs one `actionType` with
/// the list of "resources for which the principal is being authorized to perform the
/// specified action" and one `AuthResult` answers it — so it is allowed only when every
/// resource is, denied when a statement denies any of them, and allowed on none of them
/// when it names no resource at all.
///
/// The policies are attributed the way the IAM policy simulator attributes them: a
/// decision names the policies whose statements it was reached from, and the implicit deny
/// names the policies that stated nothing about the question.
pub fn evaluate_authorization(
    policies: &[PolicyInEffect],
    action: Option<&str>,
    resources: &[String],
    context: &[(String, String)],
) -> AuthorizationDecision {
    // Read once, then asked per resource: one statement walk per question instead of one
    // parse per question.
    let documents: Vec<String> = policies
        .iter()
        .map(|policy| policy.document.clone())
        .collect();
    let documents = prepare_documents(&documents);

    // Which policies denied, which allowed, and which stated anything at all, by their
    // position in `policies`.
    let mut denying: BTreeSet<usize> = BTreeSet::new();
    let mut allowing: BTreeSet<usize> = BTreeSet::new();
    let mut stating: BTreeSet<usize> = BTreeSet::new();
    let mut missing_context_values: BTreeSet<String> = BTreeSet::new();
    let mut allowed_resources = 0usize;

    // No `actionType` names no action, so no statement is about the question at all: there
    // is nothing a grant of an action could name, and every policy in effect states
    // nothing about it.
    let asked = action.into_iter().flat_map(|action| {
        resources
            .iter()
            .map(move |resource| (action, resource.as_str()))
    });
    for (action, resource) in asked {
        // A simulated authorization carries exactly the context its request supplied, so
        // the context is the WHOLE context: a key absent from it is one this question
        // carries no value for, which is what lets a condition over such a key be decided
        // instead of leaving every conditioned statement undecided.
        let request = Request::new(action, resource, &[])
            .with_context(context)
            .with_complete_context();
        let outcome = identity_outcome_prepared(&documents, &request);
        missing_context_values.extend(outcome.required_context_keys);
        // A decision is reached from the DENYING statements when it denies and from the
        // allowing ones when it allows, and from none at all when the documents state
        // nothing — so which side of the question each policy is on is read off the
        // statements this walk attributed the decision to.
        for matched in &outcome.matched {
            stating.insert(matched.document);
            if outcome.decision == Decision::Denied {
                denying.insert(matched.document);
            } else {
                allowing.insert(matched.document);
            }
        }
        if outcome.decision == Decision::Allowed {
            allowed_resources += 1;
        }
    }

    let every_resource_allowed = !resources.is_empty() && allowed_resources == resources.len();
    let decision = if !denying.is_empty() {
        AUTH_DECISION_EXPLICIT_DENY
    } else if every_resource_allowed {
        AUTH_DECISION_ALLOWED
    } else {
        AUTH_DECISION_IMPLICIT_DENY
    };

    let reference = |position: &usize| -> Option<PolicyReference> {
        policies.get(*position).map(|policy| PolicyReference {
            policy_name: policy.policy_name.clone(),
            policy_arn: policy.policy_arn.clone(),
        })
    };

    AuthorizationDecision {
        decision,
        allowed: if decision == AUTH_DECISION_ALLOWED {
            allowing.iter().filter_map(reference).collect()
        } else {
            Vec::new()
        },
        explicit_deny: denying.iter().filter_map(reference).collect(),
        implicit_deny: if decision == AUTH_DECISION_IMPLICIT_DENY {
            // The policies that contain no matching allow or deny statement for this
            // action on these resources: those none of the walks read a statement from.
            (0..policies.len())
                .filter(|position| !stating.contains(position))
                .collect::<Vec<usize>>()
                .iter()
                .filter_map(reference)
                .collect()
        } else {
            Vec::new()
        },
        missing_context_values: missing_context_values.into_iter().collect(),
    }
}

impl super::ComAmazonawsIotAwsiotserviceState {
    /// The policies whose statements decide a question asked about a principal: those the
    /// attachment relation holds for every key of `scope`, plus the names
    /// `treat_as_attached` and minus the names `treat_as_detached`.
    ///
    /// The two name lists are what `TestAuthorization` documents: `policyNamesToAdd` are
    /// "treated as if they are attached to the principal being authorized" and
    /// `policyNamesToSkip` "as if they are not", so a simulated decision can be asked
    /// about a policy set that is not the one the relation holds — without the relation
    /// being touched.
    ///
    /// Each policy is reported ONCE however many paths bring it into effect, with the
    /// document of the version in effect, which is the document `GetPolicy` reports. A
    /// name the registry does not hold decides nothing and is left out rather than
    /// carried without a document.
    pub fn policies_in_effect(
        &self,
        scope: &[String],
        treat_as_attached: &[String],
        treat_as_detached: &[String],
    ) -> Vec<PolicyInEffect> {
        let mut names: BTreeSet<String> = BTreeSet::new();
        for key in scope {
            names.extend(self.policy_attachments.policies_of(key));
        }
        names.extend(treat_as_attached.iter().cloned());
        for name in treat_as_detached {
            names.remove(name);
        }
        names
            .iter()
            .filter_map(|policy_name| self.policies.get(policy_name))
            .map(|policy| PolicyInEffect {
                policy_name: policy.policy_name.clone(),
                policy_arn: policy.policy_arn.clone(),
                document: policy.default_document().to_string(),
            })
            .collect()
    }
}
