// SPDX-License-Identifier: CC0-1.0

//! Where a refused request's AUTHORIZATION DETAILS live, and how a refusal points a caller
//! at them.
//!
//! A refusal IAM decides by evaluating the policies in force closes with a pointer to the
//! detail of that decision ([`pointer_clause`]) — unless it is one of the refusals whose
//! recordings carry no such pointer ([`DeniedRequest::points_at_its_detail`]) — naming an
//! authorization id this service mints for that one request ([`mint_authorization_id`]) —
//! "supported services include an
//! authorization ID in the access denied error message. Pass that ID to this operation to
//! retrieve the details" (`GetRequestAuthorizationDetails`). The id is therefore only half
//! of the pointer: the other half is a RECORD of what was decided, which this module writes
//! ([`record_denial`]) so that the id the message hands out resolves through the operation
//! the message names.
//!
//! That operation belongs to another service — IAM Toolbox,
//! `services/service-com-amazonaws-iamtoolbox-authrequestservice` — so the record is written
//! into the store THAT service owns and reads, through the seam it publishes
//! (`ComAmazonawsIamtoolboxAuthrequestserviceState::record_denial`), never into a second copy
//! of it here. The two services share one engine, so the handle to that store is taken where
//! this service's own store is taken ([`crate::state::service_state`]) and held beside it
//! ([`AuthorizationDetailsStore`]), which is what lets the gate — holding this service's
//! state and no engine — write the record of the refusal it just decided.
//!
//! What the record carries is what the decision was made FROM, and nothing invented: the
//! condition context the gate assembled, one evaluation for the `(action, resource)` pair the
//! call was decided on with the effect that decided it, one entry per policy document in
//! force, and the DOCUMENT of each of those policies the account holds — captured under the
//! same URI the entry publishes, so the read that resolves a URI
//! (`GetRequestAuthorizationPolicy`) answers the text the decision was made from. A member
//! the decision does not know — which statement matched, which requirement's check grouped
//! it — is left out rather than filled in.

use ::std::sync::atomic::{AtomicU64, Ordering};
use ::std::time::{SystemTime, UNIX_EPOCH};

use ::service_com_amazonaws_iamtoolbox_authrequestservice::ops::types::{
    AttachedTo, Evaluation, PolicyInfo,
};
use ::service_com_amazonaws_iamtoolbox_authrequestservice::state::{
    captured_policy_document, AuthorizationContext, AuthorizationDetailsRecord,
    ComAmazonawsIamtoolboxAuthrequestserviceState as AuthorizationDetailsState,
};

use crate::state::{
    ComAmazonawsIamAwsidentitymanagementv20100508State as IamState, PolicySource, name_key,
};

/// The Region the console address in a refusal's pointer names.
///
/// IAM is a GLOBAL service: its endpoint lives in one Region of the partition and a call to
/// it is answered the same wherever the caller addressed it from, so the console that shows
/// an authorization detail of the partition's IAM is that Region's console — not the Region
/// the refused request named, which the message carries nothing of.
const CONSOLE_REGION: &str = "us-east-1";

/// How many characters an authorization id is spelled in.
///
/// The id has to be unique per refused request and opaque to the caller: 25 base-36
/// characters carry the 128 bits [`mint_authorization_id`] draws, which is what makes it
/// neither guessable nor a visible counter. The modelled `authorizationId`
/// (`GetRequestAuthorizationDetailsInput$authorizationId`, pattern
/// `^[0123456789abcdefghijklmnopqrstuvwxyz]+$`) admits exactly this alphabet and publishes no
/// length, so the length is this service's own.
const AUTHORIZATION_ID_CHARACTERS: usize = 25;

/// The alphabet that pattern admits, in the order a radix spelling counts it.
const AUTHORIZATION_ID_ALPHABET: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";

/// The `EvaluatedEffect` value of a request a statement about it explicitly denied — one of
/// the three that enum declares (`ALLOW`, `EXPLICIT_DENY`, `IMPLICIT_DENY`).
const EXPLICIT_DENY_EFFECT: &str = "EXPLICIT_DENY";

/// The `EvaluatedEffect` value of a request nothing allowed and nothing explicitly denied:
/// "IMPLICIT_DENY - The action was denied because no policy allowed it".
const IMPLICIT_DENY_EFFECT: &str = "IMPLICIT_DENY";

