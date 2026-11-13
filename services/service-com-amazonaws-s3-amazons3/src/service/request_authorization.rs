// SPDX-License-Identifier: CC0-1.0

//! WHO a signed request speaks for, and whether the policies in force let it perform the
//! action it asks for — the authorization step an object or bucket-configuration handler
//! runs BEFORE it reads or writes anything.
//!
//! "When a request is made, the Amazon Web Services principal sends a request to Amazon S3.
//! Amazon S3 evaluates all of the relevant access policies, user policies, and
//! resource-based policies in deciding whether to authorize the request" (Amazon S3 User
//! Guide, *Identity and Access Management for Amazon S3*). So a signed request is NOT the
//! bucket owner's by assumption: it is judged, per action, against the documents in force on
//! the principal that signed it and against the bucket's own policy.
//!
//! Four pieces, each held in exactly one place:
//!
//! 1. **who** — the principal the request's access key id speaks for ([`request_caller`]):
//!    the session STS minted that key for (the same place `sts:GetCallerIdentity` answers
//!    from, so the principal a refusal blames is the one the caller reads back for the
//!    credentials it signed with), else the IAM user that holds the long-term key, else the
//!    account's own published credential.
//! 2. **what it may do** — the identity-based documents the IAM store holds for that
//!    principal, read through the IAM service's OWN accessor (`principal_policies`, where
//!    the collection rule itself lives, groups included), so a policy `iam:PutUserPolicy`
//!    wrote takes effect on the next request and deleting it lifts it again; and the
//!    BUCKET's own policy, which this service holds.
//! 3. **what the CREDENTIAL itself was scoped to** — the session policies the mint of the
//!    credential passed, which STS holds on the session the access key id names
//!    (`sts_store::session_scoping_documents`) and which are read by the one reading of a
//!    resolved scope (`sts_store::ScopingPolicies::decision`). They SCOPE the permissions of
//!    the identity down and grant nothing of their own: "the resulting session's permissions
//!    are the intersection of the role's identity-based policies and the session policies"
//!    (`AssumeRoleRequest$Policy`). A long-term access key is scoped by none.
//! 4. **the decision** — [`::service_common::iam_authorization`], the tree's ONE evaluation
//!    of policy documents against `(action, resource, principal, context)`, so this service
//!    and every other one handed the same documents answer alike.
//!
//! ## The rule
//!
//! In IAM's own order ("Determining whether a request is allowed or denied within an
//! account"): an explicit `Deny` about the request — in the bucket policy, in an
//! identity-based document or in a session policy — refuses it, whatever any grant says.
//! Otherwise the request
//! needs a grant: an `Allow` about it among the caller's identity-based documents, or one in
//! the bucket policy that names the CALLER ("the resource-based policy and the
//! identity-based policy are evaluated together, and access is granted if either allows
//! it"). With neither the request falls to IAM's implicit deny and is refused 403
//! `AccessDenied` ([`state::ACCESS_DENIED_CODE`]), naming the principal, the action and the
//! resource it addressed. A grant still has to pass the scope the credential was minted
//! with: a request the session policies do not allow is refused even where the documents in
//! force on the identity allow it, and the refusal names THEM as the document that withheld
//! the grant, because that needs a different fix.
//!
//! A statement may be CONDITIONED, and a condition is decided against the keys the request
//! carries values for — the request's own members, the global keys this gate answers for
//! every request, and whether the credential that signed was MFA-backed
//! ([`authorization::multi_factor_auth_conditions`], read from the credential rather than
//! decided here), which is what a "deny unless MFA is present" statement is written over.
//!
//! A statement that names the ACCOUNT rather than the caller (the account's root ARN, or the
//! bare account id that stands for it) delegates to the account rather than granting — "you
//! must also attach an identity-based policy" to the principal — so the account spellings
//! decide a `Deny`, which covers every principal of the account, and never make a grant on
//! their own.
//!
//! Only a deny the evaluation could DECIDE refuses ([`authorization::deny_is_decided`]): a
//! clause over a condition key this service does not answer leaves its statement undecided,
//! and reading an undecided `Deny` as a refusal would refuse calls on every bucket whose
//! policy conditions anything this emulator does not model.
//!
//! ## The callers this gate does not bound
//!
//! * **The account's own published credential.** A request whose access key id neither STS
//!   nor IAM holds a record for speaks for the account itself — the position AWS account root
//!   credentials are in, which no identity-based policy bounds — so it is allowed where no
//!   readable document denies it. This is also what every in-process call another emulated
//!   service makes against S3 presents.
//! * **A principal holding an attachment this emulator cannot read.** An AWS-managed policy
//!   is owned by AWS rather than by the account, so the account's store holds no document for
//!   it; refusing on a grant this emulator cannot see would deny access AWS permits. An
//!   explicit `Deny` in a document that CAN be read still refuses. A credential scoped by a
//!   MANAGED session policy whose document is likewise unheld is in that same position: a
//!   scope this emulator cannot read withholds no grant, while a `Deny` in a document it CAN
//!   read still refuses.
//! * **An engine carrying no STS or IAM store**, where no session and no identity can exist.
//!
//! An expired session credential speaks for nobody: it carries no documents and no principal
//! spelling, so every action it asks for is refused. (In the assembled emulator the layer
//! that authenticates a request refuses such a credential before any handler runs.)
//!
//! ## The caller that is another service
//!
//! One class of request is no customer principal's at all: a call ANOTHER emulated service
//! makes as its OWN service principal for one of its resources — CloudFront fetching an
//! origin object for a distribution whose origin access control signed the fetch. AWS decides
//! such a call for that service principal, with `aws:SourceArn` and `aws:SourceAccount`
//! naming the resource it acts for, and a service principal holds no identity-based policies:
//! the BUCKET's own policy is the only document in force, so the call needs a grant in it that
//! names the service principal and whose conditions the call's own source keys satisfy.
//!
//! Which calls are in that class is recorded by the CALLING service, under the access key id
//! of the credential it minted for the one call and signed it with, for only as long as that
//! call is in flight ([`::service_common::service_principal_calls`]) — so it is resolved here
//! from the access key id the request PRESENTS ([`service_principal_call`]), which is the one
//! thing about a request a client cannot choose for itself. Every other request is resolved
//! from the credential's own principal, as above.

