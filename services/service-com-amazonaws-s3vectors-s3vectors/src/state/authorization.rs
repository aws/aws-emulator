// SPDX-License-Identifier: CC0-1.0

//! Whether IAM lets a request to this service perform the action it asks for.
//!
//! Two documents decide that, and both are read here, in IAM's own order.
//!
//! ## The caller's identity-based policies
//!
//! Every operation of this service documents the permission it takes ("you must have the
//! `s3vectors:ListVectors` permission to use this operation"), so a request is authorized
//! against the identity-based policies in force on the principal its credential names,
//! on the action `s3vectors:<OperationName>` and the resource it addresses, BEFORE the
//! operation reads or writes anything. A request no statement allows — IAM's default deny
//! — and one an explicit `Deny` covers are both refused with `AccessDeniedException`
//! (HTTP 403, a service-common error of this model), and nothing is performed. A
//! permissions boundary on the principal CAPS what those policies grant, so a grant the
//! boundary does not also allow is not a permission the principal has.
//!
//! Three callers this is not a bound on, for the reasons every other gate in this tree
//! leaves them unbounded:
//!
//! * the account's own published credential, which is not a principal the IAM store holds
//!   policies for — the position AWS account root credentials are in;
//! * a principal carrying an attachment whose document this emulator does not hold (an
//!   AWS-managed policy belongs to AWS rather than to this account), where refusing would
//!   deny access AWS permits. An explicit `Deny` in a document that CAN be read still
//!   refuses it, and a boundary the store does hold still caps it;
//! * an engine carrying no IAM store, where no principal can hold a policy at all.
//!
//! An expired session credential speaks for nobody: it carries no documents and no
//! principal spelling, so every action it asks for falls to the default deny.
//!
//! ## What the CREDENTIAL itself was scoped to
//!
//! A temporary credential may have been minted with session policies, which STS holds on
//! the session its access key id names ([`sts_store::session_scoping_documents`]) and which
//! are read by the tree's one reading of a resolved scope
//! ([`sts_store::ScopingPolicies::decision`]). They SCOPE the permissions of the identity
//! down and grant nothing of their own: "the resulting session's permissions are the
//! intersection of the role's identity-based policies and the session policies"
//! (`AssumeRoleRequest$Policy`). So a request those documents do not allow is refused even
//! where the identity's own documents allow it, and one only they allow is refused as if
//! they had allowed nothing — which is what makes the permission a call needs a property of
//! the CREDENTIAL rather than of the role alone. A long-term access key is scoped by none,
//! and a scope whose managed document this emulator does not hold scopes nothing, for the
//! reason an unreadable attachment bounds nothing.
//!
//! ## The vector bucket's own resource policy
//!
//! A vector bucket policy is a RESOURCE-BASED policy (`PutVectorBucketPolicy$policy` is
//! "the JSON that defines the policy"), so it is not a document this service only stores
//! and reports back: it is evaluated on every call that acts on that bucket or on
//! something inside it, at the instant of the call. Nothing is cached, so a policy just
//! put refuses immediately and one just deleted stops refusing immediately — the document
//! is read out of the bucket record the write left behind.
//!
//! ## One decision, from every document in force
//!
//! They are not separate decisions: [`Target::refusal`] takes IAM's ONE decision about the
//! request, in IAM's own order. An explicit `Deny` in any of them refuses whatever any
//! grant says; otherwise, within one account, an `Allow` from EITHER the caller's own
//! documents or the bucket's policy is enough
//! ([`::service_common::iam_authorization::is_allowed`]) — so a caller whose own policies
//! say nothing is served a call the bucket's policy allows it, and a caller the bucket's
//! policy says nothing about is served a call its own policies allow. With neither, the
//! request falls to IAM's default deny. A grant then has to clear the documents that only
//! CAP what the identity may do: the permissions boundary, and the session policies the
//! credential was minted with.
//!
//! The pieces, each held in exactly one place:
//!
//! 1. **who** — the principal the request's access key id speaks for, read from the SAME
//!    place `sts:GetCallerIdentity` answers from, so the principal a refusal blames is the
//!    one the caller reads back from STS for the credentials it signed with. Resolving it
//!    materialises the session expiries the clock has already made due, a write owed at
//!    that instant whether or not anybody called, which is why a REPORTING operation may
//!    make this decision: what a later caller sees is unchanged by the read itself.
//! 2. **the documents** — the caller's own, from the IAM store ([`caller`]), the session
//!    policies its credential was scoped with, from the STS session the access key id names
//!    ([`caller`] again), and the bucket record's stored policy
//!    ([`super::VectorBucketRecord`]), collected with the resources the request acts on by
//!    [`target`] and [`target_for_arn`].
//! 3. **the decision** — [`::service_common::iam_authorization`], the tree's ONE
//!    evaluation of policy documents against `(action, resource, principal)`, so this
//!    service and every other one handed the same documents decide alike.
//!
//! The decision is taken from the REQUEST — the ARNs come from the names it carried, and
//! the document from the bucket record only when the account holds that bucket — so it is
//! taken before anything is resolved, and a refusal tells an unauthorized caller nothing
//! about what the account holds.
//!
//! ## The operations a bucket policy is not evaluated for
//!
//! The policy operations themselves — PutVectorBucketPolicy, GetVectorBucketPolicy and
//! DeleteVectorBucketPolicy — are the bucket owner's, and this emulator serves them for
//! the account that owns the bucket. That is AWS's own escape hatch for a bucket policy
//! that locks everyone out ("if you accidentally deny everyone access to your bucket …
//! sign in as the root user … and delete the policy"), and it is what keeps a `Deny` on
//! the policy actions from making a bucket's policy unrecoverable. ListVectorBuckets is an
//! account-level action rather than one on a bucket, so it has no document to be decided
//! against either.
//!
//! That exemption is the RESOURCE policy's alone: the caller's own identity-based policies
//! are read for every one of those operations too, because a document that caps what the
//! caller may do cannot lock a bucket owner out of its own bucket.

