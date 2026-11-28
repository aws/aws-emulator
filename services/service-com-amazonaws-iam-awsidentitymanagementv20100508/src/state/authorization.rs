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
//! closed by why the decision went that way and then, for the refusals whose recordings
//! carry it ([`details::DeniedRequest::points_at_its_detail`]), by the pointer at that
//! decision's own authorization detail ([`details`]), which is where the sentence ENDS,
//! whichever action was asked for and whichever document decided it.
//!
//! The five pieces, each held in exactly one place:
//!
//! 1. **who** — the principal the request's SigV4 access key id speaks for: the IAM user
//!    that holds a LONG-TERM key, read out of
//!    [`super::ComAmazonawsIamAwsidentitymanagementv20100508State::access_keys`], the same
//!    map [`caller_identity`] resolves a nameless caller through, or — for a
//!    TEMPORARY credential — the role session that key was vended for, read out of
//!    [`super::ComAmazonawsIamAwsidentitymanagementv20100508State::role_sessions`], the
//!    record the vending operation leaves in this store, and failing that the principal the
//!    mint recorded beside the credential's class
//!    ([`super::ComAmazonawsIamAwsidentitymanagementv20100508State::temporary_credentials`]),
//!    which is what resolves a session of an IAM USER ([`signing_principal`]);
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
//!   call, so retagging the user changes what the same unchanged request decides; for a
//!   role session those tags are the role's own plus the session tags the mint attached
//!   ([`principal_tags`]);
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
//!   (`aws:SourceIp`), which is what a network clause is decided against, and whether it
//!   arrived encrypted (`aws:SecureTransport`), which is what a transport clause is decided
//!   against ([`Connection`]);
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
//! * **A credential whose CLASS cannot call IAM at all.** The ban AWS states per class of
//!   temporary credential — a federated-user credential "cannot call any IAM operations",
//!   and a `GetSessionToken` credential cannot "unless MFA authentication information is
//!   included in the request" — is a fact about the credential and not a document, so it is
//!   decided before any policy is read and no `Allow` anywhere lifts it
//!   ([`class_refusal`], over the class the mint recorded in
//!   [`super::TemporaryCredentialRecord`]).
//! * **A credential this account holds no identity for.** The emulator's own published
//!   credential is not a key this account's `access_keys` map holds, and no role session
//!   was vended for it. It carries no identity-based policies at all, which is the position
//!   AWS account root credentials are in, so such a request is allowed rather than refused
//!   on evidence that does not exist. The same holds for a role
//!   session whose role has since been deleted.
//! * **A principal holding a grant this emulator cannot read.** A policy AWS owns is not
//!   account state, so its document is AWS's published one, read out of the catalog of
//!   them this crate carries ([`super::AWS_MANAGED_POLICIES`]) — an attachment of a
//!   published entry therefore decides a call exactly as an inline document does, in
//!   either direction. The catalog is a SUBSET of AWS's, and an attachment it does not
//!   carry resolves to no document at all, which is what `principal_policies` reports as
//!   uncarried. A principal holding one of those is allowed where the documents that ARE
//!   held neither grant nor deny: what that document grants is unknown here, and refusing
//!   on evidence this emulator does not have would deny access AWS permits. An explicit
//!   `Deny` in a document that is held still refuses, because no grant overturns one. A
//!   boundary whose document is not held is read the same way: it is a ceiling this
//!   emulator cannot measure a call against, so it caps nothing rather than refusing what
//!   AWS would permit.

use ::service_common::iam_authorization::{self as iam, Decision};
use ::service_common::temporary_credential_classes::{
    PRINCIPAL_TYPE_CONDITION_KEY, TemporaryCredentialClass,
};

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

/// Where the detail of a refusal is filed and how its message points a caller at it: the
/// authorization id minted per refused request, the clause naming it, and the record it
/// resolves to.
pub mod details;

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

/// The IAM-specific condition key carrying the ARN of the managed policy a request SETS AS
/// A PERMISSIONS BOUNDARY: "`iam:PermissionsBoundary` — checks that the specified policy is
/// attached as permissions boundary on the IAM principal resource", compared under IAM's ARN
/// operators.
///
/// It is the value of that request's own `PermissionsBoundary` parameter, which is what lets
/// a grant be scoped to the creates and puts that attach ONE named boundary — the delegation
/// pattern where an administrator may create principals only inside a ceiling it cannot
/// remove. Filled — and declared — only by the call sites that carry such a parameter and
/// that IAM publishes the key for ([`authorize_setting_boundary`]): for every other call it
/// stays undeclared, for the reason [`POLICY_ARN_KEY`] states.
const PERMISSIONS_BOUNDARY_KEY: &str = "iam:PermissionsBoundary";

/// The condition key carrying the ADDRESS the request was made from: "`aws:SourceIp` —
/// the requester's IP address", which is what an IP clause compares against.
const SOURCE_IP_KEY: &str = "aws:SourceIp";

