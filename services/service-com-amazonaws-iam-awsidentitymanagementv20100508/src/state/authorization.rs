// SPDX-License-Identifier: CC0-1.0

//! Who a signed request to IAM speaks for, and whether that principal's own
//! identity-based policies — within the boundary that caps them — let it do what it asks.
//!
//! IAM is itself an AWS service whose API is governed by IAM: a call to it is authorized
//! against the calling principal's identity-based policies before the operation reports
//! or changes anything, and the answer is the one IAM's documented evaluation gives — an
//! explicit `Deny` whose statement is about the request wins over every `Allow`, an
//! `Allow` with no `Deny` permits, and a principal with neither is refused (the implicit
//! deny). A refused call is answered HTTP 403 with the error code `AccessDenied`
//! ([`crate::scaffolded::Scaffolded::access_denied`]) and a message built from the three
//! things the decision was made about — the principal, the action and the resource —
//! closed by why the decision went that way ([`not_authorized`]) — the refusal AWS
//! documents. For the few actions whose refusal AWS continues with the pointer at the
//! request's authorization details ([`NAMES_AUTHORIZATION_DETAILS`]), a refusal that
//! WITHHELD the call — nothing in force allowed it, the implicit deny — says where those
//! details are read and under which authorization id ([`not_authorized_with_details`]);
//! every other refusal, and any refusal explained by an explicit `Deny` (the statement the
//! reason already names IS the explanation), ends at the reason.
//!
//! The five pieces, each held in exactly one place:
//!
//! 1. **who** — the principal the request's SigV4 access key id speaks for: the IAM user
//!    that holds a LONG-TERM key, read out of
//!    [`super::ComAmazonawsIamAwsidentitymanagementv20100508State::access_keys`], the same
//!    map [`caller_identity`] resolves a nameless caller through, or — for a
//!    TEMPORARY credential — the role session that key was vended for, read out of
//!    [`super::ComAmazonawsIamAwsidentitymanagementv20100508State::role_sessions`], the
//!    record the vending operation leaves in this store;
//! 2. **what it may do** — the identity-based documents in force on that principal, from
//!    [`super::ComAmazonawsIamAwsidentitymanagementv20100508State::principal_policies`],
//!    which for a user is the UNION of the documents embedded in and attached to the user
//!    itself and those in force on every group it belongs to, and for a role session the
//!    ROLE's — "the temporary security credentials … have the permissions of the role"
//!    (`AssumeRole`) — so a policy just put grants immediately and one just deleted stops
//!    granting immediately;
//! 3. **the ceiling on it** — the permissions boundary in force on that principal, from
//!    [`super::ComAmazonawsIamAwsidentitymanagementv20100508State::permissions_boundary_in_force`],
//!    which sets the maximum permissions those documents can grant and grants nothing
//!    itself: a call the boundary does not allow is refused even where they allow it —
//!    and the refusal names the boundary as the document that withheld it, by the ARN of
//!    the policy that is the boundary where a statement of it explicitly denied the call —
//!    while a call only the boundary allows is refused as if the boundary had allowed
//!    nothing;
//! 4. **what the CREDENTIAL itself was scoped to** — the session policies the mint of a
//!    temporary credential passed, recorded on the role session beside the role
//!    ([`super::RoleSessionRecord::scope`]) and resolved to their documents at the instant
//!    of the call ([`super::session_scoping_documents`]). They SCOPE the permissions of the
//!    role down and grant nothing of their own — "the resulting session's permissions are
//!    the intersection of the role's identity-based policy and the session policies" — so
//!    they are read exactly as the boundary is ([`super::ScopingPolicies::decision`]), and a
//!    credential no mint scoped is capped by nothing;
//! 5. **the decision** — [`::service_common::iam_authorization`], the tree's ONE
//!    evaluation of IAM policy documents against `(action, resource)`, so IAM and every
//!    other service handed the same documents decide alike.
//!
//! ## What the decision knows about the caller, the target and the request
//!
//! A statement may be written to depend on WHO is calling, on the TARGET it is called on,
//! or on what the request itself writes, rather than only on the action asked for: through
//! a `Condition` clause over a condition key, or through a policy VARIABLE in its
//! `Resource` element, which the shared evaluation substitutes from the request's condition
//! context. That context is assembled here ([`request_context`]), out of the five things
//! IAM itself fills it from:
//!
//! * the SIGNING principal, resolved here before anything is decided — its friendly name,
//!   its unique id and its tags (`aws:PrincipalTag/<key>`), read at the instant of the
//!   call, so retagging the user changes what the same unchanged request decides;
//! * the TARGET the request addresses, by the ARN the decision is made against — its tags,
//!   under both spellings IAM publishes them as (`aws:ResourceTag/<key>` and the
//!   service-specific `iam:ResourceTag/<key>`), so an ABAC statement comparing a resource
//!   tag with a principal tag is decided from the two entities' stored tags;
//! * the REQUEST's own parameters, for each parameter IAM publishes as a condition key:
//!   for a call that writes tags, the tags it writes (`aws:RequestTag/<key>`) and the set
//!   of keys it writes (`aws:TagKeys`), which the `Tag*`, `Untag*` and `Create*` call
//!   sites hand the gate ([`WrittenTags`]); and for a call that attaches or detaches a
//!   MANAGED POLICY, the ARN of the policy it names (`iam:PolicyARN`), which those call
//!   sites hand the gate ([`authorize_policy_attachment`]);
//! * the CONNECTION the request arrived on — the address it was made from
//!   (`aws:SourceIp`), which is what a network clause is decided against
//!   ([`source_address`]);
//! * the CLOCK, which is what a date clause is decided against (`aws:CurrentTime`, and the
//!   same instant as epoch seconds in `aws:EpochTime`).
//!
//! Each of those keys is DECLARED to the evaluation
//! ([`::service_common::iam_authorization::Request::with_published_keys`]) where this gate
//! can answer for it — the resource-tag families only for a target whose tag set the
//! account holds — beside the keys a request reaching this decision structurally cannot
//! carry ([`PUBLISHED_ABSENT_KEYS`]). For a declared key the gate can answer whether the
//! request carries a value, including answering that it carries none, so IAM's documented
//! absent-key rule decides the clause: a positive comparison over such a key does not
//! hold, while the `IfExists` spelling, a negated comparison and `Null: {key: true}` do. A
//! clause over any OTHER key is left undecided, which withholds a grant and keeps a
//! refusal.
//!
//! ## The callers that are not IAM users
//!
//! * **A credential this account holds no identity for.** The emulator's own published
//!   credential is not a key this account's `access_keys` map holds, and no role session
//!   was vended for it. It carries no identity-based policies at all, which is the position
//!   AWS account root credentials are in, so such a request is allowed rather than refused
//!   on evidence that does not exist. The same holds for a session credential of an
//!   identity that is not a role of this account — a federated user — and for a role
//!   session whose role has since been deleted.
//! * **A principal holding a grant this emulator cannot read.** An AWS-managed policy is
//!   owned by AWS, so the account's store holds no document for it and
//!   `principal_policies` reports it as uncarried. A principal with such an attachment is
//!   allowed where the documents that ARE held neither grant nor deny: what that document
//!   grants is unknown here, and refusing on evidence this emulator does not have would
//!   deny access AWS permits. An explicit `Deny` in a document that is held still
//!   refuses, because no grant overturns one. A boundary whose document is not held is
//!   read the same way: it is a ceiling this emulator cannot measure a call against, so
//!   it caps nothing rather than refusing what AWS would permit.