use std::time::SystemTime;

use ::service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts_store;
use ::service_common::iam_authorization::{self as iam, Decision};

/// This service's IAM action prefix: an action is `s3vectors:<OperationName>`, which is
/// also this service's `arnNamespace` and signing name ([`super::ARN_SERVICE`]).
pub const ACTION_PREFIX: &str = super::ARN_SERVICE;

/// The `s3vectors:GetVectors` permission ListVectors ALSO requires when the caller asks
/// for vector data or metadata: "with only s3vectors:ListVectors permission, you can list
/// vector keys when returnData and returnMetadata are both set to false or not specified.
/// If you set returnData or returnMetadata to true, you must have both s3vectors:ListVectors
/// and s3vectors:GetVectors permissions" (ListVectors' own documentation).
pub const GET_VECTORS_OPERATION: &str = "GetVectors";

/// The `s3vectors:TagResource` permission a CREATE also requires when it carries tags:
/// "you must have the s3vectors:TagResource permission in addition to
/// s3vectors:CreateIndex permission to create a vector index with tags" (CreateIndex's own
/// documentation, and the same sentence on CreateVectorBucket). A create carrying no tags
/// writes no tags and stands on its own permission alone.
pub const TAG_RESOURCE_OPERATION: &str = "TagResource";

/// Which of IAM's rules refused a request its credential's own SESSION policies deny.
const SESSION_POLICY_DENY: &str = "with an explicit deny in a session policy";

/// What an authorization decision about one request is made from: the bucket's stored
/// resource policy, and the ARNs of the resources the request acts on.
///
/// The FIRST resource is the one the action is taken on — the index for a request that
/// addresses an index, the bucket otherwise — which is the resource a refusal names. The
/// rest are the ancestors a statement may name instead: a request that addresses an index
/// acts on the bucket that holds it too, so a statement naming the bucket is about the
/// vector data inside it — which is also why a statement carrying no `Resource` at all
/// covers the request (the shared evaluator's reading of a resource policy). Both
/// documents are read over the same ARNs, so a grant scoped to the bucket covers the
/// vectors in it whichever document states it.
#[derive(Debug, Clone, Default)]
pub struct Target {
    /// The bucket's policy document, or `None` when the bucket carries none — or when the
    /// account holds no such bucket, where there is no resource document to decide with.
    pub policy: Option<String>,
    /// The ARNs the decision is taken against, the resource the action is taken on first.
    /// Empty for an account-level action, which names no resource.
    pub resources: Vec<String>,
}