use std::time::SystemTime;

use ::service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts_store;
use ::service_common::iam_authorization::{self as iam, Decision};

use super::authorization::{self, PUBLISHED_CONDITION_KEYS};
use crate::state::{self, Bucket, Object, GET_OBJECT_ACTION, LIST_BUCKET_ACTION};

/// The IAM action a read that names a `versionId` is authorized under: "if you include a
/// `versionId` in your request header, you must have the `s3:GetObjectVersion` permission to
/// access a specific version of an object. The `s3:GetObject` permission is not required in
/// this scenario" (`GetObject`'s own documentation) — so naming a version asks for THIS
/// action instead of [`GET_OBJECT_ACTION`], and a read of the current version asks for that
/// one alone.
pub const GET_OBJECT_VERSION_ACTION: &str = "s3:GetObjectVersion";

/// The IAM action an object WRITE is authorized under: "you must have the `s3:PutObject`
/// permission" (`PutObject`'s own documentation of its general purpose bucket permissions).
pub const PUT_OBJECT_ACTION: &str = "s3:PutObject";

/// The IAM action a delete that names NO `versionId` is authorized under: "`s3:DeleteObject`
/// — to delete an object (or add a delete marker)" (`DeleteObject`'s own documentation of
/// the permissions a delete needs).
pub const DELETE_OBJECT_ACTION: &str = "s3:DeleteObject";

/// The IAM action a delete that NAMES a `versionId` is authorized under: "`s3:DeleteObject
/// Version` — to delete a specific version of an object" (same documentation), which is a
/// permission of its own — a caller holding only [`DELETE_OBJECT_ACTION`] may stack a delete
/// marker and may not remove a version.
pub const DELETE_OBJECT_VERSION_ACTION: &str = "s3:DeleteObjectVersion";

/// The IAM action a listing of a bucket's VERSIONS is authorized under: "you must have the
/// `s3:ListBucketVersions` permission" (`ListObjectVersions`'s own documentation), which the
/// permission to list the bucket's current objects ([`LIST_BUCKET_ACTION`]) does not cover.
pub const LIST_BUCKET_VERSIONS_ACTION: &str = "s3:ListBucketVersions";

/// The IAM action a read of a bucket's default-encryption configuration is authorized under:
/// "to use this operation, you must have permission to perform the
/// `s3:GetEncryptionConfiguration` action" (`GetBucketEncryption`'s own documentation). The
/// name does not follow the operation's, so a policy granting the `s3:GetBucket*` family does
/// NOT cover it.
pub const GET_ENCRYPTION_CONFIGURATION_ACTION: &str = "s3:GetEncryptionConfiguration";

/// The IAM action a bucket-policy write is authorized under: "the `s3:PutBucketPolicy`
/// permission is required in a policy" (`PutBucketPolicy`'s own documentation).
pub const PUT_BUCKET_POLICY_ACTION: &str = "s3:PutBucketPolicy";

/// What a refusal names as the principal of a request whose session credential has already
/// expired: that credential speaks for nobody, and the key it presented is not quoted back.
const EXPIRED_CREDENTIAL_PRINCIPAL: &str = "an expired session credential";

/// The cross-service call a request IS, when another emulated service made it as its own
/// service principal on behalf of one of its resources (see the module docs).
#[derive(Debug, Clone)]
pub struct ServicePrincipalCall {
    /// The service principal the call was made as, which is the principal a bucket policy
    /// names to grant it.
    pub principal: String,
    /// `aws:SourceArn`: the resource the call was made on behalf of, which is what a grant to
    /// that principal is scoped by.
    pub source_arn: String,
    /// `aws:SourceAccount`: the account that resource belongs to.
    pub source_account: String,
}

/// The cross-service call this request IS, or `None` for every other request — a request
/// whose access key id the shared directory holds no live record of, and one on an engine
/// carrying no such directory.
///
/// The record is read for the access key id the request PRESENTS, which the emulator's
/// verifier resolves and checks the signature against when request authentication is on, and
/// only for a call of the account this service serves
/// ([`::service_common::service_principal_calls::ServicePrincipalCalls::call`] carries both
/// bounds).
///
/// LOCKING: this takes the directory's lock and no other, so a handler may ask it while
/// holding this service's own store guard — which is what the decision an SSE-KMS read makes
/// about the key does ([`crate::state::sse_kms`]).
#[must_use]
pub fn service_principal_call<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
) -> Option<ServicePrincipalCall> {
    let directory = in_flight_service_principal_calls(engine)?;
    let access_key_id = ::engine::extract_akid(&ctx.parts().headers);
    let held = directory.lock();
    let call = held.call(
        &access_key_id,
        ::engine::emulator_account_id(),
        SystemTime::now(),
    )?;
    Some(ServicePrincipalCall {
        principal: call.principal.clone(),
        source_arn: call.source_arn.clone(),
        source_account: call.source_account.clone(),
    })
}