/// The condition key saying whether the request arrived over a secure transport:
/// "`aws:SecureTransport` — whether the request was sent using SSL", which is what a
/// `Bool` clause requiring — or refusing — encryption in flight compares against.
const SECURE_TRANSPORT_KEY: &str = "aws:SecureTransport";

/// The two values that key carries, as IAM's boolean literals spell them: "true" for a
/// request that arrived over a secure transport and "false" for one that did not.
const SECURE_TRANSPORT_TRUE: &str = "true";

/// See [`SECURE_TRANSPORT_TRUE`].
const SECURE_TRANSPORT_FALSE: &str = "false";

/// The header a proxy in front of this emulator states the SCHEME it forwarded a request
/// over in, whose FIRST entry is the hop the client itself made — the transport's
/// counterpart of [`FORWARDED_FOR_HEADER`], read the same way.
const FORWARDED_PROTO_HEADER: &str = "x-forwarded-proto";

/// The scheme a request made over a PLAINTEXT hop states, and the one value that makes
/// [`SECURE_TRANSPORT_KEY`] false: every other hop this emulator is reachable over
/// terminates TLS.
const PLAINTEXT_SCHEME: &str = "http";

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

/// The condition key carrying the Region the request was made TO: "`aws:RequestedRegion` —
/// the Region that was called in the request", which is what a clause scoping a grant to
/// one Region compares against.
const REQUESTED_REGION_KEY: &str = "aws:RequestedRegion";

/// The Region every request to this service was made to, whatever Region the caller
/// addressed it from: the one the single global IAM endpoint lives in.
///
/// IAM is a GLOBAL service — one endpoint for the whole partition, in the Region
/// [`super::IAM_CONSOLE_REGION`] names — so a call signed for any other Region is still a
/// call made to that one endpoint, and the Region it REQUESTED is that endpoint's. The
/// Region a request named in its SigV4 credential scope is therefore not what this key
/// answers, and the same signed request decides the same way wherever the client was
/// configured.
const REQUESTED_REGION: &str = super::IAM_CONSOLE_REGION;

/// The keys every request to this gate models about ITSELF: the tags it writes, the
/// address it was made from, the transport it arrived over and the instant it is decided
/// at.
///
/// Published for every call, because each is answered from the call itself — a request
/// that writes no tag carries no `aws:RequestTag/` and no `aws:TagKeys` value, which is
/// exactly what a clause over one of them is then decided from, while the connection and
/// the clock always have an answer. [`REQUESTED_REGION_KEY`] is published beside them for
/// a decision about THIS request, and left undeclared for one about a request nobody has
/// made yet ([`request_context`]).
const PUBLISHED_REQUEST_KEYS: &[&str] = &[
    REQUEST_TAG_PREFIX,
    TAG_KEYS_KEY,
    SOURCE_IP_KEY,
    SECURE_TRANSPORT_KEY,
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

/// Whether the request `parts` arrived over a secure transport, as `aws:SecureTransport`
/// carries it.
///
/// Like the address, this is a fact of the CONNECTION and not of anything the request body
/// says: the scheme the request itself states — on its own URI, or in
/// [`FORWARDED_PROTO_HEADER`] where a proxy stated the hop it forwarded for — and
/// otherwise secure, because this emulator's API listener terminates TLS, so a request
/// that reached this gate without saying how it arrived arrived encrypted. A hop stated as
/// [`PLAINTEXT_SCHEME`] is the one that did not, so the TLS-enforcement idiom — an
/// unconditional `Allow` beside a `Deny` conditioned on the key being false — permits an
/// encrypted call and refuses a plaintext one, rather than leaving both undecided.
fn secure_transport(parts: &::http::request::Parts) -> bool {
    let stated = parts
        .uri
        .scheme_str()
        .map(str::to_ascii_lowercase)
        .or_else(|| {
            parts
                .headers
                .get(FORWARDED_PROTO_HEADER)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.split(',').next())
                .map(|hop| hop.trim().to_ascii_lowercase())
        })
        .filter(|scheme| !scheme.is_empty());
    stated.is_none_or(|scheme| scheme != PLAINTEXT_SCHEME)
}

/// What the CONNECTION a request arrived on contributes to the condition context: the
/// address it was made from and whether it was encrypted, the two things IAM publishes
/// about a request's transport rather than about its parameters.
///
/// Read once per decision, off the request's own head ([`Self::of`]), so every decision
/// taken about one request is decided against one reading of the connection it arrived on.
#[derive(Debug, Clone)]
struct Connection {
    /// The address the request was made from, as [`SOURCE_IP_KEY`] carries it
    /// ([`source_address`]).
    source_ip: String,
    /// Whether it arrived over a secure transport, as [`SECURE_TRANSPORT_KEY`] carries it
    /// ([`secure_transport`]).
    secure_transport: bool,
}

