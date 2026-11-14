// SPDX-License-Identifier: CC0-1.0

//! Serving the RETRIEVAL of a secret this service is asked for by another AWS service, on a
//! ROLE's behalf.
//!
//! A compute service that exposes a secret to the workload it runs does not sign a request
//! of its own: it retrieves the value AS a role of the caller's account — an Amazon ECS task
//! execution role, say — before the workload exists. That retrieval is still this service's
//! to answer, because the two questions it turns on are facts about this service's own
//! resource:
//!
//! 1. **may this principal read it** — `secretsmanager:GetSecretValue` ("Retrieves the
//!    contents of the encrypted fields ... of the secret", `GetSecretValue`) on the secret
//!    the reference names, decided over the documents in force on the role and the secret's
//!    own resource-based policy, by the same shared evaluation this service's signed requests
//!    go through ([`super::authorization`]);
//! 2. **is there a value to read** — the secret exists in the Region its reference names and
//!    is not scheduled for deletion, since "if a secret is scheduled for deletion, then its
//!    details, including the encrypted secret value, is not accessible".
//!
//! WHICH VERSION a retrieval is answered with, and the value itself, are the retrieval's own
//! answer: a caller that may read the secret reads it through `GetSecretValue`, which selects
//! the version and opens it under the secret's key. Nothing here opens a value, so no KMS
//! call is made and no plaintext passes through this module.
//!
//! Nothing is withheld on evidence this emulator does not have. An engine carrying no IAM
//! store, a role that store holds no record for, and a role carrying an attachment whose
//! document this account does not hold (an entry of AWS's own catalog) each show nothing
//! about the role, so the retrieval proceeds — the same rule every other gate over a role's
//! documents in this tree keeps. A retrieval made with no principal at all is not decided
//! either: a caller that names no role states nothing for a policy to be read against.
//!
//! A READ throughout: the decision consults the IAM store and this one and writes to neither,
//! so deciding a retrieval changes nothing a later caller of either service sees — the access
//! stamp a real `GetSecretValue` records belongs to the retrieval that reads the value, not to
//! the decision whether it may.
//!
//! LOCK ORDER: the IAM store's lock is taken with no guard of this service's store held, and
//! each guard hands back owned values — the contract that keeps a lock cycle between the
//! stores unreachable. A caller reaches this with no lock of its own held.

use super::*;

use ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam_store;
use ::service_common::iam_authorization::{self as iam, Decision};

/// The action a retrieval of a secret's value takes, as IAM publishes it ("Actions defined
/// by AWS Secrets Manager", IAM Service Authorization Reference): the operation a caller
/// reads a value through.
pub const GET_SECRET_VALUE_ACTION: &str = "GetSecretValue";

/// Why this service did not serve a retrieval.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RetrievalRefusal {
    /// No policy in force on the principal permits the read: the action that was withheld,
    /// and the sentence AWS refuses it with.
    NotAuthorized {
        /// The fully qualified IAM action (`secretsmanager:<OperationName>`) that was
        /// withheld.
        action: String,
        /// The authorization refusal itself.
        message: String,
    },
    /// There is no value to read: this Region holds no such secret, or it holds one that is
    /// scheduled for deletion — carrying this service's own sentence about it.
    NotFound {
        /// Why nothing was retrieved, in this service's own words.
        message: String,
    },
}

/// The principal a retrieval is made as.
#[derive(Debug, Clone, Copy)]
pub struct RetrievalPrincipal<'a> {
    /// The IAM ROLE whose identity-based documents are in force on the retrieval, and which
    /// a resource-based policy grants the read by naming.
    pub role_arn: &'a str,
    /// What a refusal NAMES as the principal: the ARN of the SESSION the retrieving service
    /// assumed that role in, which is what AWS reports for a call made under one. `None`
    /// names the role itself, for a caller that holds no session of its own.
    pub session_arn: Option<&'a str>,
}

impl RetrievalPrincipal<'_> {
    /// The ARN a refusal names as the principal (see [`Self::session_arn`]).
    fn reported_arn(&self) -> &str {
        self.session_arn.unwrap_or(self.role_arn)
    }
}

/// Serve the retrieval of the secret `secret_id` names in `region`, made on behalf of
/// `principal`.
///
/// `Ok(())` for a retrieval this service would serve — and for every caller this gate does
/// not bound (see the module docs) — and the refusal otherwise, which the retrieving service
/// reports in its own vocabulary. The VALUE is not answered here: the caller reads it through
/// `GetSecretValue`, so what this answers is whether that read would be served at all, which
/// is what decides whether the workload the value is for can start.
pub fn serve_secret_retrieval<Eng: Send + Sync + 'static>(
    engine: &Eng,
    region: &str,
    secret_id: &str,
    principal: Option<RetrievalPrincipal<'_>>,
) -> Result<(), RetrievalRefusal> {
    if let Some(refusal) = withheld_retrieval(engine, region, secret_id, principal) {
        return Err(refusal);
    }
    let Some(state) = crate::state::service_state(engine) else {
        // No store of this service to ask: nothing is known about the secret, so the
        // retrieval is not refused on it.
        return Ok(());
    };
    let store = state.lock();
    let Some(view) = store.view(region, secret_id) else {
        return Err(RetrievalRefusal::NotFound {
            message: SECRET_NOT_FOUND_MESSAGE.to_string(),
        });
    };
    if view.secret.scheduled_for_deletion() {
        return Err(RetrievalRefusal::NotFound {
            message: MARKED_FOR_DELETION_MESSAGE.to_string(),
        });
    }
    Ok(())
}