/// The shared directory of the cross-service calls another service is making as its own
/// service principal, in the one shared engine: written by that service, read here.
fn in_flight_service_principal_calls<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<
    ::engine::ServiceState<::service_common::service_principal_calls::ServicePrincipalCalls>,
> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(
            ::engine::InMemoryEngine::state::<
                ::service_common::service_principal_calls::ServicePrincipalCalls,
            >,
        )
}

/// The principal a request speaks for, and everything the decision is made from.
///
/// Resolved once per request and BEFORE this service's own store is locked, so the STS and
/// IAM stores are never reached while this service's state guard is held.
#[derive(Debug, Clone, Default)]
pub struct Caller {
    /// The principal's ARN, as `sts:GetCallerIdentity` reports it — what a refusal names.
    pub arn: String,
    /// Every spelling of the principal a policy may name IT by: the ARN STS reports, and the
    /// ARN of the IAM role behind an assumed-role session. These decide a GRANT.
    pub principals: Vec<String>,
    /// The ARN of the IDENTITY whose policies are in force: the role behind an assumed-role
    /// session, else the principal's own ARN. It is what `aws:PrincipalArn` answers with
    /// (`authorization::global_conditions`), and empty for a caller that speaks for nobody.
    pub identity_arn: String,
    /// The spellings that name the ACCOUNT the principal belongs to rather than the
    /// principal: they decide a deny and never grant (see the module docs).
    pub account_principals: Vec<String>,
    /// The identity-based policy documents in force on the principal.
    pub documents: Vec<String>,
    /// The session policy documents the mint of this credential scoped it with, as they read
    /// at the instant of this request, and whether a managed one among them names a document
    /// the IAM store does not hold (see the module docs). Empty for a credential no mint
    /// scoped, which nothing scopes down.
    pub scope: sts_store::ScopingPolicies,
    /// What the request's MFA condition key answers for the credential presented: `Some` for a
    /// session, whose mint either satisfied an MFA challenge or did not, and `None` for a
    /// long-term access key, which carries no value for the key
    /// (`sts_store::Caller::multi_factor_auth_present`).
    pub multi_factor_auth_present: Option<bool>,
    /// Set when this request presents the account's own published credential, which holds no
    /// IAM policies (see the module docs).
    pub account_credential: bool,
    /// Set when the principal carries a policy whose document the IAM store does not hold —
    /// an AWS-managed policy (see the module docs).
    pub unreadable_attachment: bool,
    /// Set when this request is a call another service made as its own SERVICE PRINCIPAL for
    /// one of its resources (see the module docs): the resource's policy is then the only
    /// document in force, and the call carries the two source condition keys a grant to such
    /// a principal is scoped by.
    pub service_principal: Option<ServicePrincipalCall>,
    /// The session tags the principal carries, as the mint attached them when the session
    /// was minted: the values `aws:PrincipalTag/<key>` holds for a request this credential
    /// signs.
    pub tags: Vec<(String, String)>,
    /// Set when the principal's session tags are KNOWN — a session this emulator's token
    /// registry holds — which is what lets a clause over a tag key the principal does NOT
    /// carry be decided rather than left undecided. A long-term credential's tags live in
    /// IAM and are not read here, so such a caller declares nothing about them.
    pub tags_known: bool,
}

impl Caller {
    /// Every spelling a DENY is decided against: the principal's own and its account's,
    /// because a statement denying the account denies each of its principals.
    fn denied_spellings(&self) -> Vec<String> {
        let mut named = self.principals.clone();
        named.extend(self.account_principals.iter().cloned());
        named
    }

    /// Whether nothing the identity-based decision found can refuse this caller once no
    /// readable document DENIES the action (see the module docs).
    fn identity_is_unbounded(&self) -> bool {
        self.account_credential || self.unreadable_attachment
    }

    /// The caller a request another service made as its own service principal speaks for: the
    /// principal itself, named by nothing else.
    ///
    /// It holds no identity-based documents and no account spelling — a service principal is
    /// not a principal OF the account, so a statement about the account says nothing about it
    /// — and it is not one of the callers the identity-based half leaves unbounded: the
    /// resource's own policy is the only document in force, so the call needs a grant there.
    /// It carries no `aws:PrincipalArn` value either: that key names an IAM identity, and this
    /// call is made by none, so the key is reported ABSENT — the same treatment a caller
    /// speaking for nobody gets ([`authorization::global_conditions`]) — rather than being
    /// decided against a principal spelling AWS does not report for such a call.
    fn of_service_principal(call: ServicePrincipalCall) -> Self {
        Self {
            arn: call.principal.clone(),
            principals: vec![call.principal.clone()],
            service_principal: Some(call),
            ..Self::default()
        }
    }
}