impl Target {
    /// The target of an ACCOUNT-LEVEL action: one that names no resource, so no statement
    /// about it names an ARN and no bucket's policy is about it.
    pub fn account() -> Self {
        Self::default()
    }

    /// The `AccessDeniedException` message (HTTP 403) when IAM does not permit `operations`
    /// on this target for `caller`, or `None` when it does.
    ///
    /// ONE decision per action, in IAM's own order: an explicit `Deny` in ANY document in
    /// force — this target's resource policy, the session policies the credential was
    /// minted with, or the caller's own identity-based documents — refuses whatever any
    /// grant says; otherwise an `Allow` from EITHER the caller's own documents or this
    /// target's resource policy grants it, and with neither the request falls to IAM's
    /// default deny. A grant still has to pass the two documents that only CAP what the
    /// identity may do and grant nothing themselves: the permissions boundary, and the
    /// session policies the credential was scoped with. The sentence says which of those
    /// rules refused it, because they need different fixes.
    ///
    /// `caller` is who the request speaks for, resolved by [`caller`] BEFORE this
    /// service's own store is locked, so only one store is ever held at a time; the
    /// decision itself reads nothing but the documents already collected, which the
    /// operations that accepted them have bounded.
    ///
    /// `operations` are this service's operation names — `Self::NAME` at the call site —
    /// which an IAM action names as `s3vectors:<OperationName>`. Several are passed where
    /// one call requires several permissions (see [`GET_VECTORS_OPERATION`] and
    /// [`TAG_RESOURCE_OPERATION`]), and a permission the caller is missing for ANY of them
    /// refuses the call.
    pub fn refusal(&self, caller: &Caller, operations: &[&str]) -> Option<String> {
        let acted_on = self.resources.first().map(String::as_str);
        for operation in operations {
            let action = format!("{ACTION_PREFIX}:{operation}");
            let (identity_said, resource_said) = self.decided(caller, &action);
            // What the SCOPE the credential was minted with says: a session policy caps
            // what the identity's documents grant and grants nothing of its own, so a
            // request it explicitly denies is refused whatever any grant says — taken
            // before the grants are read, because no grant overturns it.
            let scope_said = self.scoped(caller, &action);
            // The one decision a resource policy takes that nothing else can override.
            if resource_said == Decision::Denied {
                return Some(not_authorized(
                    &caller.arn,
                    &action,
                    acted_on,
                    RESOURCE_POLICY_DENY,
                ));
            }
            if scope_said == Decision::Denied {
                return Some(not_authorized(
                    &caller.arn,
                    &action,
                    acted_on,
                    SESSION_POLICY_DENY,
                ));
            }
            // Then the caller's own: an explicit `Deny` in a document that CAN be read
            // refuses the request whatever else the principal carries — including a grant
            // this emulator cannot read, which only lifts the DEFAULT deny.
            if identity_said == Decision::Denied {
                return Some(not_authorized(
                    &caller.arn,
                    &action,
                    acted_on,
                    IDENTITY_POLICY_DENY,
                ));
            }
            if !(iam::is_allowed(identity_said, resource_said) || caller.identity_unbounded) {
                return Some(not_authorized(
                    &caller.arn,
                    &action,
                    acted_on,
                    &format!("because no identity-based policy allows the {action} action"),
                ));
            }
            // "A permissions boundary sets the maximum permissions that an identity-based
            // policy can grant" (IAM, permissions boundaries): it caps what the CALLER's
            // own documents grant, so a grant this target's resource policy makes is not
            // one it bounds.
            if resource_said != Decision::Allowed {
                if let Some(reason) = self.boundary_refusal(caller, &action, acted_on) {
                    return Some(not_authorized(&caller.arn, &action, acted_on, &reason));
                }
            }
            // The grant still has to pass the scope the credential was minted with: a
            // request only the documents in force on the identity allow is refused as if
            // the session policies had allowed nothing, and the refusal names THEM as the
            // document that withheld the grant — a different fix from a missing grant.
            if scope_said != Decision::Allowed {
                return Some(not_authorized(
                    &caller.arn,
                    &action,
                    acted_on,
                    &format!("because no session policy allows the {action} action"),
                ));
            }
        }
        None
    }