/// The ARN the secret `secret_id` names is JUDGED against: the ARN of the secret this
/// account holds under that id, and — for an id it holds none under — the ARN that id
/// spells, itself when it is an ARN of this service already and the ARN a secret of that
/// name would carry in `region` otherwise.
///
/// The same reading [`super::authorization`] judges a signed request by, so one statement
/// covers a caller's own `GetSecretValue` and a retrieval made for it by another service.
fn judged_arn<Eng: Send + Sync + 'static>(engine: &Eng, region: &str, secret_id: &str) -> String {
    let held = crate::state::service_state(engine).and_then(|state| {
        let store = state.lock();
        store.get(region, secret_id).map(|secret| secret.arn.clone())
    });
    held.unwrap_or_else(|| {
        if ::service_common::arn::is_resource_arn_of(secret_id, authorization::ACTION_PREFIX) {
            secret_id.to_string()
        } else {
            authorization::secret_arn(region, secret_id)
        }
    })
}

/// The resource-based policy attached to the secret `secret_id` names, which is the other
/// way a retrieval is allowed: a policy naming the role grants the read with no
/// identity-based grant at all. `None` for a secret carrying none, and for one this account
/// does not hold.
fn attached_policy<Eng: Send + Sync + 'static>(
    engine: &Eng,
    region: &str,
    secret_id: &str,
) -> Option<String> {
    let state = crate::state::service_state(engine)?;
    let store = state.lock();
    store
        .view(region, secret_id)
        .and_then(|view| view.content.resource_policy.clone())
}

/// Why no policy in force on `principal` permits the retrieval, or `None` when one does —
/// or when nothing of the kind can be shown (see the module docs).
fn withheld_retrieval<Eng: Send + Sync + 'static>(
    engine: &Eng,
    region: &str,
    secret_id: &str,
    principal: Option<RetrievalPrincipal<'_>>,
) -> Option<RetrievalRefusal> {
    let principal = principal?;
    let role_arn = principal.role_arn;
    let iam = iam_store::service_state(engine)?;
    let (documents, boundary) = {
        let store = iam.lock();
        // A role this account's IAM holds no record for may belong to another account, so
        // its documents are not this store's to read.
        store.roles.values().find(|role| role.arn == role_arn)?;
        let policies = store.principal_policies(role_arn)?;
        // An attachment whose document this account does not hold — an entry of AWS's own
        // catalog — may be the very policy that carries the grant, so nothing is withheld
        // on it.
        if !policies.uncarried.is_empty() {
            return None;
        }
        (
            policies.documents,
            store.permissions_boundary_document(role_arn),
        )
    };
    let shared = attached_policy(engine, region, secret_id);
    let resource = judged_arn(engine, region, secret_id);
    let action = format!(
        "{}:{GET_SECRET_VALUE_ACTION}",
        authorization::ACTION_PREFIX
    );
    // Every spelling a resource-based policy may name this retrieval's principal by: the
    // session it is made in, the role whose permissions are in force, and the account each
    // belongs to — read the way every other service in this tree reads them.
    let mut principals = vec![role_arn.to_string()];
    if let Some(session_arn) = principal.session_arn {
        principals.push(session_arn.to_string());
    }
    principals.extend(iam::account_principals(role_arn));
    let request = iam::Request::new(&action, &resource, &principals);
    let identity = iam::identity_decision(&documents, &request);
    let stated = shared
        .as_deref()
        .map_or(Decision::NotStated, |document| {
            iam::resource_decision(document, &request)
        });
    // IAM's own evaluation order, the order this service's gate for a signed request takes
    // too ([`super::authorization`]), so one secret and one role are decided alike however
    // the retrieval is asked for.
    //
    // An explicit `Deny` in either kind of document wins; a grant in the SECRET's own policy
    // then stands on its own — "a permissions boundary does not limit the permissions granted
    // to the entity by a resource-based policy that names it as a principal" (IAM, permissions
    // boundaries) — and only an identity-based grant is capped by the boundary, which grants
    // nothing itself.
    let explicit_deny = identity == Decision::Denied || stated == Decision::Denied;
    let reason = if stated == Decision::Denied {
        authorization::RESOURCE_POLICY_EXPLICIT_DENY.to_string()
    } else if identity == Decision::Denied {
        iam::IDENTITY_POLICY_EXPLICIT_DENY.to_string()
    } else if stated == Decision::Allowed {
        return None;
    } else if identity != Decision::Allowed {
        iam::no_identity_policy_allows(&action)
    } else {
        match boundary.as_ref().map_or(Decision::Allowed, |document| {
            iam::identity_decision(::std::slice::from_ref(document), &request)
        }) {
            Decision::Allowed => return None,
            Decision::Denied => iam::PERMISSIONS_BOUNDARY_EXPLICIT_DENY.to_string(),
            Decision::NotStated => iam::no_permissions_boundary_allows(&action),
        }
    };
    // The refusal is the security-relevant event of this retrieval, so it is attributable in
    // the emulator's audit log the way this service's own gate logs a refused request: who
    // the retrieval was made as, what it asked for, and the outcome. Neither the policy
    // documents nor any value is logged.
    ::tracing::info!(
        target: "emulator::audit",
        audit = true,
        operation = "ServeSecretRetrieval",
        outcome = "refused_access_denied",
        principal = %principal.reported_arn(),
        action = %action,
        resource = %resource,
        explicit_deny = explicit_deny,
        "no policy in force permits this role to retrieve this secret"
    );
    Some(RetrievalRefusal::NotAuthorized {
        message: iam::not_authorized_by_identity_policy(
            principal.reported_arn(),
            &action,
            &resource,
            &reason,
        ),
        action,
    })
}