/// Who this request speaks for, resolved from the access key id it signed with.
///
/// Call it BEFORE locking this service's store (see [`Caller`]). Resolving the caller goes
/// through STS's own resolution, which first materialises the session expiries the clock has
/// already made due: that write is owed at the same instant whether or not anybody called,
/// which is why a REPORTING operation of this service may take this decision too.
///
/// A request another service made as its own SERVICE PRINCIPAL is that principal and nothing
/// else ([`service_principal_call`], and the module docs for why): its credential is one the
/// emulator minted for that one call, so no session and no IAM identity stands behind it.
pub fn request_caller<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
) -> Caller {
    if let Some(call) = service_principal_call(engine, ctx) {
        ::tracing::debug!(
            principal = %call.principal,
            source_arn = %call.source_arn,
            "this request is one another service is making as its own service principal, so it \
             is authorized as that principal acting for that resource"
        );
        return Caller::of_service_principal(call);
    }
    let access_key_id = ::engine::extract_akid(&ctx.parts().headers);
    let iam_state = sts_store::iam_state(engine);
    // A long-term access key speaks for the IAM user that holds it, which is the identity
    // STS itself reports for it; a key no store holds is the account's own credential.
    let long_term = match &iam_state {
        Some(iam_state) => sts_store::long_term_identity(Some(&iam_state.lock()), &access_key_id),
        None => sts_store::ambient_identity(),
    };
    let resolved = match sts_store::service_state(engine) {
        Some(sts) => sts_store::resolve_request_caller(
            &mut sts.lock(),
            &access_key_id,
            SystemTime::now(),
            ctx.credential_store(),
            long_term.clone(),
        )
        .map(|caller| {
            (
                caller.identity.arn.clone(),
                caller.role_arn.clone(),
                caller.session_policies.clone(),
                caller.multi_factor_auth_present(),
                // The tags are the registry's record of the session, so they are known
                // exactly for a credential it minted.
                caller.kind.is_some(),
                caller.tags.clone(),
            )
        }),
        // No STS store on this engine: the credential names no session, so it speaks for the
        // long-term identity it was resolved to.
        None => Ok((
            long_term.arn.clone(),
            None,
            sts_store::SessionPolicies::default(),
            None,
            false,
            Vec::new(),
        )),
    };
    // An expired session credential speaks for nobody: it carries no documents and no
    // principal spelling, so every action it asks for falls to the implicit deny.
    let Ok((arn, role_arn, session_policies, multi_factor_auth_present, tags_known, tags)) =
        resolved
    else {
        return Caller {
            arn: EXPIRED_CREDENTIAL_PRINCIPAL.to_string(),
            ..Caller::default()
        };
    };
    let mut caller = Caller {
        account_credential: arn == sts_store::ambient_identity().arn,
        principals: vec![arn.clone()],
        account_principals: iam::account_principals(&arn),
        arn,
        multi_factor_auth_present,
        tags,
        tags_known,
        ..Caller::default()
    };
    // An assumed-role session is also named by the ARN of the ROLE it is a session of, which
    // is the spelling a policy uses to name the role itself, and the identity whose policies
    // are in force for it ("the temporary security credentials … have the permissions of the
    // role", `AssumeRole`).
    let identity_arn = match role_arn {
        Some(role_arn) => {
            if !caller.principals.contains(&role_arn) {
                caller.principals.push(role_arn.clone());
            }
            role_arn
        }
        None => caller.arn.clone(),
    };
    if let Some(iam_state) = iam_state {
        // The IAM store of the same engine, read ONCE for both halves of what this credential
        // may do: the documents in force on its identity, and the documents its mint scoped it
        // with — the latter read at the instant of the request, so a managed scoping policy
        // edited since the mint scopes by what it says NOW.
        let held = iam_state.lock();
        if let Some(policies) = held.principal_policies(&identity_arn) {
            caller.documents = policies.documents;
            // An attachment the collection resolved no document for is an entry of AWS's own
            // catalog this emulator does not carry: a grant it cannot read.
            caller.unreadable_attachment = !policies.uncarried.is_empty();
        }
        caller.scope = sts_store::session_scoping_documents(Some(&held), &session_policies);
    } else {
        // No IAM store to read a managed scoping policy out of; the inline document the mint
        // passed scopes the session whether or not a store is there.
        caller.scope = sts_store::session_scoping_documents(None, &session_policies);
    }
    caller.identity_arn = identity_arn;
    caller
}

