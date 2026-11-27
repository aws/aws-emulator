// SPDX-License-Identifier: CC0-1.0

//! Whether IAM lets a request to this service perform the action it asks for — the
//! authorization step taken BEFORE anything of the request's own input is read and before
//! any plan state is read or written.
//!
//! AWS decides authorization at the IAM layer, ahead of the service looking at the
//! request's own input and at the account's own records, so a caller whose identity-based
//! policies do not grant the action is told it is not authorized rather than told anything
//! about the data it asked for OR about the request it sent. That ordering is structural
//! here: the decision is taken by the operation wrapper every modelled operation of this
//! service is served through ([`crate::service`]), in front of the constraint walk and the
//! handler alike, so no later layer can answer a caller the gate refuses — see
//! [`refusal`] and [`refusal_response`].
//!
//! The operations that answer such a caller are exactly the ones whose model declares
//! `AccessDeniedException` ("You don't have sufficient access to perform this action", HTTP
//! 403 — `smithy_query(query="digest")` lists it for `GetAccountPlanState` and
//! `UpgradeAccountPlan`); an operation whose model declares no 403 is NOT judged here,
//! because answering a caller with an error its operation does not model would be a
//! different divergence.
//!
//! Three pieces, each held in exactly one place — the shape every other gate in this tree
//! takes, because this is one rule and not one per service:
//!
//! 1. **who** — the principal the request's access key id speaks for: the session STS
//!    minted that key for ([`sts_store::resolve_caller`], the same place
//!    `sts:GetCallerIdentity` answers from, so the principal a refusal blames is the one
//!    the caller reads back for the credentials it signed with), else the IAM user that
//!    owns the long-term key, else the account's own published credential;
//! 2. **what it may do** — the identity-based policy documents the IAM store holds for
//!    that principal and the permissions boundary that caps them, read through the IAM
//!    service's OWN accessors (`principal_policies`, `permissions_boundary_document`),
//!    where the collection rule itself lives — so a policy `iam:PutUserPolicy` wrote takes
//!    effect on the next request and deleting it lifts the grant again;
//! 3. **the decision, and the sentence it is stated in** —
//!    [`::service_common::iam_authorization`], the ONE shared evaluation of those documents
//!    against `(action, resource)`, so this service and every other one handed the same
//!    documents answer alike: an explicit `Deny` wins, otherwise an `Allow` about the
//!    request grants it, and with neither the request falls to IAM's default deny. The
//!    WORDS of a refusal come from that same module
//!    ([`iam::not_authorized_by_identity_policy`] and the reason clauses beside it), so no
//!    part of IAM's wording is spelled in this crate.
//!
//! Only the store LOOKUPS live here; neither the decision nor the ARN readings do. The
//! lookups stay in this service crate for the reason every other gate keeps theirs there:
//! hoisting them into `service-common` would make every service that depends on that crate
//! link the IAM and STS service crates. WHICH IAM entity a principal ARN names carries no
//! such dependency, so it is read through the shared [`::service_common::iam_role`].
//!
//! Free Tier publishes no resource-based policy of its own — the model declares no policy
//! operation and no policy member — so the decision here is the identity-based one alone.
//!
//! ## What the decision is taken about
//!
//! The ACTION is `freetier:<OperationName>` ([`ACTION_PREFIX`], this service's
//! `sigv4Name`/`endpointPrefix`, `smithy_query(query="service_info")`), and the RESOURCE is
//! the operation's own API resource ([`api_resource`]): Free Tier publishes no resource
//! type of its own, so the resource an action is judged against is the API path the
//! request named, under this service's one Region ([`crate::state::SERVICE_REGION`]).
//!
//! ## The callers this gate does not bound
//!
//! * **The account's own published credential.** A request whose access key id neither STS
//!   nor IAM holds a record for resolves to the account's own identity, which is not a
//!   principal the IAM store holds policies for — the position AWS account root
//!   credentials are in. Such a request is allowed.
//! * **A principal holding an attachment this emulator cannot read.** An AWS-managed
//!   policy belongs to AWS rather than to this account, so the IAM store holds no document
//!   for it; refusing on a grant this emulator cannot see would deny access AWS permits.
//!   An explicit `Deny` in a document that CAN be read still refuses, and a permissions
//!   boundary the store DOES hold still caps the grant.
//! * **An engine carrying no STS or IAM store**, where no session and no user can exist.
//!
//! An expired session credential speaks for nobody: it carries no documents and no
//! principal spelling, so every action it asks for falls to the default deny. (In the
//! assembled emulator the layer that authenticates a request refuses such a credential
//! before any handler runs.)