use ::service_common::iam_authorization::{self as iam, Decision};

use crate::ops::types::{DelegationRequest, Role, Tag, User};
use crate::scaffolded::AccessDenied;

use super::{
    ComAmazonawsIamAwsidentitymanagementv20100508State as IamState, PrincipalPolicies,
    ScopingPolicies,
};

/// The `iam:<Operation>` action name each operation is authorized on, in the one list
/// this gate decides, re-exported so a handler names one as
/// `crate::state::authorization::<CONST>`.
pub mod actions;

/// Which IAM user a request that named none addresses — the other half of "who a signed
/// request speaks for", resolved from the same access key id this gate judges the call on
/// and read by every operation of this service whose `UserName` is optional.
pub mod caller_identity;

pub use actions::*;

/// The resource an account-WIDE call is decided against.
///
/// An action that reports the account's whole configuration is not taken on one
/// entity, and the Service Authorization Reference lists no resource type for such an
/// IAM action, so a statement permits it only by covering every resource — which is
/// what this value is matched by.
pub const ACCOUNT_WIDE_RESOURCE: &str = "*";

/// The decision value for a request the policies in force permit — one of the three
/// `PolicyEvaluationDecisionType` declares
/// (`smithy_query(query="shape",shape="PolicyEvaluationDecisionType")`).
pub const DECISION_ALLOWED: &str = "allowed";

/// The decision value for a request a statement explicitly denies.
pub const DECISION_EXPLICIT_DENY: &str = "explicitDeny";

/// The decision value for a request nothing allows and nothing explicitly denies.
pub const DECISION_IMPLICIT_DENY: &str = "implicitDeny";

/// The permission-check result for a caller that covers every ask of a delegation
/// request: "ALLOWED: The caller has sufficient permissions cover all the requested
/// permissions" (`GetDelegationRequestResponse$PermissionCheckResult`). One of the three
/// values `permissionCheckResultType` declares, which
/// `crate::ops::types::PERMISSION_CHECK_RESULT_TYPE_VALUES` records.
pub const PERMISSION_CHECK_ALLOWED: &str = "ALLOWED";

/// The permission-check result for a caller that does not: "DENIED: The caller does not
/// have sufficient permissions to cover all the requested permissions". See
/// [`PERMISSION_CHECK_ALLOWED`].
pub const PERMISSION_CHECK_DENIED: &str = "DENIED";

/// The permission-check result for a check that reaches neither answer: "UNSURE: It is
/// not possible to determine whether the caller has all the permissions needed". See
/// [`PERMISSION_CHECK_ALLOWED`].
pub const PERMISSION_CHECK_UNSURE: &str = "UNSURE";

/// The modelled name of the decision the two sets of documents come to, as
/// `EvaluationResult$EvalDecision` reports it.
///
/// The three values are the three outcomes IAM's evaluation has, so the mapping lives
/// beside the evaluation rather than in each operation that reports one: an explicit
/// `Deny` from either side is [`DECISION_EXPLICIT_DENY`], a grant from either side with
/// no such deny is [`DECISION_ALLOWED`], and neither is the implicit deny.
pub fn decision_name(identity: Decision, resource: Decision) -> &'static str {
    if identity == Decision::Denied || resource == Decision::Denied {
        DECISION_EXPLICIT_DENY
    } else if iam::is_allowed(identity, resource) {
        DECISION_ALLOWED
    } else {
        DECISION_IMPLICIT_DENY
    }
}

/// The condition key whose value is the signing IAM user's friendly name: "`aws:username`
/// — the friendly name of the current user", which for an IAM user is its name without
/// the path it lives under.
const USER_NAME_KEY: &str = "aws:username";

/// The condition key whose value is the signing principal's unique id: "`aws:userid` — the
/// unique identifier of the current user", which for an IAM user is the id the account
/// minted it under.
const USER_ID_KEY: &str = "aws:userid";

/// The prefix the condition key of ONE tag of the signing principal is spelled with:
/// "`aws:PrincipalTag/tag-key` — the tag attached to the principal making the request",
/// read from the principal's CURRENT tags, since nothing in the request carries them.
const PRINCIPAL_TAG_PREFIX: &str = "aws:PrincipalTag/";

/// The prefix the condition key of ONE tag of the resource the request ADDRESSES is
/// spelled with in IAM's own key namespace: "`iam:ResourceTag/tag-key` — works with the
/// tag attached to the IAM resource", the spelling an identity policy about an IAM
/// resource uses.
const IAM_RESOURCE_TAG_PREFIX: &str = "iam:ResourceTag/";

/// The GLOBAL spelling of the same key: "`aws:ResourceTag/tag-key` — compares the tag key-
/// value pair that you specify in the policy with the key-value pair attached to the
/// resource". Both name the tags of the resource the request is taken on, so both are
/// answered from the same tag set.
const AWS_RESOURCE_TAG_PREFIX: &str = "aws:ResourceTag/";