/// Why the policies in force refuse `caller` the IAM `action` on `resource` — the sentence
/// S3 words that refusal in — or `None` when they permit it.
///
/// `policy` is the bucket's own policy, `None` for a bucket carrying none, and `conditions`
/// the condition keys this request carries a value for
/// ([`authorization::ObjectWriteContext`], [`authorization::read_conditions`]). The decision
/// is the module docs' rule.
///
/// A caller that is another service's own SERVICE PRINCIPAL carries two condition keys more —
/// the resource it acts for and that resource's account
/// ([`authorization::service_principal_conditions`]) — because a grant to such a principal is
/// scoped by them, and it is refused in S3's own published wording
/// ([`state::ACCESS_DENIED_MESSAGE`]) rather than in the sentence IAM words about a USER:
/// there is no IAM identity to name in one.
///
/// ## A request made through an ACCESS POINT
///
/// An access point is a named data path onto one bucket, with a policy of its own, and that
/// policy is in force on the requests made THROUGH the access point and on no others
/// ("access point policies … are evaluated in addition to the bucket policy", Amazon S3 User
/// Guide, *Managing data access with Amazon S3 access points*). So for a request whose
/// address reached one ([`crate::state::access_points`] records which), this decision is
/// taken over TWO resources — the bucket-scoped `resource` the operation asked about, and
/// the access point's own resource for the same object
/// ([`crate::state::access_points::AccessPointRoute::resource_for`])
/// — against THREE documents: the identity's, the bucket's, and the access point's.
///
/// The rule is IAM's own, unchanged: an explicit `Deny` about the request in ANY of the
/// three refuses it, whatever any grant says — which is what makes an access point policy
/// that denies a role refuse that role's reads through the access point while its reads
/// addressed to the bucket are untouched, because no note is recorded for those and the
/// access point's policy is not among the documents in force on them. A grant is needed for
/// the path the request took: an `Allow` naming either resource, in the caller's
/// identity-based documents, in the bucket's policy or in the access point's.
///
/// An access point confined to a VPC is decided BEFORE any of that: it "will only allow
/// connections from the specified VPC ID" (`VpcConfiguration$VpcId`), so a request that
/// reached it from outside that VPC is refused for the NETWORK it came over, in the sentence
/// an explicit resource-policy deny is worded in — and no grant in any document lifts it.
#[must_use]
pub fn denial(
    caller: &Caller,
    policy: Option<&str>,
    action: &str,
    resource: &str,
    conditions: &[(String, String)],
) -> Option<String> {
    // The access point this request was addressed THROUGH, when its address reached one.
    let through = crate::state::access_points::addressed();
    // What a refusal names as the resource: the access point's own for a request that came
    // through one, which is the resource the decision about that path was taken over.
    let named = through
        .as_ref()
        .map_or_else(|| resource.to_string(), |route| route.resource_for(resource));
    // A call another service made as its own service principal is refused in S3's own
    // published wording: the sentence IAM words names a USER, and no IAM identity made it.
    let refusal = |reason: &str| {
        Some(if caller.service_principal.is_some() {
            state::ACCESS_DENIED_MESSAGE.to_string()
        } else {
            state::authorization_denial(&caller.arn, action, &named, reason)
        })
    };
    // A VPC-CONFINED access point answers no request that did not arrive from the VPC it is
    // bound to — "if this field is specified, this access point will only allow connections
    // from the specified VPC ID" (`VpcConfiguration$VpcId`). That is a restriction on the
    // network the path is reachable over, not a document, so it is decided HERE: before any
    // policy is read, and therefore not liftable by a grant in any of them — an `Allow` for
    // every principal in the access point's own policy leaves the refusal exactly as it is.
    // Which requests arrived outside that VPC is the store's one reading of it
    // ([`crate::state::access_points::addressed_outside_its_vpc`]).
    if crate::state::access_points::addressed_outside_its_vpc().is_some() {
        return refusal(state::RESOURCE_POLICY_DENY_REASON);
    }
    let prepared = policy.map(iam::PreparedDocument::read);
    let access_point_policy = through
        .as_ref()
        .and_then(|route| route.policy.as_deref())
        .map(iam::PreparedDocument::read);
    // The condition context the decision is taken over: what the REQUEST carried, plus the
    // global keys this gate holds for every request (who the caller is, which account it and
    // the resource belong to, and whether the credential that signed was MFA-backed), so a
    // statement written over one of those decides rather than
    // standing undecided. A request that came through an access point also answers the two
    // keys naming it, which is what a bucket policy that delegates to its access points is
    // written over.
    let mut context = conditions.to_vec();
    context.extend(authorization::global_conditions(&caller.identity_arn));
    context.extend(authorization::multi_factor_auth_conditions(
        caller.multi_factor_auth_present,
    ));
    // The session tags the principal carries, which is what an ABAC policy conditioned on
    // `aws:PrincipalTag/<key>` is decided against. The family is DECLARED only for a
    // session whose tags are known in full, so a clause over a tag the principal does not
    // carry is decided as absent there and stays undecided for a credential whose tags this
    // gate cannot read.
    for (key, value) in &caller.tags {
        context.push((
            format!("{PRINCIPAL_TAG_CONDITION_KEY_FAMILY}{key}"),
            value.clone(),
        ));
    }
    if let Some(route) = &through {
        context.extend(authorization::access_point_conditions(route));
    }
    // The keys this decision PUBLISHES: a service principal's call knows two more than any
    // other request does (the resource it acts for and that resource's account), and every
    // other request is judged against the list as it stands — borrowed rather than copied,
    // since that is the common case.
    let published: ::std::borrow::Cow<'_, [&str]> = match &caller.service_principal {
        None if !caller.tags_known => ::std::borrow::Cow::Borrowed(PUBLISHED_CONDITION_KEYS),
        None => {
            let mut published = PUBLISHED_CONDITION_KEYS.to_vec();
            published.push(PRINCIPAL_TAG_CONDITION_KEY_FAMILY);
            ::std::borrow::Cow::Owned(published)
        }
        Some(call) => {
            context.extend(authorization::service_principal_conditions(
                &call.source_arn,
                &call.source_account,
            ));
            let mut published = PUBLISHED_CONDITION_KEYS.to_vec();
            published.extend(authorization::SERVICE_PRINCIPAL_CONDITION_KEYS);
            if caller.tags_known {
                published.push(PRINCIPAL_TAG_CONDITION_KEY_FAMILY);
            }
            ::std::borrow::Cow::Owned(published)
        }
    };
    let conditions: &[(String, String)] = &context;
    let published: &[&str] = &published;
    // Every resource this request addresses: the bucket-scoped one the operation asked
    // about, and the access point's own when the request came through one.
    let mut addressed = vec![resource.to_string()];
    if named != resource {
        addressed.push(named.clone());
    }

    // An explicit deny wins over every grant, including the account's own credential — in
    // the bucket's own policy, and in the policy of the access point the request came
    // through.
    let denied_spellings = caller.denied_spellings();
    for document in [prepared.as_ref(), access_point_policy.as_ref()]
        .into_iter()
        .flatten()
    {
        let denies = addressed.iter().any(|resource| {
            let denying = request_over(action, resource, &denied_spellings, conditions, published);
            authorization::deny_is_decided(
                &iam::resource_outcome_prepared(document, &denying),
                published,
            )
        });
        if denies {
            return refusal(state::RESOURCE_POLICY_DENY_REASON);
        }
    }
    let documents = iam::prepare_documents(&caller.documents);
    let identity_said: Vec<iam::Outcome> = addressed
        .iter()
        .map(|resource| {
            iam::identity_outcome_prepared(
                &documents,
                &request_over(action, resource, &caller.principals, conditions, published),
            )
        })
        .collect();
    if identity_said
        .iter()
        .any(|said| authorization::deny_is_decided(said, published))
    {
        return refusal(IDENTITY_POLICY_DENY_REASON);
    }
    // What the SCOPE the credential was minted with says about this request, read by the
    // tree's one reading of a resolved scope (`sts_store::ScopingPolicies::decision`): a
    // session policy CAPS what the documents in force grant and grants nothing of its own, so
    // a request it explicitly denies is refused whatever any grant says — decided here,
    // before the grants are read, because no grant overturns it — and a request it does not
    // allow is refused once a grant is found, naming THEM as the document that withheld it.
    // A credential no mint scoped is capped by nothing.
    let scoped: Vec<Decision> = addressed
        .iter()
        .map(|resource| {
            caller.scope.decision(&request_over(
                action,
                resource,
                &caller.principals,
                conditions,
                published,
            ))
        })
        .collect();
    if scoped.contains(&Decision::Denied) {
        return refusal(iam::SESSION_POLICY_EXPLICIT_DENY);
    }
    // Whether the scope leaves the request to the documents in force on the identity: a
    // credential scoped by nothing readable is in that position for every request.
    let within_scope = scoped.contains(&Decision::Allowed);
    if identity_said
        .iter()
        .any(|said| said.decision == Decision::Allowed)
        || caller.identity_is_unbounded()
    {
        return if within_scope {
            None
        } else {
            refusal(&iam::no_session_policy_allows(action))
        };
    }
    // A grant in the BUCKET's own policy — or in the policy of the access point the request
    // came through — names this principal directly, which is a permission of its own (see
    // the module docs).
    let granted = |document: &iam::PreparedDocument| {
        addressed.iter().any(|resource| {
            iam::resource_outcome_prepared(
                document,
                &request_over(action, resource, &caller.principals, conditions, published),
            )
            .decision
                == Decision::Allowed
        })
    };
    if prepared.as_ref().is_some_and(granted) || access_point_policy.as_ref().is_some_and(granted) {
        return if within_scope {
            None
        } else {
            refusal(&iam::no_session_policy_allows(action))
        };
    }
    refusal(&format!(
        "because no identity-based policy allows the {action} action"
    ))
}

