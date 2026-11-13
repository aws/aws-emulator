// SPDX-License-Identifier: CC0-1.0

//! Whether IAM lets a request to this media data plane perform the action it asks for —
//! the step every route of this service takes BEFORE it reads or writes a stream's media.
//!
//! AWS decides authorization at the IAM layer, ahead of the service looking at the
//! resource the request named, so a caller whose identity-based policies do not grant the
//! action is told it is not authorized rather than handed media (or told anything about
//! the stream), and the call changes nothing. The code AWS refuses such a call with is
//! `AccessDeniedException`, which neither Kinesis Video media model declares a shape for —
//! their `NotAuthorizedException` is the stream-level refusal — so it is answered through
//! the one channel an operation can return an unmodelled error on
//! ([`crate::scaffolded::Scaffolded::access_denied`]) and, on the ingest plane, in that
//! plane's own error envelope.
//!
//! Three pieces, each held in exactly one place:
//!
//! 1. **who** — the principal the request's access key id speaks for: the session STS
//!    minted that key for (`sts_store::resolve_caller`, the same place
//!    `sts:GetCallerIdentity` answers from, so the principal a refusal blames is the one
//!    the caller reads back for the credentials it signed with), else the IAM user that
//!    owns the long-term key, else the account's own published credential;
//! 2. **what it may do** — the identity-based policy documents the IAM store holds for
//!    that principal and the permissions boundary that caps them, read through the IAM
//!    service's OWN accessors (`principal_policies`, `permissions_boundary_document`), so
//!    a policy `iam:PutRolePolicy` wrote takes effect on the next request and deleting it
//!    lifts it again;
//! 3. **the decision** — [`::service_common::iam_authorization`], the ONE shared evaluation
//!    of those documents against `(action, resource)`, so this service and every other one
//!    handed the same documents answer alike: an explicit `Deny` wins, otherwise an `Allow`
//!    about the request grants it, and with neither the request falls to IAM's default
//!    deny.
//!
//! Only the store LOOKUPS live here; the decision does not.
//!
//! The SENTENCE is the tree's shared one, in its clause-less form
//! ([`::service_common::iam_authorization::not_authorized_to_perform`]): the recorded refusal
//! of a media API of this family states "User: `<principal>` is not authorized to perform:
//! `<action>` on resource: `<the stream the request addressed>`" and stops there, with no
//! clause naming which of IAM's rules withheld the grant — so one sentence answers an
//! explicit `Deny` and IAM's default deny alike, and the resource it names is the identifier
//! the request carried rather than the ARN the decision is taken on ([`AddressedStream`]).
//! Where a service's recording DOES carry that clause, its gate words the refusal with
//! `not_authorized_by_identity_policy` instead; the two forms are the same sentence, stated
//! in the one place.
//!
//! The caller GATHERING below is a per-service copy by the tree's convention — the same shape
//! as `service-com-amazonaws-arcregionswitch-arcregionswitch/src/service/authorization.rs`
//! and `…-kinesisvideosignaling-…/src/state/authorization.rs` — because it reads the IAM and
//! STS service stores, which `services/common` is a dependency of nearly every service crate
//! for; only the DECISION and the SENTENCE, which need no store, live there.
//!
//! A Kinesis video stream carries no resource-based policy — no model of this family binds
//! a policy operation — so the identity-based decision is the whole decision. The action is
//! `kinesisvideo:<ApiName>` ([`ACTION_PREFIX`] is this family's `arnNamespace`) and the
//! resource is the stream the request addresses ([`AddressedStream`]).
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
//! * **An engine carrying no STS or IAM store**, where no session and no role can exist.
//!
//! An expired session credential speaks for nobody: it carries no documents and no
//! principal spelling, so every action it asks for is refused. (A session the
//! authenticating layer has already refused never reaches a handler at all.)
//!
//! What is NOT decided here: a statement's `Condition` clauses are evaluated with an empty
//! request context, so a statement conditioned on a key this emulator does not supply is
//! read as the shared evaluator reads an undecided clause. Nothing here guesses a condition
//! value.