/// The condition keys a request to IAM MODELS about its SIGNING PRINCIPAL, whatever
/// values it then carries for them
/// (`::service_common::iam_authorization::Request::published`).
///
/// The signing principal is resolved before any decision is made, so for each of these
/// this gate can answer whether the request carries a value — including answering that it
/// carries none. [`PRINCIPAL_TAG_PREFIX`] is published as the whole key FAMILY it is,
/// because the principal's tags are known in full: a key of that family the context does
/// not carry is a tag the principal does not hold, not a key this emulator cannot read.
const PUBLISHED_PRINCIPAL_KEYS: &[&str] = &[USER_NAME_KEY, USER_ID_KEY, PRINCIPAL_TAG_PREFIX];

/// The two resource-tag key families, published — like [`PRINCIPAL_TAG_PREFIX`] — only
/// where the addressed resource's tag set is known in full, which is what lets a clause
/// over one of its tags be decided at all.
const PUBLISHED_RESOURCE_TAG_KEYS: &[&str] = &[IAM_RESOURCE_TAG_PREFIX, AWS_RESOURCE_TAG_PREFIX];

/// The prefix the condition key of ONE tag the REQUEST writes is spelled with:
/// "`aws:RequestTag/tag-key` — the tag key-value pair that is passed in the request", so a
/// grant can be conditioned on the value a tagging call is about to store.
const REQUEST_TAG_PREFIX: &str = "aws:RequestTag/";

/// The condition key carrying the SET of tag keys the request writes: "`aws:TagKeys` — the
/// tag keys that are passed in the request". Multi-valued, so a clause over it is decided
/// under one of IAM's set qualifiers.
const TAG_KEYS_KEY: &str = "aws:TagKeys";

/// The IAM-specific condition key carrying the ARN of the managed policy a request NAMES:
/// "`iam:PolicyARN` — the ARN of an IAM policy", compared under IAM's ARN operators.
///
/// It is the value of that request's own `PolicyArn` parameter, which is what lets a grant
/// be scoped to the managed policies a delegate may attach or detach rather than to the
/// identity it may attach them to. Filled — and declared — only by the call sites that
/// carry such a parameter and that IAM publishes the key for
/// ([`authorize_policy_attachment`]): for every other call it stays undeclared, because
/// absence from the context there would have to mean this gate knows the request carries
/// no policy ARN, which is a claim about the key's availability per action that is not
/// this gate's to make.
const POLICY_ARN_KEY: &str = "iam:PolicyARN";

/// The condition key carrying the ADDRESS the request was made from: "`aws:SourceIp` —
/// the requester's IP address", which is what an IP clause compares against.
const SOURCE_IP_KEY: &str = "aws:SourceIp";

/// The header a proxy in front of this emulator states the address it forwarded a request
/// for in, whose FIRST entry is the client's own — the same reading every other service in
/// this tree takes of a request's source address.
const FORWARDED_FOR_HEADER: &str = "x-forwarded-for";

/// The address a request carries when nothing forwarded it: this emulator serves its API
/// over the loopback interface, so a connection that reached it came from the local host.
const LOOPBACK_SOURCE_IP: &str = "127.0.0.1";

/// The condition key carrying the instant the request is being decided at:
/// "`aws:CurrentTime` — the date and time of the request", which is what a date clause
/// compares against.
const CURRENT_TIME_KEY: &str = "aws:CurrentTime";

/// The same instant in epoch seconds: "`aws:EpochTime` — the date and time of the request
/// in epoch or Unix time".
const EPOCH_TIME_KEY: &str = "aws:EpochTime";

/// The keys every request to this gate models about ITSELF: the tags it writes, the
/// address it was made from and the instant it is decided at.
///
/// Published for every call, because each is answered from the call itself — a request
/// that writes no tag carries no `aws:RequestTag/` and no `aws:TagKeys` value, which is
/// exactly what a clause over one of them is then decided from, while the connection and
/// the clock always have an answer.
const PUBLISHED_REQUEST_KEYS: &[&str] = &[
    REQUEST_TAG_PREFIX,
    TAG_KEYS_KEY,
    SOURCE_IP_KEY,
    CURRENT_TIME_KEY,
    EPOCH_TIME_KEY,
];

/// The address the request `parts` was made from, as `aws:SourceIp` carries it.
///
/// IAM fills that key from the CONNECTION, not from anything the request body says: the
/// first entry of [`FORWARDED_FOR_HEADER`] when a proxy stated the client it forwarded
/// for, and otherwise [`LOOPBACK_SOURCE_IP`], the host every connection to this emulator
/// is made from. So a statement gated on a network either admits or refuses a real signed
/// call, rather than leaving the clause undecided — and the value is never read off the
/// request's own parameters.
fn source_address(parts: &::http::request::Parts) -> String {
    parts
        .headers
        .get(FORWARDED_FOR_HEADER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(',').next())
        .map(str::trim)
        .filter(|address| !address.is_empty())
        .unwrap_or(LOOPBACK_SOURCE_IP)
        .to_string()
}

/// The condition keys a request that reaches THIS decision structurally cannot carry, so
/// that a clause over one of them is decided from its absence rather than left undecided.
///
/// This emulator holds no organization, so no principal of it is in one; and nothing
/// reaches it through a VPC endpoint, which is what the `aws:SourceVpce` family of keys
/// describes. Neither depends on what the request was signed with; the keys that describe
/// the CREDENTIAL are published separately ([`LONG_TERM_CREDENTIAL_ABSENT_KEYS`]).
///
/// Every OTHER key stays undeclared, and a clause over one of those is left UNDECIDED by
/// the evaluation — fail-closed in both directions — because for those absence from the
/// context means this emulator does not model the key rather than that the request carries
/// no value for it: a real signed request does carry `aws:PrincipalArn`,
/// `aws:RequestedRegion`, `aws:SecureTransport` and their peers, and this gate must not
/// decide a clause over them from their absence here.
const PUBLISHED_ABSENT_KEYS: &[&str] = &[
    "aws:PrincipalOrgID",
    "aws:PrincipalOrgPaths",
    "aws:SourceVpce",
    "aws:SourceVpc",
    "aws:VpcSourceIp",
];