    /// Which of IAM's rules the permissions boundary in force on `caller` refuses `action`
    /// by, or `None` when it permits it — or when the caller carries no boundary, which
    /// caps nothing.
    fn boundary_refusal(
        &self,
        caller: &Caller,
        action: &str,
        acted_on: Option<&str>,
    ) -> Option<String> {
        let boundary = caller.boundary.as_ref()?;
        let named = acted_on.unwrap_or(ANY_RESOURCE);
        let request = iam::Request::new(action, named, &caller.principals);
        match iam::identity_decision(::std::slice::from_ref(boundary), &request) {
            Decision::Allowed => None,
            Decision::Denied => Some(BOUNDARY_DENY.to_string()),
            Decision::NotStated => Some(format!(
                "because no permissions boundary allows the {action} action"
            )),
        }
    }

    /// What the session policies the request's credential was minted with say about
    /// `action` on this target.
    ///
    /// Read over the same ARNs the other two decisions are, and through the tree's ONE
    /// reading of a resolved scope ([`sts_store::ScopingPolicies::decision`]), so this
    /// service cannot cap a credential differently from any other service handed it: a
    /// `Deny` naming any resource the request acts on refuses, an `Allow` naming any of
    /// them passes the grant through, and a scope that states nothing about the action
    /// withholds it. A credential no mint scoped is capped by nothing, which that reading
    /// answers as allowed.
    fn scoped(&self, caller: &Caller, action: &str) -> Decision {
        let mut decided = Decision::NotStated;
        for named in self.named_resources() {
            let request = iam::Request::new(action, named, &caller.principals);
            match caller.scope.decision(&request) {
                Decision::Denied => return Decision::Denied,
                Decision::Allowed => decided = Decision::Allowed,
                Decision::NotStated => {}
            }
        }
        decided
    }

    /// The ARNs every decision about this target is taken over: the resources the request
    /// acts on, or — for an account-level action, which names none — the one element a
    /// statement about it can carry, which is also what a `Resource` of `*` matches.
    fn named_resources(&self) -> Vec<&str> {
        match self.resources.is_empty() {
            true => vec![ANY_RESOURCE],
            false => self.resources.iter().map(String::as_str).collect(),
        }
    }

    /// What the documents in force say about `action` on this target: the identity-based
    /// decision and the resource-based one.
    ///
    /// Each is taken over EVERY ARN the request acts on (see the field docs): a `Deny`
    /// naming any of them refuses, an `Allow` naming any of them grants, and a document
    /// about none of them states nothing.
    ///
    /// The principal spellings travel with the request because that is what a statement's
    /// `Principal` element is matched against, and the resource-based decision is taken
    /// from TWO sets of them, as [`iam::account_principals`] states the rule: a `Deny` is
    /// decided from the caller's own spellings AND the account ones (a statement that
    /// denies an account denies each of its principals), while a GRANT is decided from the
    /// caller's own alone — IAM reads a statement that allows an ACCOUNT as delegating to
    /// that account's identity-based policies rather than as a grant to each of its
    /// principals. An identity-based document states no `Principal` element at all, so its
    /// decision rests on the action and the resource.
    fn decided(&self, caller: &Caller, action: &str) -> (Decision, Decision) {
        let mut identity = Decision::NotStated;
        let mut resource = Decision::NotStated;
        let denied_by = caller.denied_spellings();
        for named in self.named_resources() {
            let request = iam::Request::new(action, named, &caller.principals);
            match iam::identity_decision(&caller.documents, &request) {
                Decision::Denied => identity = Decision::Denied,
                Decision::Allowed if identity != Decision::Denied => identity = Decision::Allowed,
                _ => {}
            }
            let Some(policy) = self.policy.as_deref() else {
                continue;
            };
            // The deny pass, over every spelling that can be denied; what it says about a
            // grant is not read, because its spellings are wider than a grant's.
            let denial = iam::Request::new(action, named, &denied_by);
            if iam::resource_decision(policy, &denial) == Decision::Denied {
                resource = Decision::Denied;
                continue;
            }
            // The grant pass, over the caller's own spellings alone.
            if resource != Decision::Denied
                && iam::resource_decision(policy, &request) == Decision::Allowed
            {
                resource = Decision::Allowed;
            }
        }
        (identity, resource)
    }
}