impl Connection {
    /// The connection the request `parts` arrived on.
    fn of(parts: &::http::request::Parts) -> Self {
        Self {
            source_ip: source_address(parts),
            secure_transport: secure_transport(parts),
        }
    }

    /// The `(key, value)` context entries it contributes, under the keys IAM publishes
    /// them as.
    fn context(&self) -> Vec<(String, String)> {
        vec![
            (SOURCE_IP_KEY.to_string(), self.source_ip.clone()),
            (
                SECURE_TRANSPORT_KEY.to_string(),
                if self.secure_transport {
                    SECURE_TRANSPORT_TRUE
                } else {
                    SECURE_TRANSPORT_FALSE
                }
                .to_string(),
            ),
        ]
    }
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
/// no value for it: a real signed request does carry `aws:PrincipalArn` and its peers, and
/// this gate must not decide a clause over them from their absence here.
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

/// What the REQUEST's own parameters contribute to the condition context, beside the action
/// it asks for and the resource it is taken on: one entry per parameter IAM publishes as a
/// condition key.
///
/// Held as ONE value rather than as a parameter apiece so that a call site hands the gate
/// everything its request carries, and so that adding the next published parameter does not
/// re-thread every spelling of [`authorize`]. A call that carries none of them is decided
/// against the default, which declares nothing.
#[derive(Debug, Clone, Copy, Default)]
struct RequestParameters<'a> {
    /// The tags the call WRITES ([`WrittenTags`]).
    written: WrittenTags<'a>,
    /// The managed policy the call NAMES, for the attachment calls ([`POLICY_ARN_KEY`]).
    policy_arn: Option<&'a str>,
    /// The managed policy the call SETS AS A BOUNDARY, for the creates and puts that carry
    /// such a parameter ([`WrittenBoundary`]).
    permissions_boundary: WrittenBoundary<'a>,
}

/// What a request says about the permissions boundary it SETS, which is where
/// [`PERMISSIONS_BOUNDARY_KEY`] comes from.
///
/// The three positions a request can be in are distinguished because two of them are facts
/// about the request and the third is not: a call CARRYING a `PermissionsBoundary` parameter
/// either sets one or sets none, and both are things this gate knows, so the key is declared
/// for both and a clause over it is decided — a negated comparison or a `Null` asking for
/// absence holding on the call that sets none. A call carrying no such parameter is a call
/// IAM publishes no value of the key for at all, so there the key stays undeclared and a
/// clause over it is left undecided, the fail-closed reading every key this gate cannot
/// answer for gets.
#[derive(Debug, Clone, Copy, Default)]
enum WrittenBoundary<'a> {
    /// A call that carries no `PermissionsBoundary` parameter — every operation but the
    /// creates and the boundary puts.
    #[default]
    Unmodelled,
    /// A call that carries the parameter and sets no boundary.
    Unset,
    /// A call that sets the boundary this ARN names.
    Arn(&'a str),
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
    /// policies the call is judged on, the session's own name, and the tags it was minted
    /// carrying.
    RoleSession {
        /// The role the session is of.
        role: &'a Role,
        /// The session name the vending call carried, which is what IAM spells into the
        /// session's unique id after the role's own.
        session_name: &'a str,
        /// The session tags the mint attached, as it submitted them
        /// ([`super::RoleSessionRecord::tags`]).
        session_tags: &'a [(String, String)],
    },
}

/// The tags the principal of a role session holds, as `(key, value)` pairs: the ones the
/// mint attached to the SESSION, and the ones the ROLE it is a session of carries.
///
/// `aws:PrincipalTag/tag-key` is "the tag attached to the principal making the request",
/// and a request a role session signs is made by a principal that holds both sets: the
/// role's own tags and the session tags the mint passed, which "you can … use … to control
/// access to resources" (`AssumeRoleRequest$Tags`). Each value is carried through exactly
/// as it was stored, so the key holds the text the mint submitted and a `StringEquals`
/// clause over it compares that text.
///
/// One key holds one value, and a session tag is the more specific of the two, so where
/// both name the same tag the session's value is the one in force — found through the ONE
/// comparison this service reads a PRINCIPAL's tag key by
/// ([`super::TagListTarget::same_tag_key`]), since tag keys "are not case sensitive, but
/// case is preserved" (the same member) and a role's own tag set is read by that same rule.
fn principal_tags(role: &Role, session_tags: &[(String, String)]) -> Vec<(String, String)> {
    let mut held: Vec<(String, String)> = session_tags.to_vec();
    for tag in role.tags.iter().flatten() {
        if !held
            .iter()
            .any(|(key, _)| super::TagListTarget::Principal.same_tag_key(key, &tag.key))
        {
            held.push((tag.key.clone(), tag.value.clone()));
        }
    }
    held
}