/// The condition keys a request signed with an IAM user's LONG-TERM access key cannot
/// carry: it carries no multi-factor claim ("this key is not present when the request is
/// made using long-term credentials") and no session's issue time, source identity or
/// federating provider, because it is no session at all.
///
/// A request signed with a role session's TEMPORARY credential does carry values for these
/// — the session was issued at an instant, and may have been issued with a source identity
/// — and this gate holds no record of them, so for such a caller they stay undeclared and
/// a clause over one of them is left undecided, which withholds a grant and keeps a
/// refusal.
const LONG_TERM_CREDENTIAL_ABSENT_KEYS: &[&str] = &[
    "aws:MultiFactorAuthPresent",
    "aws:MultiFactorAuthAge",
    "aws:TokenIssueTime",
    "aws:SourceIdentity",
    "aws:FederatedProvider",
];

/// The tags a request WRITES, which is where `aws:RequestTag/<key>` and `aws:TagKeys` come
/// from.
///
/// The two forms a tagging call takes are the two IAM declares: a `Tag*` call writes
/// key-value pairs, and an `Untag*` call names keys alone — "`aws:TagKeys` … the tag keys
/// that are passed in the request", which both forms carry, while only the first carries
/// values. Every other operation writes no tags.
#[derive(Debug, Clone, Copy, Default)]
pub enum WrittenTags<'a> {
    /// A request that writes no tags at all, which is every operation but the tagging
    /// ones.
    #[default]
    None,
    /// A `Tag*` call: the key-value pairs it stores.
    Pairs(&'a [Tag]),
    /// An `Untag*` call: the keys it removes, which carry no value.
    Keys(&'a [String]),
}

impl WrittenTags<'_> {
    /// The `(key, value)` context entries these tags contribute: one `aws:RequestTag/<key>`
    /// per pair written, and one `aws:TagKeys` value per key written, whichever form the
    /// call takes.
    fn context(self) -> Vec<(String, String)> {
        match self {
            Self::None => Vec::new(),
            Self::Pairs(tags) => tags
                .iter()
                .flat_map(|tag| {
                    [
                        (format!("{REQUEST_TAG_PREFIX}{}", tag.key), tag.value.clone()),
                        (TAG_KEYS_KEY.to_string(), tag.key.clone()),
                    ]
                })
                .collect(),
            Self::Keys(keys) => keys
                .iter()
                .map(|key| (TAG_KEYS_KEY.to_string(), key.clone()))
                .collect(),
        }
    }
}

/// WHO a signed request speaks for, as the decision reads it: the entity whose own
/// condition keys describe the caller.
///
/// The two credentials that reach this gate carrying identity-based policies, which IAM
/// describes differently: an IAM user's own long-term access key, and the temporary
/// credential of a session of one of the account's roles, whose permissions are the
/// ROLE's.
#[derive(Debug, Clone, Copy)]
enum Caller<'a> {
    /// An IAM user of this account, resolved from the long-term access key it holds.
    User(&'a User),
    /// A session of one of the account's roles, resolved from the record the operation
    /// that vended the credential left ([`super::RoleSessionRecord`]): the role whose
    /// policies the call is judged on, and the session's own name.
    RoleSession {
        /// The role the session is of.
        role: &'a Role,
        /// The session name the vending call carried, which is what IAM spells into the
        /// session's unique id after the role's own.
        session_name: &'a str,
    },
}

/// The condition context a request by `caller` against `resource` carries, and the
/// keys it declares it models: everything IAM knows about the call at the instant it is
/// decided, which is where a policy variable's value and a `Condition` clause's value both
/// come from (see the module docs).
///
/// The keys it fills are the caller's own ([`PUBLISHED_PRINCIPAL_KEYS`]), the target's
/// ([`PUBLISHED_RESOURCE_TAG_KEYS`], both spellings of the one tag set), the request's own
/// tags and the clock's ([`PUBLISHED_REQUEST_KEYS`]). The resource-tag families are
/// DECLARED only for a resource whose tag set the account knows in full; for an ARN naming
/// nothing it holds, a clause over one of its tags stays undecided, which is the
/// fail-closed reading the shared evaluation gives every key this emulator cannot answer
/// for. The keys no such request carries at all are declared beside them
/// ([`PUBLISHED_ABSENT_KEYS`], and for a long-term key the ones that describe a session's
/// credential, [`LONG_TERM_CREDENTIAL_ABSENT_KEYS`]). A call that names a MANAGED POLICY
/// carries that ARN too, under the key IAM publishes it as ([`POLICY_ARN_KEY`]).
///
/// A ROLE SESSION is described by what IAM publishes for one: `aws:userid` is "the unique
/// identifier of the current user", which for an assumed-role session is the role's own id
/// and the session name after it, and there is no `aws:username` — a session is not a user,
/// so that key is left undeclared rather than answered as absent. Its principal TAGS are
/// the role's tags AND the session's, and the session's are not this store's to read, so
/// that family is left undeclared too: a clause over one of them stays undecided, which
/// withholds a grant and keeps a refusal.
///
/// The principal's and the target's tags are the ones they hold NOW — nothing in the
/// request carries them — so retagging either side changes what the same unchanged request
/// decides, and untagging it takes the value away again.
fn request_context(
    store: &IamState,
    caller: Caller<'_>,
    resource: &str,
    written: WrittenTags<'_>,
    policy_arn: Option<&str>,
    source_ip: String,
) -> (Vec<(String, String)>, Vec<&'static str>) {
    let mut context = Vec::new();
    let mut published = Vec::new();
    match caller {
        Caller::User(user) => {
            context.push((USER_NAME_KEY.to_string(), user.user_name.clone()));
            context.push((USER_ID_KEY.to_string(), user.user_id.clone()));
            published.extend_from_slice(PUBLISHED_PRINCIPAL_KEYS);
            for tag in user.tags.iter().flatten() {
                context.push((
                    format!("{PRINCIPAL_TAG_PREFIX}{}", tag.key),
                    tag.value.clone(),
                ));
            }
            published.extend_from_slice(LONG_TERM_CREDENTIAL_ABSENT_KEYS);
        }
        Caller::RoleSession { role, session_name } => {
            context.push((
                USER_ID_KEY.to_string(),
                format!("{}:{session_name}", role.role_id),
            ));
            published.push(USER_ID_KEY);
        }
    }
    // The tags of the entity the request addresses, under both spellings of the key: one
    // tag, two names for it. Declared only where that tag set is the account's to read.
    if let Some(tags) = store.resource_tags(resource) {
        for tag in tags {
            for prefix in PUBLISHED_RESOURCE_TAG_KEYS {
                context.push((format!("{prefix}{}", tag.key), tag.value.clone()));
            }
        }
        published.extend_from_slice(PUBLISHED_RESOURCE_TAG_KEYS);
    }
    context.extend(written.context());
    // The managed policy the request NAMES, for a call that carries one: a single value,
    // read off the request's own parameter rather than off anything stored, so an
    // `ArnEquals`/`ArnLike` clause scoping a grant to one policy is decided from the
    // policy the call actually asked about.
    if let Some(policy_arn) = policy_arn {
        context.push((POLICY_ARN_KEY.to_string(), policy_arn.to_string()));
        published.push(POLICY_ARN_KEY);
    }
    // Where the call came from, read from the connection the same way every request's
    // source address is read here ([`source_address`]).
    context.push((SOURCE_IP_KEY.to_string(), source_ip));
    let now = super::now();
    // The instant the call is decided at, in the two spellings IAM publishes it under.
    if let Ok(stated) = now.fmt(::aws_smithy_types::date_time::Format::DateTime) {
        context.push((CURRENT_TIME_KEY.to_string(), stated));
    }
    context.push((EPOCH_TIME_KEY.to_string(), now.secs().to_string()));
    published.extend_from_slice(PUBLISHED_REQUEST_KEYS);
    published.extend_from_slice(PUBLISHED_ABSENT_KEYS);
    (context, published)
}