use std::time::SystemTime;

use ::aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam_store;
use ::service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts_store;
use ::service_common::iam_authorization::{self as iam, Decision};
use ::smithy_server_runtime::Service;

use crate::ops::types::{AccessDeniedException, SCHEMA_ACCESS_DENIED_EXCEPTION};

/// This service's IAM action prefix: an action is `freetier:<OperationName>` ("Actions
/// defined by AWS Free Tier", IAM Service Authorization Reference), which is this
/// service's `sigv4Name` and `endpointPrefix`
/// (`smithy_query(query="service_info")`) — the same string every ARN of this service
/// spells.
pub const ACTION_PREFIX: &str = "freetier";

/// The wire error code the modelled refusal carries.
const ACCESS_DENIED_EXCEPTION: &str = "AccessDeniedException";

/// The status that shape is answered at: `httpError: 403`
/// (`smithy_query(query="shape",shape="AccessDeniedException")`).
const ACCESS_DENIED_STATUS: u16 = 403;

/// The operations this gate judges: the ones whose model declares
/// `AccessDeniedException`, which `smithy_query(query="digest")` lists as
/// `GetAccountPlanState` and `UpgradeAccountPlan`.
///
/// An operation outside this list models no 403, so it is not judged: answering a caller
/// with an error its operation does not model would be a different divergence. The list is
/// pinned to the model by this module's own test, so an operation whose model gains the
/// shape is judged instead of silently staying ungated.
pub const JUDGED_OPERATIONS: &[&str] = &["GetAccountPlanState", "UpgradeAccountPlan"];

/// Whether `operation`'s model declares the refusal this gate answers with
/// ([`JUDGED_OPERATIONS`]).
pub fn judges(operation: &str) -> bool {
    JUDGED_OPERATIONS.contains(&operation)
}

/// The modelled refusal this request is answered with, or `None` when there is none to
/// answer: IAM permits the call, or `operation` is one this gate does not judge.
///
/// This is what the operation wrapper asks BEFORE it reads a single value of the request's
/// own input ([`crate::service`]), because that is the order AWS takes the two steps in: a
/// caller no identity-based policy allows is refused the action it named, and is told
/// nothing about the request it sent — not even that it broke a modelled constraint.
pub fn refusal<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    operation: &str,
) -> Option<AccessDeniedException> {
    if !judges(operation) {
        return None;
    }
    authorize(engine, ctx, operation).err()
}

/// The wire answer to a request this gate refused: the modelled `AccessDeniedException` at
/// its modelled status, serialized through the SAME protocol an error a handler returns
/// goes through (`Service::override_protocol`, falling back to the model-derived codec), so
/// a refusal taken in front of the request's input and one taken inside a handler are
/// indistinguishable on the wire.
///
/// A value the codec cannot render is the one case the serialization can fail; it falls to
/// the service's own framework renderer rather than to a second hand-built envelope.
pub fn refusal_response(
    svc: &dyn Service,
    req: &RtRequest,
    denial: &AccessDeniedException,
) -> RtResponse {
    let protocol = svc
        .override_protocol()
        .unwrap_or_else(|| svc.definition().protocol());
    protocol
        .serialize_error(
            denial,
            &SCHEMA_ACCESS_DENIED_EXCEPTION,
            ACCESS_DENIED_EXCEPTION,
            ACCESS_DENIED_STATUS,
        )
        .unwrap_or_else(|error| svc.definition().framework_error_response(error, req))
}

/// What a refusal names as the principal of a request whose session credential has already
/// expired: that credential speaks for nobody, and the key it presented is not quoted back.
const EXPIRED_CREDENTIAL_PRINCIPAL: &str = "an expired session credential";

/// The IAM action `operation` is authorized under: `freetier:<OperationName>`.
pub fn action_of(operation: &str) -> String {
    format!("{ACTION_PREFIX}:{operation}")
}

/// The resource an authorization decision about `operation` is taken against:
/// `arn:aws:freetier:<region>:<account>:/<OperationName>`.
///
/// Free Tier publishes no resource type of its own — the model declares no ARN member and
/// no resource shape — so what an action is judged against is the API path the request
/// named, in this service's one Region ([`crate::state::SERVICE_REGION`]). Built through
/// the account contract's own ARN builder, so the partition and the account segment are
/// the ones every other ARN this emulator emits carries.
pub fn api_resource(operation: &str) -> String {
    ::engine::emulator_arn(
        ACTION_PREFIX,
        crate::state::SERVICE_REGION,
        &format!("/{operation}"),
    )
}