/// The condition context a request by `caller` against `resource` carries, and the
/// keys it declares it models: everything IAM knows about the call at the instant it is
/// decided, which is where a policy variable's value and a `Condition` clause's value both
/// come from (see the module docs).
///
/// The keys it fills are the caller's own ([`PUBLISHED_PRINCIPAL_KEYS`]), the target's
/// ([`PUBLISHED_RESOURCE_TAG_KEYS`], both spellings of the one tag set), the request's own
/// tags, the `connection` it arrived on, the Region it was made to when it is a request at
/// all ([`REQUESTED_REGION_KEY`]) and the clock's
/// ([`PUBLISHED_REQUEST_KEYS`]). The resource-tag families are
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
/// the role's tags AND the session's ([`principal_tags`]), both of which the account knows
/// in full — the session's travel with the record the mint left — so that family is
/// published for a session exactly as it is for a user.
///
/// The principal's and the target's tags are the ones they hold NOW — nothing in the
/// request carries them — so retagging either side changes what the same unchanged request
/// decides, and untagging it takes the value away again.
/// A call that NAMES a managed policy carries that ARN too, under the key IAM publishes it
/// as ([`POLICY_ARN_KEY`]), and one that SETS a boundary carries the boundary's
/// ([`PERMISSIONS_BOUNDARY_KEY`]) — both out of [`RequestParameters`].
fn request_context(
    store: &IamState,
    caller: Caller<'_>,
    resource: &str,
    parameters: RequestParameters<'_>,
    connection: &Connection,
    requested_region: Option<&str>,
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
        Caller::RoleSession {
            role,
            session_name,
            session_tags,
        } => {
            context.push((
                USER_ID_KEY.to_string(),
                format!("{}:{session_name}", role.role_id),
            ));
            published.push(USER_ID_KEY);
            for (key, value) in principal_tags(role, session_tags) {
                context.push((format!("{PRINCIPAL_TAG_PREFIX}{key}"), value));
            }
            published.push(PRINCIPAL_TAG_PREFIX);
        }
    }
    // What the CLASS of the presented credential answers `aws:PrincipalType` with: a role
    // session is an `AssumedRole`, and an IAM user's own long-term key is a `User`. Answered
    // for every request this gate decides, because the signing credential was resolved
    // before anything was decided, so a clause over the key decides rather than standing
    // undecided. The third class a credential can be, a federated user, never reaches a
    // decision here: such a credential may not call IAM at all ([`class_refusal`]).
    let class = match caller {
        Caller::User(_) => None,
        Caller::RoleSession { .. } => Some(TemporaryCredentialClass::AssumedRole),
    };
    context.extend(::service_common::temporary_credential_classes::principal_type_conditions(
        Some(::service_common::temporary_credential_classes::principal_type(class)),
    ));
    published.push(PRINCIPAL_TYPE_CONDITION_KEY);
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
    context.extend(parameters.written.context());
    // The managed policy the request NAMES, for a call that carries one: a single value,
    // read off the request's own parameter rather than off anything stored, so an
    // `ArnEquals`/`ArnLike` clause scoping a grant to one policy is decided from the
    // policy the call actually asked about.
    if let Some(policy_arn) = parameters.policy_arn {
        context.push((POLICY_ARN_KEY.to_string(), policy_arn.to_string()));
        published.push(POLICY_ARN_KEY);
    }
    // The managed policy the request SETS AS A BOUNDARY, read off that parameter the same
    // way for a call that carries one — whether it sets a boundary or sets none, both of
    // which are facts about the request ([`WrittenBoundary`]) — so a grant admitting only
    // the calls that attach one named ceiling is decided from the ceiling the call actually
    // asked for.
    match parameters.permissions_boundary {
        WrittenBoundary::Unmodelled => {}
        WrittenBoundary::Unset => published.push(PERMISSIONS_BOUNDARY_KEY),
        WrittenBoundary::Arn(permissions_boundary) => {
            context.push((
                PERMISSIONS_BOUNDARY_KEY.to_string(),
                permissions_boundary.to_string(),
            ));
            published.push(PERMISSIONS_BOUNDARY_KEY);
        }
    }
    // Where the call came from and how it arrived, read from the connection and from
    // nothing the request's own parameters say ([`Connection`]).
    context.extend(connection.context());
    // The Region the call was made TO, for a decision about THIS request: for this global
    // service that is the Region its one endpoint lives in, whatever Region the caller
    // addressed ([`REQUESTED_REGION`]). A decision about a request nobody has made yet is
    // handed `None` and the key stays undeclared, because the Region such a request would
    // address is not this one's.
    if let Some(requested_region) = requested_region {
        context.push((
            REQUESTED_REGION_KEY.to_string(),
            requested_region.to_string(),
        ));
        published.push(REQUESTED_REGION_KEY);
    }
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
/// error messages", IAM User Guide), and for a request that addresses an entity the account
/// ALREADY HOLDS that is the entity, not the collection it lives in — so such an operation
/// words it as the entity's TYPE followed by the name the request carried (`user Bob`),
/// with the type that disambiguates which entity a name belongs to. The ARN stays what the
/// DECISION is made against, because a policy statement's `Resource` matches ARNs.
///
/// A CREATE has no such entity to name: the request is made on the entity it is creating,
/// which does not exist yet, so the only name the refusal can carry for it is the ARN it
/// WOULD have — the very value the decision is made against. Those operations therefore
/// take [`authorize`], or hand that ARN in for both, and the refusals of a create and of a
/// read of the same entity differ in exactly that.
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
    authorize_request(
        store,
        ctx,
        action,
        resource,
        named_as,
        RequestParameters {
            written,
            ..RequestParameters::default()
        },
    )
}