/// The reason clause IAM publishes for a request an identity-based document explicitly
/// denies — the identity half of [`state::RESOURCE_POLICY_DENY_REASON`].
const IDENTITY_POLICY_DENY_REASON: &str = "with an explicit deny in an identity-based policy";

/// The condition-key FAMILY the calling principal's session tags are named in, each member
/// after the tag key it carries: `aws:PrincipalTag/tenant` holds the value of the session
/// tag `tenant`.
const PRINCIPAL_TAG_CONDITION_KEY_FAMILY: &str = "aws:PrincipalTag/";

/// One decision's request: `action` on `resource`, asked for the principal spellings
/// `principals`, in the condition context `conditions`, declaring the keys `published`.
///
/// One place, so every leg of [`denial`] — the deny legs and the grant legs, over the bucket
/// resource and over the access point's own — asks about the same request and publishes the
/// same keys ([`PUBLISHED_CONDITION_KEYS`], plus the two a service principal's call knows).
fn request_over<'a>(
    action: &'a str,
    resource: &'a str,
    principals: &'a [String],
    conditions: &'a [(String, String)],
    published: &'a [&'a str],
) -> iam::Request<'a> {
    iam::Request::new(action, resource, principals)
        .with_context(conditions)
        .with_published_keys(published)
}

/// [`denial`], for one of the three operations that read, write or remove the BUCKET POLICY
/// itself.
///
/// "To ensure that bucket owners don't inadvertently lock themselves out of their own
/// buckets, the root principal in a bucket owner's Amazon Web Services account can perform
/// the `GetBucketPolicy`, `PutBucketPolicy`, and `DeleteBucketPolicy` API actions, even if
/// their bucket policy explicitly denies the root principal's access" (`PutBucketPolicy`'s
/// own documentation). The account's own published credential is that principal here (see the
/// module docs), so those three are never refused it — which is what keeps a document that
/// denies everything from making the bucket's policy unreadable and unremovable. Every other
/// principal is judged by the rule above.
#[must_use]
pub fn policy_operation_denial(
    caller: &Caller,
    policy: Option<&str>,
    action: &str,
    resource: &str,
    conditions: &[(String, String)],
) -> Option<String> {
    if caller.account_credential {
        return None;
    }
    denial(caller, policy, action, resource, conditions)
}