use std::time::SystemTime;

use ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam_store;
use ::service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts_store;
use ::service_common::iam_authorization::{self as iam, Decision};

/// The IAM service prefix the actions of this family carry: `kinesisvideo:GetMedia`,
/// `kinesisvideo:PutMedia`. It is the ARN namespace
/// (`smithy_query(query="service_info", model_path="services/service-com-amazonaws-kinesisvideomedia-awsacuityinletservice/smithy-model/com-amazonaws-kinesisvideomedia-awsacuityinletservice.json")`
/// -> `awsService.arnNamespace`), which this data plane shares with the Kinesis Video
/// control plane and with every other plane of a stream.
pub const ACTION_PREFIX: &str = "kinesisvideo";

/// What a refusal names as the principal of a request whose session credential has already
/// expired: that credential speaks for nobody, and the key it presented is not quoted back.
const EXPIRED_CREDENTIAL_PRINCIPAL: &str = "an expired session credential";

/// The stream a media request addresses, in the two forms the decision needs.
///
/// They differ because a policy and a refusal name a stream differently: a statement's
/// `Resource` element is written as the stream's ARN — the identity the control plane minted
/// and the only one both planes share — while the refusal states the stream AS THE REQUEST
/// ADDRESSED IT. That is what the recorded refusal of this API carries: a call that named the
/// stream by `StreamName` is refused in a sentence carrying that name, not an ARN.
#[derive(Debug, Clone)]
pub struct AddressedStream {
    /// The ARN a statement's `Resource` element is matched against.
    policy_resource: String,
    /// The identifier the request itself carried, which the refusal names.
    addressed_as: String,
}

impl AddressedStream {
    /// The stream the request's `StreamName` / `StreamARN` (or the ingest plane's
    /// addressing headers) names.
    ///
    /// Reads the control-plane registry for the ARN, which is why it is called before this
    /// service's own store is locked.
    pub fn of<Eng: Send + Sync + 'static>(
        engine: &Eng,
        stream_name: Option<&str>,
        stream_arn: Option<&str>,
    ) -> Self {
        Self {
            policy_resource: crate::state::addressed_stream_resource(
                engine,
                stream_name,
                stream_arn,
            ),
            addressed_as: stream_arn.or(stream_name).unwrap_or_default().to_string(),
        }
    }
}

/// Whether IAM permits `api_name` on `stream` for the caller of this request, and the
/// sentence that says why not when it does not.
///
/// `api_name` is the media API the request reached (`Self::NAME` in a generated handler,
/// the ingest plane's own API name on the producer route), which names the IAM action
/// `kinesisvideo:<ApiName>`. `None` when the caller may make the call, and the refusal's
/// message when it may not.
///
/// Asked BEFORE this service's own store is locked and before anything about the stream's
/// state is read, so a caller IAM refuses learns nothing about the stream and nothing is
/// read, written or minted for it; the IAM and STS stores are therefore never reached while
/// this service's guard is held.
///
/// Resolving the caller goes through STS's own `resolve_caller`, which first materialises
/// the session expiries the clock has already made due; that write is owed at the same
/// instant whether or not anybody called, which is why a READING operation may take this
/// decision too.
pub fn authorization_refusal<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    api_name: &str,
    stream: &AddressedStream,
) -> Option<String> {
    let action = format!("{ACTION_PREFIX}:{api_name}");
    let caller = caller(engine, ctx);
    // The principal spellings travel with the request because that is what a statement's
    // `Principal` element would be matched against; an identity-based document states no
    // such element, and a stream carries no resource-based policy (see the module docs), so
    // the decision below rests on the action and the resource.
    let request = iam::Request::new(&action, &stream.policy_resource, &caller.principals);
    let identity_said = iam::identity_decision(&caller.documents, &request);
    // An explicit `Deny` in a document that CAN be read is taken FIRST: it refuses the
    // request whatever else the principal carries — including a grant this emulator cannot
    // read, which only lifts the DEFAULT deny of a request no readable statement allows.
    let granted = identity_said != Decision::Denied
        && (iam::is_allowed(identity_said, Decision::NotStated) || caller.identity_is_unbounded());
    if !granted {
        return Some(refusal(&caller.arn, &action, &stream.addressed_as));
    }
    // "A permissions boundary sets the maximum permissions that an identity-based policy
    // can grant" (IAM, permissions boundaries): a grant the boundary does not also allow
    // is not a permission the principal has. The account's own credential carries none.
    if let Some(boundary) = &caller.boundary {
        if iam::identity_decision(::std::slice::from_ref(boundary), &request) != Decision::Allowed {
            return Some(refusal(&caller.arn, &action, &stream.addressed_as));
        }
    }
    None
}