/// What the decision about a request addressing `bucket` — and the `index` inside it, when
/// the request addresses one — is made from.
///
/// The ARNs come from the NAMES the request carried and the Region it signed for, and the
/// document from the bucket record only when the account holds that bucket, so the decision
/// does not depend on whether the bucket or the index exists — which is what lets it be
/// taken before anything is resolved, and what keeps a refusal from telling an unauthorized
/// caller whether they do.
pub fn target(
    store: &super::ComAmazonawsS3vectorsS3vectorsState,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    bucket: &str,
    index: Option<&str>,
) -> Target {
    let region = super::request_region(ctx.parts());
    let mut resources = Vec::new();
    if let Some(index) = index {
        resources.push(super::index_arn(&region, bucket, index));
    }
    resources.push(super::bucket_arn(&region, bucket));
    Target {
        policy: store
            .buckets
            .get(bucket)
            .and_then(|record| record.policy.clone()),
        resources,
    }
}

/// [`target`] for an operation whose bucket's OWN policy is not evaluated for it — the
/// three policy operations, which are the bucket owner's escape hatch from a document that
/// locks everyone out (see the module docs).
///
/// The resource the action is taken on, with NO resource document, so the decision rests on
/// the caller's own policies alone: a `Deny` in a bucket policy cannot make that bucket's
/// policy unrecoverable, while a caller whose own policies do not allow the action is still
/// refused.
pub fn target_without_bucket_policy(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    bucket: &str,
) -> Target {
    Target {
        policy: None,
        resources: vec![super::bucket_arn(&super::request_region(ctx.parts()), bucket)],
    }
}

/// [`target`] for a request that names its resource by ARN — a tagging request, whose
/// `resourceArn` is either a vector bucket or a vector index.
///
/// The resource the action is taken on is the ARN as the request CARRIED it; a tagging
/// request about an index acts on the bucket that holds it too, and that bucket's policy is
/// the document in force. An ARN that is not one of this service's names no bucket, so it
/// is decided on the caller's own documents alone — its own refusal (the modelled 400)
/// comes later, from the operation that resolves it.
pub fn target_for_arn(store: &super::ComAmazonawsS3vectorsS3vectorsState, arn: &str) -> Target {
    let mut resources = vec![arn.to_string()];
    // A tagging request about an INDEX acts on the bucket that holds it too, and that
    // bucket's ARN is this one's own prefix — so the ancestor is read out of the value the
    // request carried rather than minted beside it.
    if let Some((bucket_arn, _)) = arn.split_once("/index/") {
        resources.push(bucket_arn.to_string());
    }
    let named = super::index_from_arn("resourceArn", arn)
        .map(|(bucket, _)| bucket)
        .or_else(|_| super::bucket_from_arn("resourceArn", arn))
        .ok();
    let policy = named.and_then(|bucket| {
        store
            .buckets
            .get(&bucket)
            .and_then(|record| record.policy.clone())
    });
    Target { policy, resources }
}

/// Which of IAM's rules refused a request whose RESOURCE policy denies it.
const RESOURCE_POLICY_DENY: &str = "with an explicit deny in a resource-based policy";

/// The same, for a `Deny` in one of the caller's own identity-based documents.
const IDENTITY_POLICY_DENY: &str = "with an explicit deny in an identity-based policy";

/// The same, for a `Deny` in the permissions boundary that caps them.
const BOUNDARY_DENY: &str = "with an explicit deny in a permissions boundary";