/// Authorize this request, where `operation` is the operation's own name
/// ([`smithy_server_runtime::Operation::NAME`]).
///
/// `Ok(())` for a request IAM permits (and for every caller this gate does not bound, see
/// the module docs); the modelled refusal otherwise, answered as the operation's own
/// `AccessDeniedException` ([`refusal_response`]). It is taken through [`refusal`], in
/// front of the request's own input and therefore before the store is read or written, so
/// a refused call reports nothing about the request it was sent and nothing about the
/// account's plan data, and changes nothing.
///
/// Resolving the caller goes through STS's own `resolve_caller`, which first materialises
/// the session expiries the clock has already made due; that write is owed at the same
/// instant whether or not anybody called, which is why a REPORTING operation of this
/// service may take this decision too.
pub fn authorize<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    operation: &str,
) -> Result<(), AccessDeniedException> {
    let action = action_of(operation);
    let resource = api_resource(operation);
    let caller = caller(engine, ctx);
    // The principal spellings travel with the request because that is what a statement's
    // `Principal` element would be matched against; an identity-based document states no
    // such element, and Free Tier publishes no resource-based policy (see the module
    // docs), so the decision below rests on the action and the resource.
    let request = iam::Request::new(&action, &resource, &caller.principals);
    let identity_said = iam::identity_decision(&caller.documents, &request);
    // The decision is taken FIRST, so an explicit `Deny` in a document that CAN be read
    // refuses the request whatever else the principal carries — including a grant this
    // emulator cannot read, which only lifts the DEFAULT deny of a request no readable
    // statement allows (see the module docs).
    if identity_said == Decision::Denied {
        return Err(not_authorized(
            &caller.arn,
            &action,
            &resource,
            iam::IDENTITY_POLICY_EXPLICIT_DENY,
        ));
    }
    let granted =
        iam::is_allowed(identity_said, Decision::NotStated) || caller.identity_is_unbounded();
    if !granted {
        return Err(not_authorized(
            &caller.arn,
            &action,
            &resource,
            &iam::no_identity_policy_allows(&action),
        ));
    }
    // "A permissions boundary sets the maximum permissions that an identity-based policy
    // can grant" (IAM, permissions boundaries): a grant the boundary does not also allow
    // is not a permission the principal has. The account's own credential carries none.
    if let Some(boundary) = &caller.boundary {
        let reason = match iam::identity_decision(::std::slice::from_ref(boundary), &request) {
            Decision::Allowed => return Ok(()),
            // An explicit deny is a fact about the boundary document, so the refusal names
            // it; a boundary that merely states nothing withheld the grant the
            // identity-based policies gave.
            Decision::Denied => iam::PERMISSIONS_BOUNDARY_EXPLICIT_DENY.to_string(),
            Decision::NotStated => iam::no_permissions_boundary_allows(&action),
        };
        return Err(not_authorized(&caller.arn, &action, &resource, &reason));
    }
    Ok(())
}

/// The refusal IAM states an authorization failure with: who was refused, which action, on
/// which resource, and which of IAM's rules refused it. Every value in it comes from the
/// request and from the stored policies — the principal from the credential the request
/// presented, the action and the resource from the operation it named.
///
/// The sentence itself is the shared one every gate in this tree words this refusal with
/// ([`iam::not_authorized_by_identity_policy`], whose own reason clauses `authorize` reaches
/// for), so a correction to IAM's wording reaches this service instead of being copied into
/// it.
///
/// The sentence stays inside the modelled bound of `AccessDeniedException$message` (a
/// `GenericString` of `@length(0, 1024)`, `smithy_query(query="shape",
/// shape="AccessDeniedException")`) by construction: its only variable part is the
/// principal's ARN, which IAM itself bounds (a `pathType` of at most 512 characters and a
/// name of at most 64), and the resource is this service's own.
fn not_authorized(
    principal: &str,
    action: &str,
    resource: &str,
    reason: &str,
) -> AccessDeniedException {
    AccessDeniedException {
        message: iam::not_authorized_by_identity_policy(principal, action, resource, reason),
    }
}