/// Why S3 refuses a read of `key` — the bytes or the metadata of the version the request
/// addressed — to the CREDENTIALED caller, or `None` when the policies in force leave the
/// read to it.
///
/// WHICH action the read is judged under is decided by the request and by what its address
/// resolved to, which is what keeps a refusal from saying whether the object is there:
///
/// * a read naming a `versionId` asks for [`GET_OBJECT_VERSION_ACTION`] and one naming none
///   for [`GET_OBJECT_ACTION`] (see those constants);
/// * an address holding NOTHING is answered by what the caller may learn about the bucket's
///   contents at all: "if the object that you request doesn't exist, the error that Amazon S3
///   returns depends on whether you also have the `s3:ListBucket` permission. If you have the
///   `s3:ListBucket` permission on the bucket, Amazon S3 returns an HTTP status code 404 Not
///   Found error. If you don't have the `s3:ListBucket` permission, Amazon S3 returns an HTTP
///   status code 403 Access Denied error" (`GetObject`'s own documentation) — so a caller
///   without [`LIST_BUCKET_ACTION`] is refused the read instead of being told the key is
///   absent, and one holding it is left the 404 to be answered.
///
/// Asked BEFORE anything about the key is reported.
#[must_use]
pub fn object_read_denial(
    caller: &Caller,
    bucket: &Bucket,
    name: &str,
    key: &str,
    version_id: Option<&str>,
    addressed: Option<&Object>,
) -> Option<String> {
    let conditions = authorization::read_conditions();
    let policy = bucket.policy.as_deref();
    if addressed.is_some() {
        return denial(
            caller,
            policy,
            get_object_action(version_id),
            &state::object_arn(name, key),
            &conditions,
        );
    }
    denial(
        caller,
        policy,
        LIST_BUCKET_ACTION,
        &bucket.arn(name),
        &conditions,
    )
}

/// The IAM action a read of `version_id` is authorized under: the version-scoped one for a
/// read that NAMES a version, and the plain one for a read that names none (see
/// [`GET_OBJECT_VERSION_ACTION`]).
///
/// One place, so every operation that READS an object's bytes — the read itself, the HEAD of
/// it, and the copy that reads its source — asks for the same permission for the same kind of
/// reference.
#[must_use]
pub fn get_object_action(version_id: Option<&str>) -> &'static str {
    match version_id {
        Some(_) => GET_OBJECT_VERSION_ACTION,
        None => GET_OBJECT_ACTION,
    }
}