/// [`authorize_written`] for a call that SETS A PERMISSIONS BOUNDARY — the creates and puts
/// carrying a `PermissionsBoundary` parameter — which hands the gate that boundary's ARN so
/// a clause over [`PERMISSIONS_BOUNDARY_KEY`] can be decided.
///
/// The ARN the request's `PermissionsBoundary` parameter carries is part of the request IAM
/// decides: `iam:PermissionsBoundary` is read from it, so a grant admitting only the calls
/// that attach ONE named ceiling permits exactly those and withholds every other — which is
/// how a delegated administrator is allowed to create principals only inside a boundary it
/// cannot leave off. A request that carries the parameter and sets NO boundary carries no
/// value for the key and says so, so a negated clause over it holds there, while a call with
/// no such parameter publishes nothing about the key at all ([`WrittenBoundary`]). The
/// decision is still made against the ARN of the entity the call is taken on (`resource`),
/// and it precedes the write, so a refusal leaves that entity as it was.
pub fn authorize_setting_boundary(
    store: &IamState,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    action: &str,
    resource: &str,
    named_as: &str,
    written: WrittenTags<'_>,
    permissions_boundary: Option<&str>,
) -> Result<(), AccessDenied> {
    authorize_request(
        store,
        ctx,
        action,
        resource,
        named_as,
        RequestParameters {
            written,
            permissions_boundary: match permissions_boundary {
                Some(arn) => WrittenBoundary::Arn(arn),
                None => WrittenBoundary::Unset,
            },
            ..RequestParameters::default()
        },
    )
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
        RequestParameters {
            policy_arn: Some(policy_arn),
            ..RequestParameters::default()
        },
    )
}