/// The `PolicyType` value of an identity-based policy: "an identity-based policy attached to
/// an IAM user, group, or role".
const IDENTITY_BASED_POLICY: &str = "IDENTITY_BASED_POLICY";

/// The `PolicyType` value of a permissions boundary: "a permissions boundary for an IAM
/// entity".
const PERMISSIONS_BOUNDARY: &str = "PERMISSIONS_BOUNDARY";

/// The `PolicyType` value of a session policy: "a session policy passed during role
/// assumption or federation".
const SESSION_POLICY: &str = "SESSION_POLICY";

/// Counts the authorization ids this process has minted, so two refusals never share one.
static MINTED_AUTHORIZATION_IDS: AtomicU64 = AtomicU64::new(0);

/// The clause a refusal IAM decided on the policies in force CLOSES with: the console
/// address of the decision's authorization detail, and the API to read that detail from,
/// naming `authorization_id` in both.
///
/// Fixed text throughout except the id, which is the one thing about it drawn from the
/// request ([`mint_authorization_id`]). The ONE wording of the clause, so every refusal that
/// carries it carries the same one.
pub fn pointer_clause(authorization_id: &str) -> String {
    format!(
        ". Go to https://{CONSOLE_REGION}.console.aws.amazon.com/iam/home?region=\
         {CONSOLE_REGION}#/authorization-details/{authorization_id} for complete details, or \
         call the GetRequestAuthorizationDetails API with the following authorization id: \
         {authorization_id}"
    )
}

/// A fresh authorization id, for ONE refused request.
///
/// Unique: the low half is this process's refusal counter mixed through a bijection, so two
/// refusals cannot collide, and the high half mixes the clock in so two runs of the emulator
/// do not hand out the same ids. Nothing of the REQUEST goes into it — the id names the
/// record of a decision, not the decision — so a caller cannot read the request back out of
/// one, and the same call refused twice is pointed at two records.
pub fn mint_authorization_id() -> String {
    let sequence = MINTED_AUTHORIZATION_IDS.fetch_add(1, Ordering::Relaxed);
    let minted_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_nanos() as u64)
        .unwrap_or_default();
    let low = u128::from(mix(sequence));
    let high = u128::from(mix(sequence ^ minted_at));
    spell((high << 64) | low)
}