/// The principal a request speaks for, and everything the decision is made from.
#[derive(Debug, Clone)]
struct Caller {
    /// The principal's ARN, as `sts:GetCallerIdentity` reports it — what a refusal names.
    arn: String,
    /// Every spelling of the principal: the session ARN STS reports, and the ARN of the
    /// IAM role behind an assumed-role session.
    principals: Vec<String>,
    /// The identity-based policy documents in force on the principal.
    documents: Vec<String>,
    /// The permissions boundary in force on the principal, when it carries one whose
    /// document this emulator holds.
    boundary: Option<String>,
    /// Set when this request presents the account's own published credential, which holds
    /// no IAM policies (see the module docs).
    account_credential: bool,
    /// Set when the principal has a policy attached whose document the IAM store does not
    /// hold — an AWS-managed policy (see the module docs).
    unreadable_attachment: bool,
}

impl Caller {
    /// Whether nothing the identity-based decision found can refuse this caller once no
    /// readable document DENIES the action: the account's own credential, which holds no
    /// IAM policies, or a principal carrying a grant this emulator cannot read.
    fn identity_is_unbounded(&self) -> bool {
        self.account_credential || self.unreadable_attachment
    }

    /// Read what the IAM identity `identity_arn` names may do, through the IAM service's
    /// own accessors: the documents in force on it, whether it carries an attachment whose
    /// document this emulator cannot read, and the permissions boundary that caps it.
    fn read_policies_of(
        &mut self,
        store: &::service_com_amazonaws_iam_awsidentitymanagementv20100508::ComAmazonawsIamAwsidentitymanagementv20100508State,
        identity_arn: &str,
    ) {
        if let Some(policies) = store.principal_policies(identity_arn) {
            self.documents = policies.documents;
            // An attachment the collection resolved no document for is an entry of AWS's
            // own catalog this emulator does not carry: a grant it cannot read.
            self.unreadable_attachment = !policies.uncarried.is_empty();
        }
        self.boundary = store.permissions_boundary_document(identity_arn);
    }
}