/// The ONE decision every spelling above reaches, handed everything the REQUEST itself
/// contributes to the condition context: the tags it writes, the managed policy it names and
/// the boundary it sets (see [`request_context`]).
fn authorize_request(
    store: &IamState,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    action: &str,
    resource: &str,
    named_as: &str,
    parameters: RequestParameters<'_>,
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
        identity,
        policies,
        scope,
    } = signer;
    // Which KIND of credential presented this request, read before the caller is spent on
    // the condition context: a refusal of a role SESSION is not worded like a refusal of an
    // identity's own long-term key ([`details::DeniedRequest::points_at_its_detail`]).
    let session_credential = matches!(caller, Caller::RoleSession { .. });
    let (context, published) = request_context(
        store,
        caller,
        resource,
        parameters,
        &Connection::of(ctx.parts()),
        Some(REQUESTED_REGION),
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
    //
    // The ceiling read is the one in force on the IDENTITY whose documents were collected
    // (`identity`), not on the credential's own principal: a boundary "sets the maximum
    // permissions that an identity-based policy can grant to an entity", so for a session of
    // a role it is the ROLE's boundary that caps the session, exactly as the role's policies
    // are what grant it. A session carries no boundary of its own, so reading one against
    // the session's ARN would cap nothing whatever ceiling the role carries.
    let (capped, boundary) = if decided == Decision::Denied {
        (Decision::Allowed, None)
    } else {
        match store.permissions_boundary_in_force(&identity) {
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
    // because they need different fixes. An explicit deny also NAMES the policy the
    // deciding statement came from wherever that policy has an ARN to name — the managed
    // policy whose document denied the call ([`denying_managed_policy`]) and the boundary
    // that capped it — because in an identity carrying several documents that ARN is the
    // only thing in the message saying which one has to be changed. An inline policy has
    // no ARN, so the clause ends where the document it came from cannot be addressed.
    let reason = if decided == Decision::Denied {
        match denying_managed_policy(store, &identity, &request) {
            Some(arn) => format!("with an explicit deny in an identity-based policy: {arn}"),
            None => "with an explicit deny in an identity-based policy".to_string(),
        }
    } else if scoped == Decision::Denied {
        "with an explicit deny in a session policy".to_string()
    } else if capped == Decision::Denied {
        match &boundary {
            Some(arn) => iam::permissions_boundary_explicit_deny(arn),
            None => iam::PERMISSIONS_BOUNDARY_EXPLICIT_DENY.to_string(),
        }
    } else if !granted {
        format!("because no identity-based policy allows the {action} action")
    } else if scoped != Decision::Allowed {
        format!("because no session policy allows the {action} action")
    } else {
        format!("because no permissions boundary allows the {action} action")
    };
    let explicit_deny =
        decided == Decision::Denied || scoped == Decision::Denied || capped == Decision::Denied;
    // Audit record for the refusal: the acting principal, what it asked for and the
    // decision, so the refusal is reviewable after the fact. The policy documents
    // themselves are not logged.
    ::tracing::info!(
        audit = true,
        outcome = "refused_access_denied",
        principal = %principal,
        action = %action,
        resource = %resource,
        explicit_deny,
        "the policies in force on the caller do not permit this call"
    );
    // The refusal states the decision and then points at its DETAIL: the authorization id
    // the message names is minted here and the record it resolves to is filed under it
    // ([`details::record_denial`]), out of what this decision was made from — for the
    // refusals that carry a pointer at all
    // ([`details::DeniedRequest::points_at_its_detail`]).
    Err(not_authorized_detailed(
        store,
        &details::DeniedRequest {
            principal: &principal,
            identity: &identity,
            action,
            resource,
            context: &context,
            explicit_deny,
            boundary: boundary.as_deref(),
            session_policies: scope.documents.len(),
            session_credential,
            identity_grant_absent: !granted,
            // Read only where it can decide the wording: each question costs a parse of the
            // documents in force, and only a session's refusal for want of an identity-based
            // grant is worded by them.
            identity_grant_names_the_action: session_credential
                && !granted
                && !explicit_deny
                && identity_grant_names_the_action(action, &policies),
            scope_allows_the_action_unnamed: session_credential
                && !granted
                && !explicit_deny
                && scope_allows_the_action_unnamed(action, &scope),
        },
        named_as,
        &reason,
    ))
}

/// The ARN of the MANAGED policy whose statement denied `request` for the identity `arn`
/// names, or `None` when the deny came from an inline policy — which has no ARN — or when
/// these documents state no deny at all.
///
/// Read on the refusal path only, so the decision itself pays nothing for it: the same
/// documents the decision was made from are collected again with their PROVENANCE kept
/// ([`super::ComAmazonawsIamAwsidentitymanagementv20100508State::principal_sourced_policies`],
/// whose documents are the ones the decision read, in that order), and the statements the
/// evaluation matched say which of them denied the call — "the deny statement is the only
/// entry included in the result". The first denying document that is a managed policy is
/// the one named, since that is the policy a caller has to change for the call to be
/// decided differently.
fn denying_managed_policy(
    store: &IamState,
    arn: &str,
    request: &iam::Request<'_>,
) -> Option<String> {
    let carried = store.principal_sourced_policies(arn)?;
    // The documents in force, in the order the decision read them: an attachment whose
    // document this emulator does not hold contributed none, so it takes no position here
    // either.
    let sourced: Vec<&super::SourcedPolicy> = carried
        .iter()
        .filter(|policy| policy.document.is_some())
        .collect();
    let documents: Vec<String> = sourced
        .iter()
        .filter_map(|policy| policy.document.clone())
        .collect();
    let outcome = iam::identity_outcome_prepared(&iam::prepare_documents(&documents), request);
    if outcome.decision != Decision::Denied {
        return None;
    }
    outcome.matched.iter().find_map(|matched| {
        match &sourced.get(matched.document)?.source {
            super::PolicySource::Managed { policy_arn, .. } => Some(policy_arn.clone()),
            super::PolicySource::Inline { .. } => None,
        }
    })
}

/// Whether one of the IDENTITY's own documents states an `Allow` covering `action`.
///
/// The tree's weaker reading of a grant
/// ([`::service_common::iam_authorization::identity_may_allow_action`]), which answers the
/// question about the ACTION alone: an `Allow` covering it counts whichever resource it is
/// scoped to and whether or not its `Condition` holds at the instant of the call. So it
/// separates a decision that read a grant about the action and did not apply it from one no
/// readable `Allow` was about at all — which is what the refusal's wording turns on
/// ([`details::DeniedRequest::points_at_its_detail`]).
fn identity_grant_names_the_action(action: &str, policies: &PrincipalPolicies) -> bool {
    iam::identity_may_allow_action(&iam::prepare_documents(&policies.documents), action)
}

/// Whether the session policies the credential was scoped with allow `action` while none of
/// them SPELLS it ([`::service_common::iam_authorization::identity_names_action_exactly`]):
/// a scope written about everything the session may do rather than about this action.
///
/// The scope is read as WRITTEN here, so a credential no mint scoped allows nothing, unlike
/// [`ScopingPolicies::decision`], which reads an absent scope as capping nothing — the
/// question is which document the decision READ, not what it capped.
fn scope_allows_the_action_unnamed(action: &str, scope: &ScopingPolicies) -> bool {
    let documents = iam::prepare_documents(&scope.documents);
    iam::identity_may_allow_action(&documents, action)
        && !iam::identity_names_action_exactly(&documents, action)
}

/// The `AccessDenied` text AWS words a call it will not perform for its caller with:
/// what the refusal is about — the principal that signed the request, the action it
/// asked for and the resource it asked for it on — closed by `reason`, which says why,
/// and then, for a refusal whose recordings carry one
/// ([`details::DeniedRequest::points_at_its_detail`]), by the POINTER at the detail of that
/// decision ([`details::pointer_clause`]), naming the authorization id its record is filed
/// under ([`details::record_denial`]) — so a caller that reads the id out of the message and
/// calls the API the message names is served the detail of this very decision. A refusal that
/// carries no pointer files no record either: the id is the only way to reach one, so a
/// record no message hands out the id of resolves for nobody.
///
/// ONE wording for the code, so a refusal an operation reaches for a reason of its own
/// is worded exactly as the policy decision above is, differing only in that reason.
/// Every part of it besides that id is a fact of the request and of the account's state, so
/// the same refused call is worded the same on every run and in every Region; the id is
/// minted per request, so it is the one part two refusals of the same call differ in.
/// [`not_authorized_without_reason`] is the OTHER wording this service answers: the one
/// for a refusal no evaluation of the policies in force explains, which therefore has no
/// reason to state and no decision to point at.
fn not_authorized_detailed(
    store: &IamState,
    denied: &details::DeniedRequest<'_>,
    named_as: &str,
    reason: &str,
) -> AccessDenied {
    // The sentence itself is the tree's ONE wording of the identity-based refusal, so this
    // service and every other one that reaches the same decision word it alike.
    let sentence = iam::not_authorized_by_identity_policy(
        denied.principal,
        denied.action,
        named_as,
        reason,
    );
    if !denied.points_at_its_detail() {
        return AccessDenied::new(sentence);
    }
    let authorization_id = details::record_denial(store, denied);
    AccessDenied::new(format!(
        "{sentence}{}",
        details::pointer_clause(&authorization_id)
    ))
}

/// The `AccessDenied` text AWS words a call it will not perform for its caller with when
/// NO evaluation of the policies in force explains it: what the refusal is about — the
/// principal, the action and the resource — and nothing after it.
///
/// A refusal the documents in force explain says which of them withheld the call or which
/// statement denied it ([`not_authorized`]), because that clause is what the caller has to
/// fix. This refusal has no such clause to carry: the request addresses a resource this
/// account cannot resolve to an entity, so no statement could have been about it and no
/// document withheld anything. Recorded behaviour words it without the reason AND without
/// the colon after `perform` that the explained sentence carries, which is the tree's
/// [`::service_common::iam_authorization::not_authorized_to_perform_unpunctuated`] — the
/// family of these wordings lives there, so this is only the lift into the error channel.
pub fn not_authorized_without_reason(principal: &str, action: &str, named_as: &str) -> AccessDenied {
    AccessDenied::new(
        ::service_common::iam_authorization::not_authorized_to_perform_unpunctuated(
            principal, action, named_as,
        ),
    )
}

/// The ARN of the principal this request speaks for, as `sts:GetCallerIdentity` reports it
/// for the credential the request was signed with — what a refusal this service words
/// itself names as the caller.
///
/// The same resolution the gate above makes its decision about ([`signing_principal`]): the
/// IAM user that holds the long-term key, or the role session the temporary credential was
/// vended for. EVERY other credential — the long-term pair this emulator publishes at
/// start, a key id nothing issued, and a session whose role or user the account no longer
/// holds — resolves to no identity of this account, and STS reports one principal for all
/// of them: the IAM user that published pair belongs to
/// ([`::service_common::caller_principal::ambient_user_arn`]). So that is what a refusal
/// names for them, the identity such a caller reads back for itself rather than an invented
/// one.
///
/// A READ: the store is only consulted.
pub fn caller_principal_arn(
    store: &IamState,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
) -> String {
    let access_key_id = ::engine::extract_akid(&ctx.parts().headers);
    match signing_principal(store, &access_key_id) {
        Some(signer) => signer.arn,
        None => ::service_common::caller_principal::ambient_user_arn(),
    }
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
        // A permission check reports a verdict rather than refusing, so it files no
        // authorization detail and has no use for the identity the documents came from.
        identity: _,
    } = signer;

    let principals = [principal];
    let documents = iam::prepare_documents(&policies.documents);
    // The connection this check's own request arrived on, read once for every ask it
    // decides.
    let connection = Connection::of(ctx.parts());
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
            // An ask is an `(action, resource)` pair read out of a policy document, not a
            // call: it carries no request parameter at all, so no written tag, no managed
            // policy ARN and no boundary.
            RequestParameters::default(),
            &connection,
            // Nor any Region: an ask names a resource of another service that no request
            // has addressed yet, so the Region such a request would be made to is not this
            // call's, and a grant conditioned on it stays undecided — which is what makes
            // the answer the third value rather than a verdict.
            None,
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
    /// The ARN of the IAM IDENTITY whose documents were collected — the same ARN for a
    /// long-term key, and the ROLE's for a session of it, whose policies such a credential
    /// has. What the detail of a refusal reports the evaluated policies as attached to.
    identity: String,
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
/// Three credentials carry identity-based policies: an IAM user's own long-term access key,
/// resolved through the registry `CreateAccessKey` writes; the temporary credential of a
/// session of one of the account's roles, resolved through the record the operation that
/// vended it left ([`super::ComAmazonawsIamAwsidentitymanagementv20100508State::role_session`]);
/// and every OTHER temporary credential this account minted, resolved through the principal
/// the mint recorded beside its class
/// ([`super::ComAmazonawsIamAwsidentitymanagementv20100508State::temporary_credential`]) — a
/// session of an IAM USER, which "has the same permissions as the IAM user"
/// (`GetSessionToken`, the STS model), and a session of a role whose own record this store no
/// longer holds. A session is judged on the policies of the identity it is a session OF —
/// "the temporary security credentials … have the permissions of the role" (`AssumeRole`) —
/// while the refusal names the session, because that is the principal the request speaks for.
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
            identity: user.arn.clone(),
            policies,
            // A long-term key is the identity's own credential: no mint scoped it.
            scope: ScopingPolicies::default(),
        });
    }
    let now = super::now();
    if let Some(session) = store.role_session(access_key_id, now) {
        // The role the session is of, addressed by the ARN the account holds it under: a
        // role deleted since the session was vended leaves the session speaking for an
        // identity this account no longer holds policies for.
        let role_name = ::service_common::iam_role::role_name(&session.role_arn)?;
        let role = store.roles.get(&super::name_key(role_name))?;
        let policies = store.principal_policies(&role.arn)?;
        return Some(SigningPrincipal {
            caller: Caller::RoleSession {
                role,
                session_name: ::service_common::iam_role::assumed_role_session_name(
                    &session.session_arn,
                )?,
                session_tags: &session.tags,
            },
            arn: session.session_arn.clone(),
            identity: role.arn.clone(),
            policies,
            // What the CREDENTIAL itself was scoped to, resolved at the instant of the
            // request so a managed session policy edited since the mint scopes by what it
            // says NOW.
            scope: super::session_scoping_documents(Some(store), &session.scope),
        });
    }
    // Every other temporary credential this account minted: the principal it speaks for is
    // the one the mint recorded with its class, and the identity-based policies in force on
    // it are that principal's. A session of an IAM user is the principal's own session, so
    // the user's documents judge it; a session ARN naming a role is judged on that role's,
    // the same reading the record above gives. No scope is recorded beside such a credential,
    // so it is capped by nothing but the identity's own documents and ceiling.
    let credential = store.temporary_credential(access_key_id, now)?;
    let principal_arn = credential.principal_arn.as_str();
    if let Some(role_name) =
        ::service_common::iam_role::assumed_role_session_role_name(principal_arn)
    {
        let role = store.roles.get(&super::name_key(role_name))?;
        let policies = store.principal_policies(&role.arn)?;
        return Some(SigningPrincipal {
            caller: Caller::RoleSession {
                role,
                session_name: ::service_common::iam_role::assumed_role_session_name(
                    principal_arn,
                )?,
                session_tags: &[],
            },
            arn: principal_arn.to_string(),
            identity: role.arn.clone(),
            policies,
            scope: ScopingPolicies::default(),
        });
    }
    let user_name = ::service_common::iam_role::user_name(principal_arn)?;
    let user = store.users.get(&super::name_key(user_name))?;
    let policies = store.principal_policies(&user.arn)?;
    Some(SigningPrincipal {
        caller: Caller::User(user),
        arn: principal_arn.to_string(),
        identity: user.arn.clone(),
        policies,
        scope: ScopingPolicies::default(),
    })
}

// The CLASS of a temporary credential is NOT read here: whether a credential of the
// presented class may call IAM at all is an AUTHENTICATION outcome rather than an
// authorization one — "you cannot call any IAM operations using the Amazon Web Services CLI
// or the Amazon Web Services API" (`GetFederationToken`, the STS model) bans the credential
// and not the identity behind it, and no `Allow` anywhere lifts it. It is therefore decided
// before any document is read, by this service's pre-dispatch admission (`crate::service`,
// which reads `TemporaryCredentialClass::may_call_iam`), and what the class contributes to
// this gate is only the value `aws:PrincipalType` answers (`request_context`).