/// SplitMix64's finalizer: a bijection on `u64`, so distinct inputs stay distinct.
fn mix(value: u64) -> u64 {
    let mut mixed = value.wrapping_add(0x9E37_79B9_7F4A_7C15);
    mixed = (mixed ^ (mixed >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    mixed = (mixed ^ (mixed >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    mixed ^ (mixed >> 31)
}

/// `value` spelled in [`AUTHORIZATION_ID_ALPHABET`], exactly
/// [`AUTHORIZATION_ID_CHARACTERS`] characters long.
fn spell(value: u128) -> String {
    let radix = AUTHORIZATION_ID_ALPHABET.len() as u128;
    let mut rest = value;
    let mut id = vec![0u8; AUTHORIZATION_ID_CHARACTERS];
    for position in id.iter_mut().rev() {
        *position = AUTHORIZATION_ID_ALPHABET[(rest % radix) as usize];
        rest /= radix;
    }
    String::from_utf8(id).unwrap_or_default()
}

/// The store the authorization details of this account's refusals live in: the handle to the
/// state of the service that OWNS them, or `None` in a process that never reached this
/// service through its engine.
///
/// Held on this service's own state ([`IamState`]) because the gate that decides a refusal is
/// handed that state and nothing else. `Debug` is written by hand: the handle is a lock, which
/// a derive would have to format through.
#[derive(Default)]
pub struct AuthorizationDetailsStore(Option<::engine::ServiceState<AuthorizationDetailsState>>);

impl ::std::fmt::Debug for AuthorizationDetailsStore {
    fn fmt(&self, formatter: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        formatter
            .debug_struct("AuthorizationDetailsStore")
            .field("attached", &self.0.is_some())
            .finish()
    }
}

impl AuthorizationDetailsStore {
    /// Attach the owning service's store, once: every later touch is the same handle onto
    /// the same engine, so the first one is kept.
    pub fn attach(&mut self, store: ::engine::ServiceState<AuthorizationDetailsState>) {
        if self.0.is_none() {
            self.0 = Some(store);
        }
    }

    /// Write `record` into that store, or drop it where no store is attached: a refusal is
    /// still answered when its detail cannot be filed.
    fn write(&self, record: AuthorizationDetailsRecord) {
        if let Some(store) = &self.0 {
            store.lock().record_denial(record);
        }
    }
}

/// What the gate knows about the request it is refusing, which is what the record of the
/// refusal is built from — and what decides whether the refusal's message points at that
/// record at all ([`DeniedRequest::points_at_its_detail`]).
#[derive(Debug)]
pub struct DeniedRequest<'a> {
    /// The ARN of the principal the request speaks for, as the refusal names it.
    pub principal: &'a str,
    /// The IAM identity whose policies were evaluated: the principal itself for a long-term
    /// credential, and the ROLE for a session of it, whose policies such a credential has.
    pub identity: &'a str,
    /// The action the call was decided on.
    pub action: &'a str,
    /// The ARN the decision was made against.
    pub resource: &'a str,
    /// The condition context the decision was made in, as the gate assembled it.
    pub context: &'a [(String, String)],
    /// Whether a statement about the request said `Deny`, as against nothing allowing it.
    pub explicit_deny: bool,
    /// The ARN of the permissions boundary in force, where one was read.
    pub boundary: Option<&'a str>,
    /// How many session policy documents scoped the credential the request was signed with.
    pub session_policies: usize,
    /// Whether the credential the request was signed with is a role SESSION's temporary one,
    /// whose permissions are the role's capped by the session policies its mint passed — as
    /// against an identity's own long-term key.
    pub session_credential: bool,
    /// Whether the grant the decision could not find is the IDENTITY-based one: nothing in
    /// force allowed the call at all, as against a call the identity's own documents DID
    /// allow and the session policies or the boundary withheld.
    pub identity_grant_absent: bool,
    /// Whether one of the IDENTITY's own documents states an `Allow` covering the action
    /// asked for, so the decision was reached although a grant about that action was read —
    /// its `Condition` not holding at the instant of the call, or its `Resource` not covering
    /// the one addressed.
    pub identity_grant_names_the_action: bool,
    /// Whether the session policies the credential was scoped with allow the action WITHOUT
    /// any of them spelling it: a grant written about everything the session may do, of which
    /// the action asked for happens to be one, as against one written about that action.
    pub scope_allows_the_action_unnamed: bool,
}

impl DeniedRequest<'_> {
    /// Whether this refusal's message CLOSES with the pointer at the detail of its decision
    /// ([`pointer_clause`]), as against stating the decision in its sentence alone.
    ///
    /// No trait of any model governs the text of an `AccessDenied`, so which refusals carry
    /// the pointer is recorded behaviour, and the recordings draw the line through the
    /// refusal of a request signed with a role SESSION that no identity-based grant allowed:
    /// it is the sentence and nothing else when the decision was already explained by a grant
    /// that is not about this very action — one of the ROLE's own documents stating an
    /// `Allow` covering it, which was read and did not apply (its condition, its resource),
    /// or a session policy allowing it without spelling it, a scope written about everything
    /// the session may do. The pointer closes every other refusal this gate words: one of a
    /// long-term credential whichever document decided it, one no `Allow` about the action was
    /// in force for, one a statement about the request explicitly denied, one withheld by the
    /// boundary or by the session policies rather than by the identity's own documents, and
    /// one withheld by the identity's documents although a session policy SPELLS the action —
    /// a scope written about that action, which grants nothing of its own.
    #[must_use]
    pub fn points_at_its_detail(&self) -> bool {
        !(self.session_credential
            && self.identity_grant_absent
            && !self.explicit_deny
            && (self.identity_grant_names_the_action || self.scope_allows_the_action_unnamed))
    }
}

/// File the authorization details of a refused request, answering the authorization id they
/// are filed under — the id the refusal's pointer clause names.
///
/// A refusal is a decision, and this is the record of it: writing it adds nothing to what
/// any OTHER call reports, and it is the only thing that makes the pointer a caller is handed
/// resolve. The store it is written into belongs to the service that serves that read, so
/// nothing here reports a record that service would not return.
pub fn record_denial(store: &IamState, denied: &DeniedRequest<'_>) -> String {
    let authorization_id = mint_authorization_id();
    let context: AuthorizationContext = denied
        .context
        .iter()
        .map(|(key, value)| {
            (
                key.clone(),
                ::aws_smithy_types::Document::String(value.clone()),
            )
        })
        .collect();
    let evaluation = Evaluation {
        action: denied.action.to_string(),
        resource: denied.resource.to_string(),
        // The context of the request as a whole, which this evaluation adds nothing to:
        // one call was decided, so there is no per-evaluation context beside it.
        context: None,
        evaluated_effect: Some(
            if denied.explicit_deny {
                EXPLICIT_DENY_EFFECT
            } else {
                IMPLICIT_DENY_EFFECT
            }
            .to_string(),
        ),
        // Which statement matched is not part of the decision this gate reaches, so the
        // member is left out rather than filled with a policy that may not be the one.
        matched_policies: None,
        // The same holds for the checks that group matched policies by the requirement
        // they apply to: this gate decides the call, not which requirement's own policy
        // set matched it, so nothing is reported for them either.
        checks: None,
    };
    store.authorization_details.write(filed_record(
        authorization_id.clone(),
        context,
        evaluation,
        evaluated_policies(store, denied),
    ));
    authorization_id
}

/// The record filed for one refusal: its context, its one evaluation, the policies the
/// decision was made against, and the DOCUMENT of each of those policies this account
/// holds — captured under the same URI the policy is published under, which is the URI a
/// caller passes to `GetRequestAuthorizationPolicy` to read that document back. A policy
/// whose document this emulator does not hold (an AWS-managed ARN it carries no text for)
/// publishes its URI and captures nothing, so the read answers that URI the way the
/// operation answers a policy it has no document for.
fn filed_record(
    authorization_id: String,
    context: AuthorizationContext,
    evaluation: Evaluation,
    evaluated: EvaluatedPolicies,
) -> AuthorizationDetailsRecord {
    let mut record = AuthorizationDetailsRecord::new(
        authorization_id,
        context,
        vec![evaluation],
        evaluated.policies,
    );
    for (uri, document) in evaluated.documents {
        if let Some(document) = captured_policy_document(&document) {
            record = record.with_policy_document(uri, document);
        }
    }
    record
}

/// The policies one decision was made against, beside the document text of each policy
/// that is addressable by URI and whose text this account holds.
struct EvaluatedPolicies {
    /// What the record's `policies` list carries.
    policies: Vec<PolicyInfo>,
    /// `(policy URI, policy document text)`, one per policy a caller can read the
    /// document of back.
    documents: Vec<(String, String)>,
}

/// The policies the decision was made against: every identity-based document in force on the
/// identity, the permissions boundary that capped it where one was read, and the session
/// policies the credential was scoped with — each beside the policy document the account
/// holds for it, where the policy is one a caller can address by URI.
fn evaluated_policies(store: &IamState, denied: &DeniedRequest<'_>) -> EvaluatedPolicies {
    let mut documents: Vec<(String, String)> = Vec::new();
    let mut policies: Vec<PolicyInfo> = store
        .principal_sourced_policies(denied.identity)
        .unwrap_or_default()
        .iter()
        .map(|carried| {
            let policy = match &carried.source {
                // A managed policy is identified by its ARN ("for managed policies, this is
                // the policy ARN") and attached to the identity the gate read it from.
                PolicySource::Managed { policy_arn, .. } => PolicyInfo {
                    r#type: Some(IDENTITY_BASED_POLICY.to_string()),
                    inline: Some(false),
                    uri: Some(policy_arn.clone()),
                    attached_to: Some(vec![AttachedTo {
                        arn: Some(denied.identity.to_string()),
                    }]),
                    // `policySource` says what a policy was BUILT FROM, "such as a KMS key
                    // policy or grant", and is "absent for other policies" — which an IAM
                    // policy document is one of.
                    policy_source: None,
                },
                // An inline policy has no ARN, so it is identified by the entity it is
                // embedded in and its own name ("for inline policies, which have no ARN,
                // this is an opaque identifier") — and it is attached to that entity, which
                // for a policy inherited through a group is the GROUP and not the identity.
                PolicySource::Inline {
                    policy_name,
                    attachment_type,
                    attachment_name,
                } => PolicyInfo {
                    r#type: Some(IDENTITY_BASED_POLICY.to_string()),
                    inline: Some(true),
                    uri: Some(format!(
                        "policy://{attachment_type}/{attachment_name}/{policy_name}"
                    )),
                    attached_to: embedding_entity_arn(store, attachment_type, attachment_name)
                        .map(|arn| vec![AttachedTo { arn: Some(arn) }]),
                    policy_source: None,
                },
            };
            // The document in force is the one the decision was made FROM, so it is
            // captured under the URI the entry above publishes.
            if let (Some(uri), Some(document)) = (policy.uri.clone(), carried.document.clone()) {
                documents.push((uri, document));
            }
            policy
        })
        .collect();
    if let Some(boundary) = denied.boundary {
        policies.push(PolicyInfo {
            r#type: Some(PERMISSIONS_BOUNDARY.to_string()),
            inline: Some(false),
            uri: Some(boundary.to_string()),
            attached_to: Some(vec![AttachedTo {
                arn: Some(denied.identity.to_string()),
            }]),
            policy_source: None,
        });
        // The ceiling's document is read through the one definition of "the boundary in
        // force on this identity", so the captured text is the text that capped the call.
        if let Some(document) = store.permissions_boundary_document(denied.identity) {
            documents.push((boundary.to_string(), document));
        }
    }
    // A session policy is scoped to the CREDENTIAL the request was signed with, which is the
    // principal the refusal names rather than the identity whose policies it has.
    for _ in 0..denied.session_policies {
        policies.push(PolicyInfo {
            r#type: Some(SESSION_POLICY.to_string()),
            inline: None,
            uri: None,
            attached_to: Some(vec![AttachedTo {
                arn: Some(denied.principal.to_string()),
            }]),
            policy_source: None,
        });
    }
    EvaluatedPolicies {
        policies,
        documents,
    }
}

/// The ARN of the entity an inline policy is embedded in, read out of the account's own
/// records, or `None` for one the account no longer holds.
fn embedding_entity_arn(
    store: &IamState,
    attachment_type: &str,
    attachment_name: &str,
) -> Option<String> {
    let key = name_key(attachment_name);
    let user = || store.users.get(&key).map(|held| held.arn.clone());
    let group = || store.groups.get(&key).map(|held| held.arn.clone());
    let role = || store.roles.get(&key).map(|held| held.arn.clone());
    match attachment_type {
        "user" => user(),
        "group" => group(),
        "role" => role(),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RULE: an authorization id is spelled in the alphabet the modelled `authorizationId`
    /// admits, at a fixed length, and a fresh one is minted for every refusal.
    #[test]
    fn every_minted_authorization_id_is_new_and_within_the_modelled_alphabet() {
        let minted: Vec<String> = (0..64).map(|_| mint_authorization_id()).collect();
        for id in &minted {
            assert_eq!(
                id.chars().count(),
                AUTHORIZATION_ID_CHARACTERS,
                "case {id:?}: expected {AUTHORIZATION_ID_CHARACTERS} characters, got {}",
                id.chars().count()
            );
            assert!(
                id.bytes().all(|byte| AUTHORIZATION_ID_ALPHABET.contains(&byte)),
                "case {id:?}: expected only the alphabet the model admits, got {id:?}"
            );
        }
        let distinct: ::std::collections::BTreeSet<&String> = minted.iter().collect();
        assert_eq!(
            distinct.len(),
            minted.len(),
            "expected {} distinct ids, got {}",
            minted.len(),
            distinct.len()
        );
    }

    /// RULE: the pointer clause names the authorization id twice — once in the console
    /// address and once as the id to read the detail back with — and nothing of the request.
    #[test]
    fn the_pointer_clause_names_the_id_twice() {
        // Rows spanning the forms an id takes: one minted here, the shortest the alphabet
        // admits, and one of every character of that alphabet.
        let alphabet = String::from_utf8(AUTHORIZATION_ID_ALPHABET.to_vec())
            .expect("the alphabet is ASCII");
        for id in [mint_authorization_id(), "0".to_string(), alphabet] {
            let clause = pointer_clause(&id);
            assert_eq!(
                clause.matches(id.as_str()).count(),
                2,
                "case {id:?}: expected the clause to name the id twice, got {clause:?}"
            );
            assert!(
                clause.starts_with(". Go to https://")
                    && clause.contains(&format!("#/authorization-details/{id} for complete details")),
                "case {id:?}: expected the console address of the detail, got {clause:?}"
            );
            assert!(
                clause.ends_with(&format!(
                    "or call the GetRequestAuthorizationDetails API with the following \
                     authorization id: {id}"
                )),
                "case {id:?}: expected the clause to close with the API to read it from, got \
                 {clause:?}"
            );
        }
    }
}