/// The principal this request speaks for, with the identity-based policies in force on it.
fn caller<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
) -> Caller {
    let access_key_id = ::engine::extract_akid(&ctx.parts().headers);
    let identity = match sts_store::service_state(engine) {
        Some(sts) => sts_store::resolve_caller(
            &mut sts.lock(),
            &access_key_id,
            SystemTime::now(),
            ctx.credential_store(),
        ),
        // No STS store on this engine: the only identity a request can present is the
        // account's own published credential.
        None => Ok(sts_store::ambient_identity()),
    };
    let identity = match identity {
        Ok(identity) => identity,
        // The session credential has expired, so it speaks for nobody: no documents, no
        // principal spelling, and every action it asks for falls to the default deny. The
        // key id it presented is NOT quoted back — this module echoes no value of the
        // request.
        Err(expired) => {
            let _ = expired;
            return Caller {
                arn: EXPIRED_CREDENTIAL_PRINCIPAL.to_string(),
                principals: Vec::new(),
                documents: Vec::new(),
                boundary: None,
                account_credential: false,
                unreadable_attachment: false,
            };
        }
    };

    let mut caller = Caller {
        account_credential: identity.arn == sts_store::ambient_identity().arn,
        principals: vec![identity.arn.clone()],
        arn: identity.arn,
        documents: Vec::new(),
        boundary: None,
        unreadable_attachment: false,
    };
    let Some(iam) = iam_store::service_state(engine) else {
        return caller;
    };
    let store = iam.lock();
    // A LONG-TERM access key: STS's store holds SESSIONS, so its answer for a key it never
    // minted is the account's ambient identity — but IAM's store maps an access key id to
    // the user that owns it (`access_keys`, the map IAM's own `GetUser` resolves a caller
    // through), so a request signed with one speaks for THAT user and is judged on the
    // user's policies rather than served as the account. A key the IAM store does not hold
    // either really is the account's own published credential.
    if caller.account_credential {
        if let Some(user) = store
            .access_keys
            .get(&access_key_id)
            .and_then(|record| store.users.get(&record.user_key))
        {
            caller.account_credential = false;
            caller.arn = user.arn.clone();
            caller.principals = vec![user.arn.clone()];
        }
    }
    if let Some(role_name) = ::service_common::iam_role::assumed_role_session_role_name(&caller.arn)
    {
        // An assumed-role session's permissions are the ROLE's, and the role is addressed
        // by the ARN the account holds it under — which carries its PATH, where the
        // session ARN does not.
        if let Some(held) = store.roles.get(&iam_store::name_key(role_name)) {
            let role_arn = held.arn.clone();
            caller.principals.push(role_arn.clone());
            caller.read_policies_of(&store, &role_arn);
        }
    } else if ::service_common::iam_role::user_name(&caller.arn).is_some() {
        // An IAM user's permissions are its own, under the ARN it was resolved by.
        let user_arn = caller.arn.clone();
        caller.read_policies_of(&store, &user_arn);
    }
    caller
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The decision one stored document reaches about an action on a resource, through the
    /// shared evaluator this gate consults.
    fn decision(document: &str, action: &str, resource: &str) -> Decision {
        iam::identity_decision(
            &[document.to_string()],
            &iam::Request::new(action, resource, &[]),
        )
    }

    /// RULE: the action this gate asks about is `freetier:<OperationName>` and the
    /// resource is that operation's own API path, so a policy scoped to this service's
    /// actions covers every operation of it and nothing outside it.
    #[test]
    fn the_action_asked_about_carries_this_services_prefix() {
        let document = r#"{"Statement":[{"Effect":"Allow","Action":"freetier:*","Resource":"*"}]}"#;
        for operation in ["GetAccountPlanState", "UpgradeAccountPlan"] {
            let action = action_of(operation);
            let reached = decision(document, &action, &api_resource(operation));
            assert_eq!(
                reached,
                Decision::Allowed,
                "case {operation:?}: expected freetier:* to allow {action}, got {reached:?}"
            );
        }
        let outside = decision(document, "s3:GetObject", "*");
        assert_eq!(
            outside,
            Decision::NotStated,
            "case \"another service\": expected freetier:* to grant nothing outside this \
             service, got {outside:?}"
        );
    }

    /// RULE: the refusal names the principal, the action, the resource the decision was
    /// taken against and which of IAM's rules refused — every part taken from the request
    /// and the stored policies.
    #[test]
    fn the_refusal_names_the_principal_the_action_and_the_resource() {
        let principal = "arn:aws:iam::000000000000:user/freetier-probe";
        for operation in ["GetAccountPlanState", "UpgradeAccountPlan"] {
            let action = action_of(operation);
            let resource = api_resource(operation);
            let reason = format!("because no identity-based policy allows the {action} action");
            let message = not_authorized(principal, &action, &resource, &reason).message;
            assert_eq!(
                message,
                format!(
                    "User: {principal} is not authorized to perform: {action} on resource: \
                     {resource} {reason}"
                ),
                "case {operation:?}: expected IAM's wording, got {message:?}"
            );
        }
    }

    /// This service's Smithy model, as it ships beside this crate: the authority on which
    /// operations declare the refusal this gate answers with. Read at test time rather
    /// than embedded; no non-test build needs it.
    const MODEL_PATH: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/smithy-model/com-amazonaws-freetier-awsfreetierservice.json"
    );

    /// RULE: this gate judges exactly the operations whose model declares
    /// `AccessDeniedException`, so an operation that models the refusal is judged and one
    /// that does not is left alone. The set comes from the MODEL rather than from a second
    /// hand list, so an operation whose errors change is caught here.
    #[test]
    fn the_judged_operations_are_the_ones_whose_model_declares_the_refusal() {
        let model = std::fs::read_to_string(MODEL_PATH)
            .expect("the model that generated this crate ships beside it");
        let model: ::serde_json::Value =
            ::serde_json::from_str(&model).expect("the model that generated this crate is JSON");
        let shapes = model["shapes"]
            .as_object()
            .expect("a Smithy model declares shapes");
        let mut declared: Vec<&str> = shapes
            .iter()
            .filter(|(_, shape)| shape["type"] == "operation")
            .filter(|(_, shape)| {
                shape["errors"]
                    .as_array()
                    .is_some_and(|errors| errors.iter().any(|error| error["target"]
                        .as_str()
                        .is_some_and(|target| target.ends_with("#AccessDeniedException"))))
            })
            .filter_map(|(id, _)| id.rsplit('#').next())
            .collect();
        declared.sort_unstable();
        let mut judged = JUDGED_OPERATIONS.to_vec();
        judged.sort_unstable();
        assert_eq!(
            judged, declared,
            "case \"this service's surface\": the gate must judge exactly the operations \
             whose model declares AccessDeniedException, got {judged:?} against {declared:?}"
        );
        for operation in &declared {
            assert!(
                judges(operation),
                "case {operation:?}: an operation that models the refusal must be judged"
            );
        }
        for operation in shapes
            .iter()
            .filter(|(_, shape)| shape["type"] == "operation")
            .filter_map(|(id, _)| id.rsplit('#').next())
            .filter(|operation| !declared.contains(operation))
        {
            assert!(
                !judges(operation),
                "case {operation:?}: an operation that models no 403 must not be judged"
            );
        }
    }
}
