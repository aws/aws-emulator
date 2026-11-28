// SPDX-License-Identifier: CC0-1.0

//! Whether IAM lets a request to this service perform the action it asks for — the
//! decision taken in FRONT of every operation, before it reads or writes anything.
//!
//! Every operation of this service is signed (`smithy_query(query="service_info")`:
//! `authSchemes: [sigv4]`, and the model marks none `@optionalAuth`), so every call is
//! made by a principal — and a principal may only do what a policy in force on it ALLOWS.
//! AWS takes that decision at the IAM layer, AHEAD of the service looking at what the
//! request names, so a caller whose identity-based policies do not grant the action — or
//! that an explicit `Deny` covers — is told it is not authorized rather than told anything
//! about the resource it addressed: the refusal comes before the store is read, so a
//! request for an agreement that does not exist is refused the same way as one for an
//! agreement that does, and nothing of the account changes on the way to it.
//!
//! This service models that refusal as `AccessDeniedException` (`@httpError(403)`), which
//! is a SERVICE-COMMON error of its model (`smithy_query(query="digest")` lists it under
//! `common_errors`), so it is the answer every one of its operations gives such a caller.
//! The decision is therefore taken for EVERY operation rather than for the few whose
//! handler asks for it: it is a property of the request's identity and the action it names
//! rather than of what the handler goes on to do, so it is taken ONCE, in the layer every
//! operation of this service is already served behind
//! ([`crate::service::EmulatedService::override_operation`]), and nowhere else.
//!
//! Three pieces, each held in exactly one place:
//!
//! 1. **who** — the principal the request's access key id speaks for: the session STS
//!    minted that key for ([`sts_store::resolve_caller`], the same place
//!    `sts:GetCallerIdentity` answers from, so the principal a refusal blames is the one
//!    the caller reads back for the credentials it signed with), else the IAM user that
//!    owns the long-term key, else the account's own published credential;
//! 2. **what it may do** — the identity-based policy documents the IAM store holds for
//!    that principal and the permissions boundary that caps them, read through the IAM
//!    service's OWN accessors (`principal_policies`, `permissions_boundary_document`),
//!    where the collection rule itself lives — so a document `iam:PutRolePolicy` wrote
//!    takes effect on the next request and deleting it lifts it again;
//! 3. **the decision** — [`::service_common::iam_authorization`], the tree's ONE evaluation
//!    of those documents against `(action, resource)`, so this service and every other one
//!    handed the same documents answer alike: an explicit `Deny` wins, otherwise an `Allow`
//!    about the request grants it, and with neither the request falls to IAM's default
//!    deny.
//!
//! Only the store LOOKUPS live here; neither the decision nor the reading of a principal
//! ARN does. That is the split every other gate in this tree makes, and the lookups stay in
//! this service crate for the reason those keep theirs there: hoisting them into
//! `service-common` would make every service that depends on that crate link the IAM and
//! STS service crates.
//!
//! ## The actions and the resource
//!
//! The action an operation is performed under is `aws-marketplace:<OperationName>` — this
//! service's `sigv4Name` / `arnNamespace` ([`crate::state::SERVICE_CODE`],
//! `smithy_query(query="service_info")`) and the operation's own shape name — so one policy
//! statement scoped to this service's actions covers every operation of it and nothing
//! outside it. That is the action a refusal says the caller may not PERFORM.
//!
//! An operation may be PERMITTED by more than one action, though: AWS Marketplace grants
//! the view of an account's agreements through the coarse `aws-marketplace:ViewSubscriptions`
//! as well as through the per-API action, so the decision is taken over the SET of actions
//! that permit the operation ([`actions_for`]) rather than over one action — an `Allow` of
//! any of them grants the call, and IAM's "an explicit deny wins" is read over that same
//! set, so a `Deny` of any action the decision was taken over refuses it. Because a
//! refusal over a set is attributable to no single action, AWS states it in one sentence
//! that names the whole set and neither rule (see [`identity_denial_clause`]); an operation
//! only its own action permits keeps IAM's single-action wording. Whichever action granted
//! or refused it, the sentence blames the caller for the action the operation is PERFORMED
//! under, which is the one it asked for.
//!
//! The resource is [`ANY_RESOURCE`], IAM's own resource wildcard, for every operation: the
//! AWS Marketplace actions these operations are performed under take no resource type (the
//! IAM Service Authorization Reference lists none for them), so a statement allows one only
//! by covering every resource — and the refusal names that resource, which is what the
//! request addressed as far as IAM is concerned. Nothing of the request's own body is read
//! here, which is what keeps the decision independent of whether the agreement it names
//! exists.
//!
//! ## The callers this gate does not bound
//!
//! * **The account's own published credential.** A request whose access key id neither STS
//!   nor IAM holds a record for resolves to the account's own identity, which is not a
//!   principal the IAM store holds policies for — the position AWS account root
//!   credentials are in. Such a request is allowed.
//! * **A principal holding an attachment this emulator cannot read.** An AWS-managed policy
//!   belongs to AWS rather than to this account, so the IAM store holds no document for it;
//!   refusing on a grant this emulator cannot see would deny access AWS permits. An
//!   explicit `Deny` in a document that CAN be read still refuses, and a permissions
//!   boundary the store DOES hold still caps the grant.
//! * **An engine carrying no STS or IAM store**, where no session and no role can exist.
//!
//! An expired session credential speaks for nobody: it carries no documents and no
//! principal spelling, so every action it asks for is refused. (A session the
//! authenticating layer has already refused never reaches a handler at all.)