/// Whether the principal that signed this request may perform `action` on `resource`.
///
/// `Ok(())` when the call is permitted; `Err` is the refusal the operation answers
/// through its error channel (HTTP 403, error code `AccessDenied`), worded from the
/// principal, the action and the resource the decision was made about — which this
/// spelling names by its ARN. An operation whose request addresses ONE entity by name
/// words it as that entity instead, through [`authorize_named`].
///
/// A READ: the store is only consulted, so authorizing a call changes nothing a later
/// caller sees.
pub fn authorize(
    store: &IamState,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    action: &str,
    resource: &str,
) -> Result<(), AccessDenied> {
    authorize_named(store, ctx, action, resource, resource)
}

/// [`authorize`], with the resource the refusal NAMES (`named_as`) stated separately from
/// the ARN the decision is made against (`resource`).
///
/// A denial reports "the resource the request was made on" ("Troubleshoot access denied
/// error messages", IAM User Guide), and for a request that addresses a single entity by
/// name that is the entity, not the collection it lives in — so such an operation words
/// it as the entity's TYPE followed by the name the request carried (`user Bob`), with the
/// type that disambiguates which entity a name belongs to. The ARN stays what the DECISION
/// is made against, because a policy statement's `Resource` matches ARNs.
pub fn authorize_named(
    store: &IamState,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    action: &str,
    resource: &str,
    named_as: &str,
) -> Result<(), AccessDenied> {
    authorize_written(store, ctx, action, resource, named_as, WrittenTags::None)
}

/// [`authorize_named`] for a call that WRITES tags, which hands the gate the tags it
/// writes so the clauses over them can be decided.
///
/// A `Tag*`/`Untag*` call's own input is part of the request IAM decides: `aws:RequestTag/`
/// and `aws:TagKeys` are read from it (see the module docs), so a grant conditioned on the
/// value about to be stored, or on the set of keys about to be written, applies to exactly
/// the combinations it names — and the refusal of any other leaves the target's tags
/// untouched, because the decision is made before the write.
pub fn authorize_written(
    store: &IamState,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    action: &str,
    resource: &str,
    named_as: &str,
    written: WrittenTags<'_>,
) -> Result<(), AccessDenied> {
    authorize_request(store, ctx, action, resource, named_as, written, None)
}

/// [`authorize_named`] for a call that NAMES A MANAGED POLICY — the managed-policy
/// attachment calls — which hands the gate that policy's ARN so a clause over
/// [`POLICY_ARN_KEY`] can be decided.
///
/// The ARN the request's `PolicyArn` parameter carries is part of the request IAM decides:
/// `iam:PolicyARN` is read from it, so a grant scoped to one managed policy permits
/// exactly the attach or detach that names it and withholds every other — which is how a
/// delegated administrator is given the one policy it may attach to an identity rather
/// than every policy the account holds. The decision is still made against the ARN of the
/// IDENTITY the attachment is taken on (`resource`), because that is the resource a
/// statement's `Resource` element matches, and it precedes the write, so a refusal leaves
/// the identity's attachments untouched.
pub fn authorize_policy_attachment(
    store: &IamState,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    action: &str,
    resource: &str,
    named_as: &str,
    policy_arn: &str,
) -> Result<(), AccessDenied> {
    authorize_request(
        store,
        ctx,
        action,
        resource,
        named_as,
        WrittenTags::None,
        Some(policy_arn),
    )
}