/// The sentence a Kinesis Video media API refuses an unauthorized call with, through the
/// tree's one wording of it: who was refused, which action, and the stream the request
/// addressed. Every value comes from the request — the principal from the credential it
/// presented, the action from the API it reached, the stream from the identifier it carried —
/// and the rest is fixed text.
///
/// The clause-less form, for the reason the module docs give: the recorded refusal of this
/// API carries no clause naming which of IAM's rules withheld the grant, so one sentence
/// answers every rule the decision can rest on.
fn refusal(principal: &str, action: &str, addressed_as: &str) -> String {
    iam::not_authorized_to_perform(principal, action, addressed_as)
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
    fn decision(document: &str, action: &str, resource: &str) -> Decision {
        iam::identity_decision(
            &[document.to_string()],
            &iam::Request::new(action, resource, &[]),
        )
    }

    /// RULE: the action this gate asks about is `kinesisvideo:<ApiName>`, so a policy
    /// scoped to this family's actions covers every media API of a stream and nothing
    /// outside the family.
    #[test]
    fn the_action_asked_about_carries_this_familys_prefix() {
        let stream = "arn:aws:kinesisvideo:us-east-1:000000000000:stream/probe/1700000000";
        let document =
            r#"{"Statement":[{"Effect":"Allow","Action":"kinesisvideo:*","Resource":"*"}]}"#;
        // The media APIs of a stream: the modelled read route of this service and the
        // producer route beside it (`GetDataEndpoint$APIName`).
        for api_name in ["GetMedia", "PutMedia"] {
            let action = format!("{ACTION_PREFIX}:{api_name}");
            let reached = decision(document, &action, stream);
            assert_eq!(
                reached,
                Decision::Allowed,
                "case {api_name:?}: expected kinesisvideo:* to allow {action}, got {reached:?}"
            );
        }
        let outside = decision(document, "kinesis:GetRecords", stream);
        assert_eq!(
            outside,
            Decision::NotStated,
            "case \"another service\": expected kinesisvideo:* to grant nothing outside this \
             family, got {outside:?}"
        );
    }

    /// RULE: the refusal names the principal, the action and the stream the request
    /// addressed — every part taken from the request — and one sentence answers every rule
    /// the decision can rest on.
    #[test]
    fn the_refusal_names_the_principal_the_action_and_the_stream() {
        let principal = "arn:aws:sts::000000000000:assumed-role/reader/session-1";
        let action = format!("{ACTION_PREFIX}:GetMedia");
        // The stream AS A REQUEST ADDRESSES IT: by name, or by the ARN the control plane
        // minted. Both are the request's own value, carried into the sentence unchanged.
        let rows: &[(&str, &str)] = &[
            ("addressed by name", "probe"),
            (
                "addressed by ARN",
                "arn:aws:kinesisvideo:us-east-1:000000000000:stream/probe/1700000000",
            ),
        ];
        for (case, addressed_as) in rows {
            let message = refusal(principal, &action, addressed_as);
            assert_eq!(
                message,
                format!(
                    "User: {principal} is not authorized to perform: {action} on resource: \
                     {addressed_as}"
                ),
                "case {case:?}: expected the sentence this family words a refusal in, got \
                 {message:?}"
            );
        }
    }
}