/// The refusal in the sentence AWS words an authorization failure in: who was refused,
/// which action, on which resource, and which of IAM's rules refused it. Every value comes
/// from the request, the caller's credential and the documents in force.
///
/// `resource` is `None` for an account-level action, which names no resource and whose
/// refusal therefore states no resource clause.
fn not_authorized(
    principal: &str,
    action: &str,
    resource: Option<&str>,
    reason: &str,
) -> String {
    match resource {
        Some(resource) => format!(
            "User: {principal} is not authorized to perform: {action} on resource: \
             {resource} {reason}"
        ),
        None => format!("User: {principal} is not authorized to perform: {action} {reason}"),
    }
}

/// The principal a request speaks for: the ARN a refusal blames, every spelling a
/// resource-policy statement may name it by, and what the account's IAM store says that
/// principal may do.
///
/// Everything the decision is made FROM, and none of the deciding: that is
/// [`Target::refusal`], which has both documents.
#[derive(Debug, Clone)]
pub struct Caller {
    /// The principal's ARN, as `sts:GetCallerIdentity` reports it.
    arn: String,
    /// The spellings a statement's `Principal` may GRANT to this caller by: its own ARN,
    /// and — for an assumed-role session — the ARN of the role it is a session of.
    principals: Vec<String>,
    /// The spellings that name this caller's ACCOUNT: its root user's ARN and the bare
    /// account id. A statement that DENIES an account denies each of its principals, while
    /// one that allows an account delegates to that account's identity-based policies
    /// rather than granting each principal — so these decide a deny and never a grant
    /// ([`iam::account_principals`]).
    account_principals: Vec<String>,
    /// The identity-based policy documents in force on the principal.
    documents: Vec<String>,
    /// The permissions boundary in force on it, when it carries one whose document this
    /// emulator holds.
    boundary: Option<String>,
    /// The session policy documents the mint of this request's credential scoped it with,
    /// as they read at the instant of this request, and whether a managed one among them
    /// names a document this emulator does not hold (see the module docs). Empty for a
    /// credential no mint scoped, which is a credential capped by nothing.
    scope: sts_store::ScopingPolicies,
    /// Set for a caller nothing in the IAM store can refuse once no readable document
    /// DENIES the action: the account's own published credential, which holds no IAM
    /// policies, a principal carrying an attachment whose document this emulator does not
    /// hold, and an engine with no IAM store at all (see the module docs).
    identity_unbounded: bool,
}

impl Caller {
    /// Every spelling a `Deny` about this caller may name it by: its own, and its
    /// account's.
    fn denied_spellings(&self) -> Vec<String> {
        let mut spellings = self.principals.clone();
        spellings.extend(self.account_principals.iter().cloned());
        spellings
    }

    /// Read what the IAM identity `identity_arn` names may do, through the IAM service's
    /// OWN accessors, where the collection rule itself lives — so a document
    /// `iam:PutUserPolicy` wrote takes effect on the next request and deleting it lifts it
    /// again.
    fn read_policies(&mut self, store: &sts_store::IamState, identity_arn: &str) {
        if let Some(policies) = store.principal_policies(identity_arn) {
            self.documents = policies.documents;
            // An attachment the collection resolved no document for is an entry of AWS's
            // own catalog this emulator does not carry: a grant it cannot read.
            self.identity_unbounded |= !policies.uncarried.is_empty();
        }
        self.boundary = store.permissions_boundary_document(identity_arn);
    }
}

/// The one element a statement about an action with no resource type can name, and what a
/// `Resource` of `*` matches.
const ANY_RESOURCE: &str = "*";