/// The ONE decision every spelling above reaches, handed everything the REQUEST itself
/// contributes to the condition context: the tags it writes and the managed policy it
/// names (see [`request_context`]).
fn authorize_request(
    store: &IamState,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    action: &str,
    resource: &str,
    named_as: &str,
    written: WrittenTags<'_>,
    policy_arn: Option<&str>,
) -> Result<(), AccessDenied> {
    let access_key_id = ::engine::extract_akid(&ctx.parts().headers);
    // A credential this account holds no IAM user for carries no identity-based
    // policies, so there is nothing to judge it on (see the module docs).
    let Some(signer) = signing_principal(store, &access_key_id) else {
        return Ok(());
    };
    let SigningPrincipal {
        caller,
        arn: principal,
        policies,
        scope,
    } = signer;
    let (context, published) = request_context(
        store,
        caller,
        resource,
        written,
        policy_arn,
        source_address(ctx.parts()),
    );

    let principals = [principal.clone()];
    // What the policies in force are decided against BESIDES the action and the resource:
    // the keys that describe the caller, the target and the request itself, which a
    // statement reads either through a `Condition` clause or through a policy variable in
    // its `Resource` element.
    let request = iam::Request::new(action, resource, &principals)
        .with_context(&context)
        .with_published_keys(&published);
    let decided = iam::identity_decision(&policies.documents, &request);
    // What the SCOPE the credential itself was minted with says about the call
    // ([`ScopingPolicies::decision`]). It caps what the identity's documents grant and
    // grants nothing of its own, so it is read for every call and a credential no mint
    // scoped is capped by nothing.
    let scoped = scope.decision(&request);
    // Whether the documents in force grant the call at all. An attachment whose
    // document the account does not hold is a grant this emulator cannot read, so it
    // cannot be the basis of a refusal — but it cannot overturn one either: an explicit
    // `Deny` in a document that IS held wins over every grant, including one whose
    // content is unknown here.
    let granted = iam::is_allowed(decided, Decision::NotStated)
        || (decided != Decision::Denied && !policies.uncarried.is_empty());
    // What the permissions boundary in force on the principal says about the call. "A
    // permissions boundary … sets the maximum permissions that an identity-based policy
    // can grant to an entity": the boundary CAPS what those policies grant and grants
    // nothing itself, so a call it does not allow is refused even where they allow it,
    // and a call only it allows is refused as if it had allowed nothing. A principal
    // carrying no boundary — or one whose document is not held, a ceiling this emulator
    // cannot read — is capped by nothing, which is the same position as a boundary that
    // allows the call. A call an identity-based policy explicitly DENIES is already
    // refused on that deny, so no ceiling is read for it.
    let (capped, boundary) = if decided == Decision::Denied {
        (Decision::Allowed, None)
    } else {
        match store.permissions_boundary_in_force(&principal) {
            Some((boundary, document)) => (
                iam::identity_decision(::std::slice::from_ref(&document), &request),
                Some(boundary),
            ),
            None => (Decision::Allowed, None),
        }
    };
    if granted && capped == Decision::Allowed && scoped == Decision::Allowed {
        return Ok(());
    }

    // Why the call was refused: an explicit `Deny` that is about the request — stated in
    // an identity-based policy, in a session policy or in the boundary, and each wins over
    // every `Allow` — or the absence of an `Allow`, which names the DOCUMENT that withheld
    // it: the boundary where the identity-based policies did allow the call, the session
    // policies where the identity's did and the scope the credential was minted with did
    // not, and the identity-based policies where nothing allowed it at all. They are
    // different facts about the policies in force, and AWS words each of them differently,
    // because they need different fixes. The boundary's explicit deny is the one that also
    // names the policy the deciding statement came from, because in an account carrying
    // several boundaries that ARN is the only thing in the message identifying which
    // ceiling refused the call.
    let reason = if decided == Decision::Denied {
        "with an explicit deny in an identity-based policy".to_string()
    } else if scoped == Decision::Denied {
        "with an explicit deny in a session policy".to_string()
    } else if capped == Decision::Denied {
        match &boundary {
            Some(arn) => format!("with an explicit deny in a permissions boundary: {arn}"),
            None => "with an explicit deny in a permissions boundary".to_string(),
        }
    } else if !granted {
        format!("because no identity-based policy allows the {action} action")
    } else if scoped != Decision::Allowed {
        format!("because no session policy allows the {action} action")
    } else {
        format!("because no permissions boundary allows the {action} action")
    };
    // Audit record for the refusal: the acting principal, what it asked for and the
    // decision, so the refusal is reviewable after the fact. The policy documents
    // themselves are not logged.
    ::tracing::info!(
        audit = true,
        outcome = "refused_access_denied",
        principal = %principal,
        action = %action,
        resource = %resource,
        explicit_deny = decided == Decision::Denied
            || scoped == Decision::Denied
            || capped == Decision::Denied,
        "the policies in force on the caller do not permit this call"
    );
    // AWS's refusal is the sentence that ENDS at the reason, which is what this gate
    // answers unless the call asked for one of the actions whose refusal AWS CONTINUES
    // with the pointer at the request's authorization details
    // ([`NAMES_AUTHORIZATION_DETAILS`]). WHICH of the two wordings a refusal carries is a
    // fact about the ACTION alone: the evaluation whose details the pointer names ran
    // whatever the decision was, so a refusal explained by an explicit `Deny` carries the
    // pointer exactly as one that withheld a grant does — recorded behaviour for a
    // `GetUser` an identity-based statement explicitly denies, whose message carries the
    // pointer after the explicit-deny reason.
    if NAMES_AUTHORIZATION_DETAILS.contains(&action) {
        return Err(not_authorized_with_details(
            &principal,
            action,
            named_as,
            &reason,
            &authorization_id(ctx, &principal, action, resource),
        ));
    }
    Err(not_authorized(&principal, action, named_as, &reason))
}

/// The actions whose refusal AWS CONTINUES with the pointer at the request's authorization
/// details, instead of ending at the reason the decision went that way.
///
/// The refusal AWS documents ("Troubleshoot access denied error messages", IAM User Guide)
/// is the single sentence naming the principal, the action, the resource and why — which
/// is what [`not_authorized`] words and what EVERY action here is refused with: this list
/// is the positive evidence for the continuation, so an action nobody has observed the
/// pointer for is answered the documented sentence rather than a continuation nothing
/// supports. Which of the two an action carries is a fact about THAT action — nothing
/// about the caller, the resource, the Region, the number of attempts or whether the call
/// reads or writes selects it — and the `AccessDenied` of an authorization refusal is
/// service-common, its text modelled nowhere (`smithy_query(query="operation",
/// operation="ListUsers,ListAccessKeys")` declares only `ServiceFailureException` and, for
/// a request naming an entity, `NoSuchEntityException`), so the continuation cannot be read
/// off the model either.
///
/// SOURCE, for adding an entry: the message the service answers that action's withheld
/// call with, where it carries the pointer.
/// `tests/all/access_denied_message_wording.rs` is the table that reads both wordings back.
const NAMES_AUTHORIZATION_DETAILS: &[&str] = &[
    GENERATE_CREDENTIAL_REPORT,
    GET_CREDENTIAL_REPORT,
    GET_OUTBOUND_WEB_IDENTITY_FEDERATION_INFO,
    GET_USER,
    // The two reads of what a USER embeds, each recorded answering its withheld call with
    // the pointer — the same wording their role counterparts below are recorded with, which
    // is why each is here per the SOURCE above rather than derived from the other.
    GET_USER_POLICY,
    LIST_USER_POLICIES,
    LIST_ACCESS_KEYS,
    LIST_ATTACHED_ROLE_POLICIES,
    LIST_OPEN_ID_CONNECT_PROVIDERS,
    LIST_ROLE_POLICIES,
    LIST_ROLE_TAGS,
    LIST_USER_TAGS,
    PUT_ACCOUNT_PROPERTIES,
    SET_SECURITY_TOKEN_SERVICE_PREFERENCES,
    TAG_USER,
];