use std::time::SystemTime;

use ::aws_smithy_runtime_api::http::{Response as RtResponse, StatusCode};
use ::aws_smithy_types::body::SdkBody;
use ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam_store;
use ::service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts_store;
use ::service_common::iam_authorization::{self as iam, Decision};
use ::smithy_server_runtime::ServerHttpProtocol;

use crate::ops::types::AccessDeniedException;

/// The resource an action that takes no resource type is judged against, as IAM writes it
/// for such a request — every operation of this service (see the module docs).
pub const ANY_RESOURCE: &str = "*";

/// The wire name of the refusal this gate answers with, and the status the model gives it:
/// `AccessDeniedException`, `@error("client")` at `@httpError(403)`
/// (`smithy_query(query="operation",operation="DescribeAgreement")`).
const ACCESS_DENIED_EXCEPTION: &str = "AccessDeniedException";
/// See [`ACCESS_DENIED_EXCEPTION`].
const ACCESS_DENIED_STATUS: u16 = 403;

/// The media type this awsJson1_0 endpoint renders a body in, for the hand-built fallback
/// below.
const JSON_CONTENT_TYPE: &str = "application/x-amz-json-1.0";

/// What a refusal names as the principal of a request whose session credential has already
/// expired: that credential speaks for nobody, and the key it presented is not quoted back.
const EXPIRED_CREDENTIAL_PRINCIPAL: &str = "an expired session credential";

/// The IAM action `operation` is performed under: `aws-marketplace:<OperationName>`.
pub fn action_for(operation: &str) -> String {
    format!("{}:{operation}", crate::state::SERVICE_CODE)
}

/// The coarse AWS Marketplace action that grants the view of an account's agreements,
/// beside the per-API action.
///
/// This is the alternative AWS itself names when it refuses an unauthorized report of an
/// agreement: its IAM-denial sentence for that call lists this action beside the per-API one
/// as the actions a policy could have allowed (the refusal recorded for this service, quoted
/// in `issues/things-that-now-work/20261110T144129Z-marketplace-agreement-describeagreement-performs-no-identity-based-authorization-an-explicit.md`).
/// A coarse action that grants the view of an account's subscriptions is a statement about
/// the ACCOUNT's agreements rather than about one API, which is why it is read as permitting
/// the operations below and no others.
const VIEW_SUBSCRIPTIONS: &str = "ViewSubscriptions";

/// The operations [`VIEW_SUBSCRIPTIONS`] also permits: the ones that only REPORT what an
/// account's agreements are — the agreement itself (the one the recorded refusal names), the
/// terms it was accepted under, the entitlements it grants, and the search across them —
/// which is the subscription view that action grants. Every other operation of this service
/// (the payment, cancellation, billing-adjustment and purchase-order workflows, and every
/// write) reports or changes something other than what an account's agreements are, so it is
/// permitted by its own action alone.
const VIEWABLE_OPERATIONS: &[&str] = &[
    "DescribeAgreement",
    "GetAgreementEntitlements",
    "GetAgreementTerms",
    "SearchAgreements",
];