/// Who this request speaks for, resolved from the access key id it signed with.
///
/// Called BEFORE this service's own store is locked, so the STS and IAM stores this reads
/// are never reached while this service's state guard is held.
pub fn caller<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
) -> Caller {
    let access_key_id = ::engine::extract_akid(&ctx.parts().headers);
    let Some(sts) = sts_store::service_state(engine) else {
        // No STS store on this engine: the only identity a request can present is the
        // account's own published credential, which is this account's and holds no role.
        let identity = sts_store::ambient_identity();
        return Caller {
            principals: vec![identity.arn.clone()],
            account_principals: iam::account_principals(&identity.arn),
            arn: identity.arn,
            documents: Vec::new(),
            boundary: None,
            scope: sts_store::ScopingPolicies::default(),
            identity_unbounded: true,
        };
    };
    // A long-term access key speaks for the IAM user that holds it, which is the identity
    // STS itself reports for it; a key no store holds is the account's own published
    // credential. Both stores are read through STS's own accessors, so this service sees
    // exactly what `sts:GetCallerIdentity` would answer.
    let iam = sts_store::iam_state(engine);
    let long_term = match &iam {
        Some(iam) => sts_store::long_term_identity(Some(&iam.lock()), &access_key_id),
        None => sts_store::ambient_identity(),
    };
    let resolved = sts_store::resolve_request_caller(
        &mut sts.lock(),
        &access_key_id,
        SystemTime::now(),
        ctx.credential_store(),
        long_term,
    );
    // The IAM identity whose policies are in force on this request, filled in below. A
    // credential that speaks for nobody leaves it unset.
    let mut identity_arn: Option<String> = None;
    // The session policies the mint of this credential passed, if any: a long-term access
    // key, and a session minted without them, is scoped by none.
    let mut session_policies = sts_store::SessionPolicies::default();
    let mut caller = match resolved {
        // The spellings a statement may name this caller by, split as IAM reads them: its
        // own ARN — and, for an assumed-role session, the ARN of the role it is a session
        // of — can be GRANTED to, while its account's spellings can only DENY it.
        Ok(resolved) => {
            // Which IAM identity's policies are in force: an assumed-role session is
            // judged on the ROLE's ("the temporary security credentials … have the
            // permissions of the role", `AssumeRole`), a federated-user session on the
            // IAM USER whose long-term credentials minted it ("the session permissions
            // are the intersection of the IAM user policies and the session policies
            // that you pass", `GetFederationToken`), and any other credential on the
            // identity it speaks for, which is the IAM user that holds a long-term
            // access key — STS's own answer for which that is.
            identity_arn = Some(resolved.identity_policies_arn().to_string());
            // What the MINT of this credential scoped it to, which caps whatever those
            // policies grant; the documents it stands for are read below.
            session_policies = resolved.session_policies;
            let mut principals = vec![resolved.identity.arn.clone()];
            if let Some(role_arn) = &resolved.role_arn {
                principals.push(role_arn.clone());
            }
            Caller {
                // Two callers nothing in the IAM store bounds: the account's own published
                // credential, which is not a principal that store holds policies for, and
                // every caller on an engine carrying no IAM store at all, where no
                // principal can hold a policy (see the module docs).
                identity_unbounded: iam.is_none()
                    || resolved.identity.arn == sts_store::ambient_identity().arn,
                account_principals: iam::account_principals(&resolved.identity.arn),
                principals,
                arn: resolved.identity.arn,
                documents: Vec::new(),
                boundary: None,
                scope: sts_store::ScopingPolicies::default(),
            }
        }
        // An expired session credential speaks for nobody: no statement names it, so only
        // a statement naming every principal is about such a request, and every action it
        // asks for falls to the default deny. It is still blamed by the credential it
        // signed with.
        Err(expired) => Caller {
            arn: format!("an expired session credential ({})", expired.access_key_id),
            principals: Vec::new(),
            account_principals: Vec::new(),
            documents: Vec::new(),
            boundary: None,
            scope: sts_store::ScopingPolicies::default(),
            identity_unbounded: false,
        },
    };
    // What that principal may do, and what its credential was scoped to: both out of the
    // account's IAM store, read ONCE with the STS session registry's lock already given up,
    // so no handler holds two service stores at once. The scope is resolved at the instant
    // of the request, so a managed session policy edited since the mint scopes by what it
    // says NOW, while the inline document the mint passed scopes the credential whether or
    // not a store is there to read.
    let held = iam.as_ref().map(|iam| iam.lock());
    if let (Some(store), Some(identity_arn)) = (held.as_deref(), &identity_arn) {
        caller.read_policies(store, identity_arn);
    }
    caller.scope = sts_store::session_scoping_documents(held.as_deref(), &session_policies);
    caller
}