/// The IAM action a delete of `version_id` is authorized under: the version-scoped one for a
/// delete that NAMES a version, and the plain one for a delete that names none (see
/// [`DELETE_OBJECT_VERSION_ACTION`]).
#[must_use]
pub fn delete_object_action(version_id: Option<&str>) -> &'static str {
    match version_id {
        Some(_) => DELETE_OBJECT_VERSION_ACTION,
        None => DELETE_OBJECT_ACTION,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A caller that holds an IAM identity and no policy at all: the position every signed
    /// request from an unprivileged principal is in.
    fn named_caller(documents: &[&str]) -> Caller {
        let arn = ::engine::emulator_arn("iam", "", "user/reader");
        Caller {
            account_principals: iam::account_principals(&arn),
            principals: vec![arn.clone()],
            arn,
            documents: documents.iter().map(|text| (*text).to_string()).collect(),
            ..Caller::default()
        }
    }

    /// A document, in the flavor its `Principal` element decides: a bucket policy names one,
    /// an identity policy names none.
    fn document(effect: &str, action: &str, resource: &str, principal: Option<&str>) -> String {
        let principal = match principal {
            Some(principal) => format!("\"Principal\":{{\"AWS\":\"{principal}\"}},"),
            None => String::new(),
        };
        format!(
            "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Effect\":\"{effect}\",{principal}\
             \"Action\":\"{action}\",\"Resource\":\"{resource}\"}}]}}"
        )
    }

    /// One row of the grant table: the case, the documents in force on the caller, the bucket
    /// policy, the action asked about and whether the request must be refused.
    type GrantRow<'a> = (&'a str, Vec<String>, Option<&'a str>, &'a str, bool);

    /// One row of the deny table: the case, the documents in force on the caller, the bucket
    /// policy and the reason clause the refusal must carry (`None` for a request that stands).
    type DenyRow<'a> = (&'a str, Vec<String>, Option<&'a str>, Option<&'a str>);

    /// RULE: a signed request needs a grant for the action it asks for, from the caller's own
    /// documents or from the bucket policy naming it; the rows differ along where the grant
    /// comes from and whether it covers the action asked about.
    #[test]
    fn a_grant_from_either_side_admits_and_nothing_else_does() {
        let caller = named_caller(&[]);
        let object = "arn:aws:s3:::reports/held.csv";
        let granted_identity = document("Allow", "s3:GetObject", object, None);
        let granted_resource = document("Allow", "s3:GetObject", object, Some(&caller.arn));
        let other_user = ::engine::emulator_arn("iam", "", "user/someone-else");
        let granted_to_another = document("Allow", "s3:GetObject", object, Some(&other_user));
        let rows: [GrantRow<'_>; 6] = [
            ("no document anywhere", Vec::new(), None, "s3:GetObject", true),
            (
                "an identity grant for the action",
                vec![granted_identity.clone()],
                None,
                "s3:GetObject",
                false,
            ),
            (
                "an identity grant for another action",
                vec![granted_identity.clone()],
                None,
                "s3:PutObject",
                true,
            ),
            (
                "a bucket-policy grant naming the caller",
                Vec::new(),
                Some(granted_resource.as_str()),
                "s3:GetObject",
                false,
            ),
            (
                "a bucket-policy grant naming another principal",
                Vec::new(),
                Some(granted_to_another.as_str()),
                "s3:GetObject",
                true,
            ),
            (
                "a bucket-policy grant for another action",
                Vec::new(),
                Some(granted_resource.as_str()),
                "s3:DeleteObject",
                true,
            ),
        ];
        for (case, documents, policy, action, refused) in rows {
            let caller = Caller {
                documents,
                ..caller.clone()
            };
            let answer = denial(&caller, policy, action, object, &[]);
            assert_eq!(
                answer.is_some(),
                refused,
                "case {case:?}: expected refused={refused}, got {answer:?}"
            );
        }
    }

    /// RULE: an explicit deny about the request refuses it whatever grants it, from either
    /// kind of document, and the sentence names which document denied.
    #[test]
    fn an_explicit_deny_overrides_every_grant() {
        let caller = named_caller(&[]);
        let object = "arn:aws:s3:::reports/held.csv";
        let grant_everything = document("Allow", "s3:*", "*", None);
        let identity_deny = document("Deny", "s3:GetObject", object, None);
        let resource_deny = document("Deny", "s3:GetObject", object, Some(&caller.arn));
        let account_deny = document(
            "Deny",
            "s3:GetObject",
            object,
            Some(&format!("arn:aws:iam::{}:root", ::engine::emulator_account_id())),
        );
        let rows: [DenyRow<'_>; 4] = [
            (
                "an identity deny beside an identity grant",
                vec![grant_everything.clone(), identity_deny],
                None,
                Some(IDENTITY_POLICY_DENY_REASON),
            ),
            (
                "a bucket-policy deny beside an identity grant",
                vec![grant_everything.clone()],
                Some(resource_deny.as_str()),
                Some(state::RESOURCE_POLICY_DENY_REASON),
            ),
            (
                "a bucket-policy deny of the whole account",
                vec![grant_everything.clone()],
                Some(account_deny.as_str()),
                Some(state::RESOURCE_POLICY_DENY_REASON),
            ),
            (
                "the grant alone",
                vec![grant_everything],
                None,
                None,
            ),
        ];
        for (case, documents, policy, clause) in rows {
            let caller = Caller {
                documents,
                ..caller.clone()
            };
            let answer = denial(&caller, policy, "s3:GetObject", object, &[]);
            match clause {
                Some(clause) => {
                    let message = answer.unwrap_or_default();
                    assert!(
                        message.ends_with(clause)
                            && message.starts_with(&format!("User: {} is not authorized", caller.arn)),
                        "case {case:?}: expected a refusal ending in {clause:?}, got {message:?}"
                    );
                }
                None => assert!(
                    answer.is_none(),
                    "case {case:?}: expected the grant to stand, got {answer:?}"
                ),
            }
        }
    }

    /// RULE: the account's own published credential and a principal carrying a grant this
    /// emulator cannot read are not bounded by the identity-based half, while an explicit
    /// deny still refuses both.
    #[test]
    fn the_callers_this_gate_does_not_bound() {
        let object = "arn:aws:s3:::reports/held.csv";
        let unbounded: [(&str, Caller); 2] = [
            (
                "the account's own published credential",
                Caller {
                    arn: sts_store::ambient_identity().arn,
                    account_credential: true,
                    ..Caller::default()
                },
            ),
            (
                "a principal carrying an unreadable attachment",
                Caller {
                    unreadable_attachment: true,
                    ..named_caller(&[])
                },
            ),
        ];
        for (case, caller) in unbounded {
            let admitted = denial(&caller, None, "s3:GetObject", object, &[]);
            assert!(
                admitted.is_none(),
                "case {case:?}: expected no refusal, got {admitted:?}"
            );
            let deny = document("Deny", "s3:GetObject", object, Some("*"));
            let refused = denial(&caller, Some(&deny), "s3:GetObject", object, &[]);
            assert!(
                refused.is_some(),
                "case {case:?}: expected the explicit deny to refuse, got {refused:?}"
            );
        }
    }

    /// RULE: which action a read and a delete are judged under is decided by whether the
    /// request names a version.
    #[test]
    fn naming_a_version_asks_for_the_version_scoped_action() {
        let rows: [(&str, Option<&str>, &str); 2] = [
            ("a delete naming no version", None, DELETE_OBJECT_ACTION),
            (
                "a delete naming a version",
                Some("3HL4kqtJlcpXroDTDmjVBH40Nrjfkd"),
                DELETE_OBJECT_VERSION_ACTION,
            ),
        ];
        for (case, version_id, expected) in rows {
            let action = delete_object_action(version_id);
            assert_eq!(
                action, expected,
                "case {case:?}: expected {expected}, got {action}"
            );
        }
    }
}