/// Where the authorization details of ONE refused request are read: the console page for
/// an authorization id.
///
/// The host and the Region parameter are IAM's own endpoint Region whichever Region the
/// caller addressed, because IAM is a global service: there is one place its authorization
/// details are served from, so a refusal answered to a caller elsewhere still points
/// there.
///
/// LIMITATION, stated so the pointer is not mistaken for more than it is: this gate names
/// the id of the refusal it is answering, and nothing here records the evaluation behind
/// it, so a reader that follows the id into the authorization-details reader is told the
/// details are not available. Recording them is the work of the operation that serves
/// them, not of the refusal that names one.
const AUTHORIZATION_DETAILS_PAGE: &str =
    "https://us-east-1.console.aws.amazon.com/iam/home?region=us-east-1#/authorization-details/";

/// [`not_authorized`], closed by the pointer to the authorization details of the
/// evaluation that withheld the call: the page those details are read on, and the
/// authorization id a reader of them takes.
fn not_authorized_with_details(
    principal: &str,
    action: &str,
    named_as: &str,
    reason: &str,
    authorization_id: &str,
) -> AccessDenied {
    AccessDenied::new(format!(
        "User: {principal} is not authorized to perform: {action} on resource: {named_as} \
         {reason}. Go to {AUTHORIZATION_DETAILS_PAGE}{authorization_id} for complete \
         details, or call the GetRequestAuthorizationDetails API with the following \
         authorization id: {authorization_id}"
    ))
}

/// The authorization id of THIS request: the handle its refusal points at.
///
/// Derived from what the request itself carries — the instant it was signed, the digest of
/// its content, and what the decision was made about — rather than drawn at random, so two
/// refusals over different content, at different instants or about different subjects are
/// pointed at under different ids, and nothing has to be remembered for an id to stay the
/// id of that refusal. It is a FUNCTION of those facts and nothing else: two refusals that
/// agree on every one of them — the same caller refused the same call on the same content
/// within the same second — are pointed at under one id, which is the determinism the
/// wording this gate answers with is asserted on. The credential and signature a request is
/// signed with are deliberately NOT part of it: the id travels back to the caller, so
/// nothing derived from signing material goes into it.
///
/// Rendered by the tree's ONE derivation of an id from a seed
/// ([`::service_common::caller_principal::aws_unique_id`]), in the lower case an
/// authorization id is spelled in; it carries no type prefix, because an authorization id
/// names a request rather than an entity.
fn authorization_id(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    principal: &str,
    action: &str,
    resource: &str,
) -> String {
    /// The header fields that say WHEN a request was signed and WHAT it carried, neither
    /// of which is credential material.
    const REQUEST_HEADERS: &[&str] = &["x-amz-date", "x-amz-content-sha256"];
    /// The separator between the seed's parts, so two different splittings of the same
    /// text cannot seed the same id.
    const SEPARATOR: char = '\n';

    let headers = &ctx.parts().headers;
    let mut seed = String::new();
    for header in REQUEST_HEADERS {
        if let Some(value) = headers.get(*header).and_then(|value| value.to_str().ok()) {
            seed.push_str(value);
        }
        seed.push(SEPARATOR);
    }
    for part in [principal, action, resource] {
        seed.push_str(part);
        seed.push(SEPARATOR);
    }
    ::service_common::caller_principal::aws_unique_id("", &seed).to_lowercase()
}

/// The `AccessDenied` text AWS words a call it will not perform for its caller with:
/// what the refusal is about — the principal that signed the request, the action it
/// asked for and the resource it asked for it on — closed by `reason`, which says why.
///
/// ONE wording for the code, so a refusal an operation reaches for a reason of its own
/// is worded exactly as the policy decision above is, differing only in that reason.
/// Every part of it is a fact of the request and of the account's state, so the same
/// refused call is worded the same on every run and in every Region.
/// [`not_authorized_with_details`] is this sentence continued by the pointer at the
/// authorization details of an evaluation that withheld a grant.
pub fn not_authorized(principal: &str, action: &str, named_as: &str, reason: &str) -> AccessDenied {
    AccessDenied::new(format!(
        "User: {principal} is not authorized to perform: {action} on resource: {named_as} \
         {reason}"
    ))
}