/// Every IAM action that permits `operation`, the action it is performed under first.
///
/// A policy allowing ANY of them grants the call and an explicit `Deny` of any of them
/// refuses it, which is what a set of permitting actions means.
pub fn actions_for(operation: &str) -> Vec<String> {
    let mut actions = vec![action_for(operation)];
    if VIEWABLE_OPERATIONS.contains(&operation) {
        actions.push(action_for(VIEW_SUBSCRIPTIONS));
    }
    actions
}

/// The actions as IAM lists them in a refusal: `A`, or `A or B` for an operation more than
/// one action permits.
fn named(actions: &[String]) -> String {
    actions.join(" or ")
}

/// IAM's clause for refusing `actions` on the identity-based policies in force, where
/// `explicit` says a document this emulator can read DENIES one of them.
///
/// An operation SEVERAL actions permit is refused in one sentence naming the whole set,
/// which attributes the refusal to neither of IAM's two rules — the refusal AWS answers
/// such a call with carries that clause whether a statement denied the action or none
/// allowed it. An operation one action permits is refused in IAM's single-action wording,
/// which names the rule.
fn identity_denial_clause(actions: &[String], explicit: bool) -> String {
    if actions.len() > 1 {
        return format!(
            "because no identity-based policy allows the {} action, or explicitly denies the \
             actions",
            named(actions)
        );
    }
    if explicit {
        "with an explicit deny in an identity-based policy".to_string()
    } else {
        format!(
            "because no identity-based policy allows the {} action",
            named(actions)
        )
    }
}

/// The ARN of the principal this request speaks for, as a response member that names a
/// caller carries it (`CallerIdentity$principalArn`, "the identity that overrode the
/// approver's outcome").
///
/// Resolved through the SAME [`caller`] this gate decides on, so the principal a decision
/// records is the principal the decision was authorized for. A caller whose credential
/// speaks for nobody — the expired-session case [`caller`] states in words rather than as
/// an ARN — is reported as the account's own published identity: the member is an ARN
/// (`^arn:[^:]+:[^:]+:[^:]*:[^:]*:.+$`), so a value that is no ARN cannot be written into
/// it, and in the assembled emulator the layer that authenticates a request refuses such a
/// credential before any handler runs.
pub fn caller_principal_arn<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
) -> String {
    let arn = caller(engine, ctx).arn;
    if arn.starts_with("arn:") {
        arn
    } else {
        ::service_common::caller_principal::ambient_user_arn()
    }
}

/// Authorize this request, where `operation` is the operation's own name
/// ([`::smithy_server_runtime::Operation::NAME`]).
///
/// `Ok(())` for a request IAM permits (and for every caller this gate does not bound, see
/// the module docs); the modelled refusal otherwise. It is called BEFORE the request's own
/// input is read, so the decision cannot depend on what the request names or on whether
/// that resource exists — which is also what keeps a refused call from telling an
/// unauthorized caller what the account holds.
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
    let actions = actions_for(operation);
    // What a refusal blames the caller for is the action the operation is PERFORMED under,
    // which is the FIRST of the actions that permit it ([`actions_for`], pinned by this
    // module's own test); that function always states at least that action, so the fallback
    // cannot be reached.
    let action = actions
        .first()
        .cloned()
        .unwrap_or_else(|| action_for(operation));
    let caller = caller(engine, ctx);
    // The principal spellings travel with the request because that is what a statement's
    // `Principal` element would be matched against; an identity-based document states no
    // such element, and this service publishes no resource-based policy of its own — its
    // model declares no policy operation and no policy member — so the decision below
    // rests on the actions and the resource.
    let documents = iam::prepare_documents(&caller.documents);
    let identity_said = over_actions(&actions, &caller.principals, |request| {
        iam::identity_decision_prepared(&documents, request)
    });
    // The decision is taken FIRST, so an explicit `Deny` in a document that CAN be read
    // refuses the request whatever else the principal carries — including a grant this
    // emulator cannot read, which only lifts the DEFAULT deny of a request no readable
    // statement allows (see the module docs).
    if identity_said == Decision::Denied {
        return Err(refused(
            &caller.arn,
            &action,
            operation,
            &identity_denial_clause(&actions, true),
        ));
    }
    let granted =
        iam::is_allowed(identity_said, Decision::NotStated) || caller.identity_is_unbounded();
    if !granted {
        return Err(refused(
            &caller.arn,
            &action,
            operation,
            &identity_denial_clause(&actions, false),
        ));
    }
    // "A permissions boundary sets the maximum permissions that an identity-based policy
    // can grant" (IAM, permissions boundaries): a grant the boundary does not also allow is
    // not a permission the principal has. The account's own credential carries none.
    if let Some(boundary) = &caller.boundary {
        let capped = iam::prepare_documents(::std::slice::from_ref(boundary));
        let reason = match over_actions(&actions, &caller.principals, |request| {
            iam::identity_decision_prepared(&capped, request)
        }) {
            Decision::Allowed => return Ok(()),
            // An explicit deny is a fact about the boundary document, so the refusal names
            // it; a boundary that merely states nothing withheld the grant the
            // identity-based policies gave.
            Decision::Denied => "with an explicit deny in a permissions boundary".to_string(),
            Decision::NotStated => format!(
                "because no permissions boundary allows the {} action",
                named(&actions)
            ),
        };
        return Err(refused(&caller.arn, &action, operation, &reason));
    }
    Ok(())
}

/// What one set of documents says about an operation SEVERAL actions permit: `decide` is
/// asked about each of them on [`ANY_RESOURCE`], and the answers are combined the way a set
/// of permitting actions combines them — a `Deny` of any action refuses, otherwise an
/// `Allow` of any grants, and with neither the documents state nothing.
fn over_actions(
    actions: &[String],
    principals: &[String],
    mut decide: impl FnMut(&iam::Request<'_>) -> Decision,
) -> Decision {
    let mut reached = Decision::NotStated;
    for action in actions {
        match decide(&iam::Request::new(action, ANY_RESOURCE, principals)) {
            Decision::Denied => return Decision::Denied,
            Decision::Allowed => reached = Decision::Allowed,
            Decision::NotStated => {}
        }
    }
    reached
}

/// The refusal IAM states an authorization failure with: who was refused, which action, on
/// which resource, and which of IAM's two rules refused it. Every value in it comes from
/// the request and from the stored policies — the principal from the credential the request
/// presented, the action from the operation it named, the resource from what the action
/// takes.
///
/// The body carries the `requestId` every exception shape of this service carries
/// ([`crate::state::next_request_id`]) and no `reason`: `AccessDeniedExceptionReason`
/// publishes the marketplace conditions a request can be denied under (a private
/// marketplace policy, KYC, the account's state, MFA) and none of them is "IAM does not
/// allow it", which is a decision taken before this service sees the request at all.
///
/// The refusal is the security-relevant event of such a call, so it is attributable in the
/// emulator's log on the same `audit` target this crate's state-change record uses
/// ([`crate::state::audit`]), which is what lets a deployment route these records apart
/// from the emulator's own diagnostics: which operation was asked for, which action it was
/// judged under, the principal that asked (the ARN `sts:GetCallerIdentity` reports for its
/// credentials — an identifier, never a secret) and the outcome. The policy documents
/// themselves are NOT logged, and neither is anything else of the request.
fn refused(
    principal: &str,
    action: &str,
    operation: &str,
    reason: &str,
) -> AccessDeniedException {
    ::tracing::info!(
        target: "audit",
        operation,
        outcome = "refused_access_denied",
        resource = %ANY_RESOURCE,
        action = %action,
        principal = %principal,
        "the identity-based policies in force on the caller do not permit this call"
    );
    AccessDeniedException {
        request_id: Some(crate::state::next_request_id()),
        message: Some(format!(
            "User: {principal} is not authorized to perform: {action} on resource: \
             {ANY_RESOURCE} {reason}"
        )),
        reason: None,
    }
}

/// The wire answer to a request this gate refused: the modelled `AccessDeniedException` at
/// its modelled 403, serialized through the SAME protocol a handler-returned error of that
/// shape goes through, so a refusal taken in this layer and one taken in a handler are
/// indistinguishable on the wire.
///
/// The hand-built fallback carries the same two things a client branches on — the status
/// and the `__type` naming the shape — for the one case the serializer can fail: a value it
/// cannot render.
#[must_use]
pub fn refusal_response(
    protocol: &dyn ServerHttpProtocol,
    denial: &AccessDeniedException,
) -> RtResponse {
    protocol
        .serialize_error(
            denial,
            &crate::ops::types::SCHEMA_ACCESS_DENIED_EXCEPTION,
            ACCESS_DENIED_EXCEPTION,
            ACCESS_DENIED_STATUS,
        )
        .unwrap_or_else(|_| {
            let body = ::serde_json::json!({
                "__type": ACCESS_DENIED_EXCEPTION,
                "message": denial.message.clone().unwrap_or_default(),
            });
            let mut response = RtResponse::new(
                StatusCode::try_from(ACCESS_DENIED_STATUS)
                    .expect("the status is a constant of this module"),
                SdkBody::from(body.to_string()),
            );
            response
                .headers_mut()
                .insert("content-type", JSON_CONTENT_TYPE);
            response
                .headers_mut()
                .insert("x-amzn-errortype", ACCESS_DENIED_EXCEPTION);
            response
        })
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
        // request — and in the assembled emulator the layer that authenticates a request
        // refuses an expired credential before any handler runs.
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
        // by the ARN the account holds it under — which carries its PATH, where the session
        // ARN does not.
        if let Some(held) = store.roles.get(&iam_store::name_key(role_name)) {
            caller.principals.push(held.arn.clone());
            caller.read_policies_of(&store, &held.arn.clone());
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
    fn decision(document: &str, action: &str) -> Decision {
        iam::identity_decision(
            &[document.to_string()],
            &iam::Request::new(action, ANY_RESOURCE, &[]),
        )
    }

    /// RULE: the action this gate asks about is `aws-marketplace:<OperationName>`, so a
    /// statement scoped to this service's actions covers every operation of it and nothing
    /// outside it.
    #[test]
    fn the_action_asked_about_carries_this_services_prefix() {
        let document =
            r#"{"Statement":[{"Effect":"Allow","Action":"aws-marketplace:*","Resource":"*"}]}"#;
        for operation in [
            "DescribeAgreement",
            "CreateAgreementRequest",
            "SearchAgreements",
            "UpdatePurchaseOrders",
            "ListAgreementCharges",
        ] {
            let action = action_for(operation);
            let reached = decision(document, &action);
            assert_eq!(
                reached,
                Decision::Allowed,
                "case {operation:?}: expected the service-wide allow to cover {action}, got \
                 {reached:?}"
            );
        }
        let outside = decision(document, "s3:GetObject");
        assert_eq!(
            outside,
            Decision::NotStated,
            "case \"another service\": expected the service-wide allow to grant nothing \
             outside this service, got {outside:?}"
        );
    }

    /// RULE: a grant is read for the ACTION it names, so a statement naming one operation
    /// of this service allows nothing about another.
    #[test]
    fn a_grant_is_read_for_the_action_it_names() {
        let document = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow",
            "Action":"aws-marketplace:DescribeAgreement","Resource":"*"}]}"#;
        // (case, operation, allowed)
        let rows: &[(&str, &str, bool)] = &[
            ("the granted action", "DescribeAgreement", true),
            ("another read of this service", "GetAgreementTerms", false),
            ("a write of this service", "CancelAgreement", false),
        ];
        for (case, operation, expected) in rows {
            let said = decision(document, &action_for(operation));
            let actual = iam::is_allowed(said, Decision::NotStated);
            assert_eq!(
                actual, *expected,
                "case {case:?}: expected allowed == {expected}, got {actual} ({said:?})"
            );
        }
    }

    /// RULE: the refusal names the principal, the action, the resource the action takes and
    /// which of IAM's rules refused it — every part taken from the request and the stored
    /// policies.
    #[test]
    fn the_refusal_names_the_principal_the_action_and_the_resource() {
        let principal = format!(
            "arn:aws:sts::{}:assumed-role/agreement-reader/session-1",
            ::engine::emulator_account_id()
        );
        let action = action_for("DescribeAgreement");
        // (case, the clause the sentence must carry)
        let rows: &[(&str, String)] = &[
            (
                "no policy allows it",
                format!("because no identity-based policy allows the {action} action"),
            ),
            (
                "a policy denies it",
                "with an explicit deny in an identity-based policy".to_string(),
            ),
            (
                "a boundary withholds it",
                format!("because no permissions boundary allows the {action} action"),
            ),
        ];
        for (case, reason) in rows {
            let denial = refused(&principal, &action, "DescribeAgreement", reason);
            let message = denial.message.unwrap_or_default();
            let expected = format!(
                "User: {principal} is not authorized to perform: {action} on resource: \
                 {ANY_RESOURCE} {reason}"
            );
            assert_eq!(
                message, expected,
                "case {case:?}: expected IAM's wording, got {message:?}"
            );
            assert!(
                denial.reason.is_none(),
                "case {case:?}: an IAM denial states no marketplace reason, got {:?}",
                denial.reason
            );
        }
    }

    /// RULE: an operation is decided over every action that PERMITS it — its own action,
    /// plus the coarse view action for the operations that only report an account's
    /// agreements — so an allow of any of them grants it and a deny of any refuses it.
    #[test]
    fn an_operation_is_decided_over_every_action_that_permits_it() {
        let view = action_for(VIEW_SUBSCRIPTIONS);
        // (case, operation, whether the coarse view action also permits it)
        let rows: &[(&str, &str, bool)] = &[
            ("a read of one agreement", "DescribeAgreement", true),
            ("the terms of an agreement", "GetAgreementTerms", true),
            ("the entitlements it grants", "GetAgreementEntitlements", true),
            ("the search across agreements", "SearchAgreements", true),
            ("a payment-request read", "GetAgreementPaymentRequest", false),
            ("a listing of charges", "ListAgreementCharges", false),
            ("a write", "CancelAgreement", false),
            ("the create", "CreateAgreementRequest", false),
        ];
        for (case, operation, viewable) in rows {
            let actions = actions_for(operation);
            let expected: Vec<String> = if *viewable {
                vec![action_for(operation), view.clone()]
            } else {
                vec![action_for(operation)]
            };
            assert_eq!(
                actions, expected,
                "case {case:?}: expected {expected:?} to be the permitting actions, got \
                 {actions:?}"
            );
            // An allow of ANY permitting action grants the call, and a deny of any refuses.
            let allow_view =
                format!(r#"{{"Statement":[{{"Effect":"Allow","Action":"{view}","Resource":"*"}}]}}"#);
            let said = over_actions(&actions, &[], |request| {
                iam::identity_decision(::std::slice::from_ref(&allow_view), request)
            });
            let granted = iam::is_allowed(said, Decision::NotStated);
            assert_eq!(
                granted, *viewable,
                "case {case:?}: expected the coarse view grant to allow == {viewable}, got \
                 {granted} ({said:?})"
            );
            let deny_own = format!(
                r#"{{"Statement":[{{"Effect":"Allow","Action":"*","Resource":"*"}},
                   {{"Effect":"Deny","Action":"{}","Resource":"*"}}]}}"#,
                action_for(operation)
            );
            let denied = over_actions(&actions, &[], |request| {
                iam::identity_decision(::std::slice::from_ref(&deny_own), request)
            });
            assert_eq!(
                denied,
                Decision::Denied,
                "case {case:?}: expected a deny of the operation's own action to refuse it, \
                 got {denied:?}"
            );
        }
    }

    /// RULE: a refusal of an operation SEVERAL actions permit names the whole set in one
    /// sentence that attributes it to neither of IAM's rules; one a single action permits
    /// names the rule that refused it.
    #[test]
    fn the_clause_names_every_action_the_decision_was_taken_over() {
        let view = action_for(VIEW_SUBSCRIPTIONS);
        // (case, operation, whether a readable document denied an action, the clause)
        let rows: &[(&str, &str, bool)] = &[
            ("a read no policy allows", "DescribeAgreement", false),
            ("a read a policy denies", "DescribeAgreement", true),
            ("a write no policy allows", "CancelAgreement", false),
            ("a write a policy denies", "CancelAgreement", true),
        ];
        for (case, operation, explicit) in rows {
            let actions = actions_for(operation);
            let own = action_for(operation);
            let expected = if actions.len() > 1 {
                format!(
                    "because no identity-based policy allows the {own} or {view} action, or \
                     explicitly denies the actions"
                )
            } else if *explicit {
                "with an explicit deny in an identity-based policy".to_string()
            } else {
                format!("because no identity-based policy allows the {own} action")
            };
            let clause = identity_denial_clause(&actions, *explicit);
            assert_eq!(
                clause, expected,
                "case {case:?}: expected IAM's clause {expected:?}, got {clause:?}"
            );
        }
    }
}