/// Whether the caller has sufficient permissions to cover the asks of the delegation
/// request `delegated` — the permission check `GetDelegationRequest` performs when its
/// request asks for one — as one of the three values `permissionCheckResultType`
/// declares (`super::super::ops::types::PERMISSION_CHECK_RESULT_TYPE_VALUES`).
///
/// The asks are the `(action, resource)` pairs the request's permission policy names
/// ([`super::delegated_asks`]), and the question asked about each of them is the one
/// [`authorize_named`] asks about the call itself, decided against the SAME documents by
/// the same evaluation: the caller covers an ask when its identity-based policies allow
/// that action on that resource.
///
/// * [`PERMISSION_CHECK_DENIED`] as soon as a statement explicitly denies an ask, since
///   no grant overturns an explicit deny, and for a caller whose documents simply do not
///   cover every ask.
/// * [`PERMISSION_CHECK_ALLOWED`] when every ask is covered.
/// * [`PERMISSION_CHECK_UNSURE`] — "it is not possible to determine whether the caller
///   has all the permissions needed" — where the evidence is incomplete rather than
///   short: asks that cannot be enumerated from the request, a caller holding a grant
///   whose document the account does not hold (see the module docs), and a caller whose
///   grants are conditioned, which the documented case for this value ("most likely for
///   cases when the caller has permissions with conditions") names.
///
/// A READ, like every other decision here: the answer comes from the documents in force
/// at the instant of the call and nothing is written, so asking twice answers twice the
/// same.
pub fn delegation_permission_check(
    store: &IamState,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    delegated: &DelegationRequest,
) -> &'static str {
    let Some(asks) = delegated
        .permission_policy
        .as_deref()
        .and_then(super::delegated_asks)
    else {
        return PERMISSION_CHECK_UNSURE;
    };

    let access_key_id = ::engine::extract_akid(&ctx.parts().headers);
    // A credential this account holds no IAM user for carries no identity-based policies
    // at all — the position account root credentials are in (see the module docs) — so
    // there is nothing its permissions can fall short of.
    let Some(signer) = signing_principal(store, &access_key_id) else {
        return PERMISSION_CHECK_ALLOWED;
    };
    let SigningPrincipal {
        caller,
        arn: principal,
        policies,
        scope,
    } = signer;

    let principals = [principal];
    let documents = iam::prepare_documents(&policies.documents);
    let mut short_of_an_ask = false;
    for (action, resource) in &asks {
        // The same question [`authorize_written`] asks, so it is asked with the same
        // knowledge of the caller, of the ask's own target, of the connection and of the
        // clock (see [`request_context`]). A permission check writes no tags, so the ask
        // carries none either.
        let (ask_context, published) = request_context(
            store,
            caller,
            resource,
            WrittenTags::None,
            // An ask is an `(action, resource)` pair read out of a policy document, not a
            // call: it carries no request parameter, so no managed policy ARN either.
            None,
            source_address(ctx.parts()),
        );
        let request = iam::Request::new(action, resource, &principals)
            .with_context(&ask_context)
            .with_published_keys(&published);
        let decided = iam::identity_decision_prepared(&documents, &request);
        if decided == Decision::Denied {
            return PERMISSION_CHECK_DENIED;
        }
        // The scope the CREDENTIAL was minted with caps every ask the same way it caps a
        // call this gate decides ([`ScopingPolicies::decision`]): an ask the session
        // policies deny is a denied ask, and one they merely do not allow is an ask the
        // caller is short of.
        let scoped = scope.decision(&request);
        if scoped == Decision::Denied {
            return PERMISSION_CHECK_DENIED;
        }
        if !iam::is_allowed(decided, Decision::NotStated) || scoped != Decision::Allowed {
            short_of_an_ask = true;
        }
    }
    if !short_of_an_ask {
        return PERMISSION_CHECK_ALLOWED;
    }

    // The caller is short of an ask on the evidence that could be read. Where part of
    // that evidence could NOT be — a document the account does not hold, or a grant
    // whose condition is not decided here — what is known is that the answer is not
    // known, which is the third value rather than a refusal.
    let conditioned = !super::context_keys_in_policies(
        policies.documents.iter().map(::std::string::String::as_str),
    )
    .is_empty();
    if conditioned || !policies.uncarried.is_empty() {
        PERMISSION_CHECK_UNSURE
    } else {
        PERMISSION_CHECK_DENIED
    }
}

/// What a signed request is judged on, resolved once from the access key it carries:
/// the principal it speaks for and the identity-based documents in force on that
/// principal.
#[derive(Debug)]
struct SigningPrincipal<'a> {
    /// The principal the condition keys describing the caller are read from
    /// ([`request_context`]).
    caller: Caller<'a>,
    /// The ARN of that principal — what a refusal names as the caller, and the principal
    /// a resource-based statement would name. For a role session that is the SESSION's
    /// ARN, which is what `sts:GetCallerIdentity` reports for the credential and so what
    /// the caller reads back for the key it signed with.
    arn: String,
    /// The identity-based documents in force on it, and the attachments whose documents
    /// the account does not hold.
    policies: PrincipalPolicies,
    /// The session policy documents the mint of this credential scoped it with, as they
    /// read at the instant of this request. Empty for a credential no mint scoped — every
    /// long-term access key, and every session minted without session policies.
    scope: ScopingPolicies,
}

/// The principal the access key id `access_key_id` speaks for, or `None` when the account
/// holds no identity for that key.
///
/// Two credentials carry identity-based policies: an IAM user's own long-term access key,
/// resolved through the registry `CreateAccessKey` writes, and the temporary credential of
/// a session of one of the account's roles, resolved through the record the operation that
/// vended it left ([`super::ComAmazonawsIamAwsidentitymanagementv20100508State::role_session`]).
/// A session is judged on the ROLE's policies — "the temporary security credentials …
/// have the permissions of the role" (`AssumeRole`) — while the refusal names the session,
/// because that is the principal the request speaks for.
fn signing_principal<'a>(
    store: &'a IamState,
    access_key_id: &str,
) -> Option<SigningPrincipal<'a>> {
    if let Some(record) = store.access_keys.get(access_key_id) {
        let user = store.users.get(&record.user_key)?;
        let policies = store.principal_policies(&user.arn)?;
        return Some(SigningPrincipal {
            caller: Caller::User(user),
            arn: user.arn.clone(),
            policies,
            // A long-term key is the identity's own credential: no mint scoped it.
            scope: ScopingPolicies::default(),
        });
    }
    let session = store.role_session(access_key_id, super::now())?;
    // The role the session is of, addressed by the ARN the account holds it under: a
    // role deleted since the session was vended leaves the session speaking for an
    // identity this account no longer holds policies for.
    let role_name = ::service_common::iam_role::role_name(&session.role_arn)?;
    let role = store.roles.get(&super::name_key(role_name))?;
    let policies = store.principal_policies(&role.arn)?;
    Some(SigningPrincipal {
        caller: Caller::RoleSession {
            role,
            session_name: ::service_common::iam_role::assumed_role_session_name(
                &session.session_arn,
            )?,
        },
        arn: session.session_arn.clone(),
        policies,
        // What the CREDENTIAL itself was scoped to, resolved at the instant of the request
        // so a managed session policy edited since the mint scopes by what it says NOW.
        scope: super::session_scoping_documents(Some(store), &session.scope),
    })
}
