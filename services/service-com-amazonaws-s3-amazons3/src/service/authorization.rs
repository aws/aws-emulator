// SPDX-License-Identifier: CC0-1.0

//! What AUTHORIZES one request this service is serving: the bucket's own policy, and the
//! account the request says owns the bucket.
//!
//! A bucket policy is a RESOURCE-BASED policy, so it is in force on every request against
//! the bucket's resources — a read, a write, a copy's read of its source — and not only on
//! the public ones. "An explicit deny in any policy overrides any allow" (IAM's own
//! evaluation order), so a statement about the request whose `Effect` is `Deny` refuses the
//! call before it does anything, whatever an identity-based policy would have granted.
//!
//! The decision itself is the tree's ONE evaluation of IAM policy documents
//! (`service_common::iam_authorization`), which every other emulated service that enforces a
//! resource policy answers with. What this module adds is the half only S3 knows:
//!
//! * the CONTEXT the request carries — the condition keys a statement may be written over,
//!   taken from the request's own members ([`ObjectWriteContext`]) and from the facts this
//!   gate holds for every request ([`global_conditions`]: who the caller is, which account it
//!   and the resource belong to; [`multi_factor_auth_conditions`]: whether the credential that
//!   signed was MFA-backed) — so a clause over one of them is DECIDED rather than
//!   unknown, which is what makes a policy that conditions a write on an encryption header,
//!   or a grant that conditions a read on the resource's owning account, enforceable at all;
//! * the KEYS this service declares it models ([`PUBLISHED_CONDITION_KEYS`]): absence of one
//!   of these from the context means "the request carries no value", which IAM reads under a
//!   negated operator, an `IfExists` spelling and a `Null` clause, while absence of any other
//!   key leaves the statement undecided;
//! * the SENTENCE a refusal is worded in, which for an explicit deny is the one IAM
//!   publishes for that decision ([`crate::state::explicit_resource_policy_denial`]).
//!
//! Only a deny this evaluation could DECIDE refuses. A clause over a key this service does
//! not answer leaves its statement undecided, and reading an undecided `Deny` as a refusal
//! would refuse the bucket owner's own calls on every bucket whose policy conditions anything
//! this emulator does not model — so such a deny stands aside. A bucket carrying no policy
//! denies nothing.

use ::service_common::iam_authorization::{
    account_principals, resource_outcome_prepared, Decision, Outcome, PreparedDocument, Request,
};

use crate::state::{
    caller_principal_arn, explicit_resource_policy_denial, ACCESS_DENIED_CODE,
    ACCESS_DENIED_MESSAGE, ACCESS_DENIED_STATUS,
};

/// The condition key saying whether the request arrived over a secure transport: "whether
/// the request was sent using SSL" (AWS General Reference, *Global condition context keys*).
///
/// This service ANSWERS it rather than leaving it open, because it knows: the emulator serves
/// every endpoint over TLS, so every request it judges arrived that way.
const SECURE_TRANSPORT_KEY: &str = "aws:SecureTransport";

/// The value that key carries, as IAM's boolean literals spell it.
const SECURE_TRANSPORT_VALUE: &str = "true";

/// The condition key an object write's `x-amz-acl` header answers (Amazon S3 User Guide,
/// *Actions, resources, and condition keys for Amazon S3*: `s3:x-amz-acl`).
const ACL_KEY: &str = "s3:x-amz-acl";

/// The condition key an object write's `x-amz-storage-class` header answers
/// (`s3:x-amz-storage-class`, same published list).
const STORAGE_CLASS_KEY: &str = "s3:x-amz-storage-class";

/// The condition key an object write's `x-amz-server-side-encryption` header answers
/// (`s3:x-amz-server-side-encryption`, same published list) — the key a policy that makes
/// encryption mandatory is written over.
const SERVER_SIDE_ENCRYPTION_KEY: &str = "s3:x-amz-server-side-encryption";

/// The condition key an object write's `x-amz-server-side-encryption-aws-kms-key-id` header
/// answers (`s3:x-amz-server-side-encryption-aws-kms-key-id`, same published list).
const KMS_KEY_ID_KEY: &str = "s3:x-amz-server-side-encryption-aws-kms-key-id";

/// The condition key an object write's `x-amz-server-side-encryption-customer-algorithm`
/// header answers (`s3:x-amz-server-side-encryption-customer-algorithm`, same published
/// list) — the key a policy that requires, or refuses, a CUSTOMER-provided key is written
/// over. The two other SSE-C headers answer no condition key of their own: the published
/// list carries this one, which is what says the write supplied its own key.
const SSE_CUSTOMER_ALGORITHM_KEY: &str = "s3:x-amz-server-side-encryption-customer-algorithm";

/// The condition key a copy's `x-amz-copy-source` header answers (`s3:x-amz-copy-source`,
/// same published list).
const COPY_SOURCE_KEY: &str = "s3:x-amz-copy-source";

/// The condition key a copy's `x-amz-metadata-directive` header answers
/// (`s3:x-amz-metadata-directive`, same published list).
const METADATA_DIRECTIVE_KEY: &str = "s3:x-amz-metadata-directive";

/// The condition key a LISTING's `max-keys` parameter answers (`s3:max-keys`, same published
/// list) — the key a bucket policy caps the size of an admitted listing with.
const MAX_KEYS_KEY: &str = "s3:max-keys";

/// The condition key a listing's `prefix` parameter answers (`s3:prefix`, same published
/// list) — the key a policy scopes a caller to one folder of a bucket with.
const PREFIX_KEY: &str = "s3:prefix";

/// The condition key a listing's `delimiter` parameter answers (`s3:delimiter`, same
/// published list).
const DELIMITER_KEY: &str = "s3:delimiter";

/// The global condition keys that name the calling PRINCIPAL, which this gate holds exactly
/// for every request it judges: "aws:PrincipalArn — the ARN of the principal that made the
/// request" and "aws:PrincipalAccount — the account the principal belongs to" (AWS General
/// Reference, *Global condition context keys*).
///
/// "For IAM roles, the request context returns the ARN of the role, not the ARN of the user
/// that assumed the role" (`aws:PrincipalArn`), so a request signed by an assumed-role
/// session answers that key with the ROLE's ARN — the identity whose policies are in force —
/// while the ARN a refusal names stays the session's, which is what STS reports for that
/// credential.
const PRINCIPAL_ARN_KEY: &str = "aws:PrincipalArn";
const PRINCIPAL_ACCOUNT_KEY: &str = "aws:PrincipalAccount";

/// The global condition key saying whether the credential that signed the request was
/// obtained with multi-factor authentication: "whether multi-factor authentication (MFA) was
/// used to validate the temporary security credentials that made the request" (AWS General
/// Reference, *Global condition context keys*, `aws:MultiFactorAuthPresent`).
///
/// The gate ANSWERS it from the credential the request presented, because the registry that
/// minted the credential records whether its mint satisfied an MFA challenge
/// ([`multi_factor_auth_conditions`]). It is PUBLISHED, so the key's ABSENCE is decided too:
/// "this key is present in the request context only when the principal uses temporary
/// credentials", so a request signed with a long-term access key carries no value for it, and
/// an `…IfExists` clause over it — the spelling a "deny unless MFA" statement is written in —
/// holds on such a request while a clause without the suffix does not.
const MULTI_FACTOR_AUTH_PRESENT_KEY: &str = "aws:MultiFactorAuthPresent";

/// The global condition key naming the instant the SESSION whose credential signed the
/// request was issued: "the date and time that temporary security credentials were issued"
/// (AWS General Reference, *Global condition context keys*, `aws:TokenIssueTime`).
///
/// The gate ANSWERS it from the credential the request presented, because the registry that
/// minted it records that instant (`sts_store::Caller::token_issue_time`), and it is
/// PUBLISHED for the same reason the MFA key is: "this key is present in the request context
/// only when the principal uses temporary credentials", so a request signed with a long-term
/// access key carries no value for it and the key's ABSENCE is decided rather than unknown.
///
/// A `Deny` conditioned on it with `DateLessThan` is the shape that revokes every session a
/// role issued before a cut-off (the `AWSRevokeOlderSessions` policy an administrator attaches
/// to a role), and such a statement can only decide — rather than leave its refusal
/// undecided, which words it as if nothing had granted the action — where the request answers
/// the key.
const TOKEN_ISSUE_TIME_KEY: &str = "aws:TokenIssueTime";
/// The global condition key naming the account that OWNS the resource the request addressed
/// ("aws:ResourceAccount", same published list).
///
/// This gate ANSWERS it rather than leaving it open, because it knows: this emulator serves
/// ONE account, and that account owns every bucket it holds — the same fact the
/// confused-deputy guard of every operation is decided by
/// ([`expected_bucket_owner_refusal`]). The owning account is not in a bucket's own ARN
/// (`arn:aws:s3:::<bucket>` carries no account segment), which is why it is answered from the
/// account this emulator serves rather than parsed out of the resource.
const RESOURCE_ACCOUNT_KEY: &str = "aws:ResourceAccount";

/// The global condition keys naming the resource a CROSS-SERVICE call was made on behalf of
/// and the account that resource belongs to: "aws:SourceArn — the ARN of the resource making
/// a service-to-service request" and "aws:SourceAccount — the account the resource making the
/// request belongs to" (AWS General Reference, *Global condition context keys*).
///
/// They are the two keys a grant to a SERVICE PRINCIPAL is scoped by, so only a request
/// another service made as its own service principal carries a value for them
/// ([`service_principal_conditions`]).
const SOURCE_ARN_KEY: &str = "aws:SourceArn";
const SOURCE_ACCOUNT_KEY: &str = "aws:SourceAccount";

/// The keys a decision about a request another service made AS ITS OWN SERVICE PRINCIPAL
/// publishes beside [`PUBLISHED_CONDITION_KEYS`]: such a call knows each of them exactly (the
/// resource it acts for and that resource's account), so a statement asking whether one is
/// present — rather than what it equals — is decided rather than left undecided.
///
/// A request no such call was recorded for carries neither, so neither is published for it:
/// this service answers a key only where it knows the value.
pub const SERVICE_PRINCIPAL_CONDITION_KEYS: &[&str] = &[SOURCE_ARN_KEY, SOURCE_ACCOUNT_KEY];

/// The two pairs above, for a call made on behalf of `source_arn` in `source_account`.
#[must_use]
pub fn service_principal_conditions(
    source_arn: &str,
    source_account: &str,
) -> Vec<(String, String)> {
    vec![
        (SOURCE_ARN_KEY.to_string(), source_arn.to_string()),
        (SOURCE_ACCOUNT_KEY.to_string(), source_account.to_string()),
    ]
}

/// The two condition keys that name the ACCESS POINT a request was made through: its ARN
/// ("s3:DataAccessPointArn") and the account that owns it ("s3:DataAccessPointAccount"),
/// both of S3's own published list (Amazon S3 User Guide, *Amazon S3 condition key
/// examples*).
///
/// They are what a bucket policy DELEGATES access control to its access points with — a
/// statement admitting any principal "when" the call came through an access point of the
/// bucket's own account — so this gate answers them for a request that came through one and
/// reports them ABSENT for one addressed to the bucket directly, which is exactly the
/// difference such a statement is written over.
const DATA_ACCESS_POINT_ARN_KEY: &str = "s3:DataAccessPointArn";
const DATA_ACCESS_POINT_ACCOUNT_KEY: &str = "s3:DataAccessPointAccount";

/// Every condition key a request judged here carries a value for when its member is present,
/// and so DECLARES this service models — the keys whose absence from the context is decided
/// ("the request carries none") rather than unknown. Every other key keeps the undecided
/// reading, which refuses nothing.
///
/// The keys that name the CALLER and its credential are last, because only a decision that
/// resolved the signing identity can answer them: [`CALLER_FREE_CONDITION_KEYS`] is this list
/// without them, and `the_two_published_lists_differ_by_the_caller_keys` in this module's
/// tests holds the two to that relation.
pub const PUBLISHED_CONDITION_KEYS: &[&str] = &[
    SECURE_TRANSPORT_KEY,
    ACL_KEY,
    STORAGE_CLASS_KEY,
    SERVER_SIDE_ENCRYPTION_KEY,
    KMS_KEY_ID_KEY,
    SSE_CUSTOMER_ALGORITHM_KEY,
    COPY_SOURCE_KEY,
    METADATA_DIRECTIVE_KEY,
    MAX_KEYS_KEY,
    PREFIX_KEY,
    DELIMITER_KEY,
    DATA_ACCESS_POINT_ARN_KEY,
    DATA_ACCESS_POINT_ACCOUNT_KEY,
    RESOURCE_ACCOUNT_KEY,
    PRINCIPAL_ARN_KEY,
    PRINCIPAL_ACCOUNT_KEY,
    MULTI_FACTOR_AUTH_PRESENT_KEY,
    TOKEN_ISSUE_TIME_KEY,
];

/// The keys a decision taken WITHOUT the caller resolved publishes ([`policy_denial`], whose
/// principal is this service's own stand-in rather than the identity that signed): every key
/// above except the ones naming the caller and its credential.
///
/// Publishing a key is saying this service ANSWERS it, and a key published with no value is
/// decided as ABSENT — which a negated clause over it HOLDS for. So a gate that cannot name
/// the caller must not publish the caller's keys: a clause over them stays undecided there,
/// and only [`super::request_authorization::denial`], which resolved the signing identity,
/// decides one.
pub const CALLER_FREE_CONDITION_KEYS: &[&str] = &[
    SECURE_TRANSPORT_KEY,
    ACL_KEY,
    STORAGE_CLASS_KEY,
    SERVER_SIDE_ENCRYPTION_KEY,
    KMS_KEY_ID_KEY,
    SSE_CUSTOMER_ALGORITHM_KEY,
    COPY_SOURCE_KEY,
    METADATA_DIRECTIVE_KEY,
    MAX_KEYS_KEY,
    PREFIX_KEY,
    DELIMITER_KEY,
    DATA_ACCESS_POINT_ARN_KEY,
    DATA_ACCESS_POINT_ACCOUNT_KEY,
    RESOURCE_ACCOUNT_KEY,
];

/// The keys of [`PUBLISHED_CONDITION_KEYS`] that only a decision which resolved the signing
/// identity can answer — who the caller is, which account it belongs to, and whether its
/// credential was MFA-backed — which are the difference between the two lists above; see
/// `the_two_published_lists_differ_by_the_caller_keys`.
#[cfg(test)]
const CALLER_CONDITION_KEYS: &[&str] = &[
    PRINCIPAL_ARN_KEY,
    PRINCIPAL_ACCOUNT_KEY,
    MULTI_FACTOR_AUTH_PRESENT_KEY,
    TOKEN_ISSUE_TIME_KEY,
];

/// The global condition key a gate answers from the account this emulator serves, for EVERY
/// request: which account owns the resource addressed ([`RESOURCE_ACCOUNT_KEY`]).
///
/// A clause over it is therefore DECIDED rather than left open, which is what makes a grant
/// scoped to the resource's owner — the shape a publishing or deployment role's policy is
/// written in — enforceable at all.
///
/// ONE place, so the bucket-policy half ([`policy_denial`]) and the full authorization gate
/// ([`super::request_authorization::denial`]) judge one document alike.
#[must_use]
pub fn resource_conditions() -> Vec<(String, String)> {
    vec![(
        RESOURCE_ACCOUNT_KEY.to_string(),
        ::engine::emulator_account_id().to_string(),
    )]
}

/// [`resource_conditions`] together with the keys naming the PRINCIPAL that signed the
/// request: who it is ([`PRINCIPAL_ARN_KEY`]) and which account it belongs to
/// ([`PRINCIPAL_ACCOUNT_KEY`]).
///
/// `identity_arn` is the ARN of the identity whose policies are in force — the ROLE behind an
/// assumed-role session, the user behind a long-term key (see [`PRINCIPAL_ARN_KEY`]). Only a
/// decision that RESOLVED the signing identity may use this, and it publishes
/// [`PUBLISHED_CONDITION_KEYS`]: a caller speaking for nobody carries no ARN, so its principal
/// keys are reported absent rather than guessed, while a gate holding no identity at all
/// publishes [`CALLER_FREE_CONDITION_KEYS`] and leaves a clause over them undecided.
#[must_use]
pub fn global_conditions(identity_arn: &str) -> Vec<(String, String)> {
    let mut context = resource_conditions();
    if identity_arn.is_empty() {
        return context;
    }
    context.push((PRINCIPAL_ARN_KEY.to_string(), identity_arn.to_string()));
    // The account is read out of the principal's ARN through the tree's one ARN parse, so an
    // ARN carrying none reports no account rather than a guess.
    if let Some(account) = ::service_common::arn::parse(identity_arn)
        .map(|parsed| parsed.account.to_string())
        .filter(|account| !account.is_empty())
    {
        context.push((PRINCIPAL_ACCOUNT_KEY.to_string(), account));
    }
    context
}

/// What the request's MFA key ([`MULTI_FACTOR_AUTH_PRESENT_KEY`]) answers for the credential
/// that signed it: one pair when the credential carries a value for the key, and NOTHING when
/// it carries none — a long-term access key, where the key is simply not in the request
/// context.
///
/// `present` is what the credential itself says
/// (`sts_store::Caller::multi_factor_auth_present`), read there rather than decided here, so
/// this service and every other one handed the same credential answer the key alike. The
/// value is IAM's boolean literal, which is what a `Bool` clause compares against.
#[must_use]
pub fn multi_factor_auth_conditions(present: Option<bool>) -> Vec<(String, String)> {
    present
        .map(|present| {
            (
                MULTI_FACTOR_AUTH_PRESENT_KEY.to_string(),
                present.to_string(),
            )
        })
        .into_iter()
        .collect()
}

/// What the request's SESSION-ISSUE-TIME key ([`TOKEN_ISSUE_TIME_KEY`]) answers for the
/// credential that signed it: one pair when the credential belongs to a session, and NOTHING
/// when it belongs to none — a long-term access key, where the key is simply not in the
/// request context.
///
/// `issued_at` is the instant the registry recorded for the session
/// (`sts_store::Caller::token_issue_time`), read there rather than rendered here, so this
/// service and every other one handed the same credential answer the key with the same text.
#[must_use]
pub fn token_issue_time_conditions(issued_at: Option<&str>) -> Vec<(String, String)> {
    issued_at
        .map(|issued_at| (TOKEN_ISSUE_TIME_KEY.to_string(), issued_at.to_string()))
        .into_iter()
        .collect()
}

/// The two keys naming the ACCESS POINT `route` a request came through
/// ([`DATA_ACCESS_POINT_ARN_KEY`], [`DATA_ACCESS_POINT_ACCOUNT_KEY`]), for the decision
/// taken on that request.
///
/// Only a request that really came through an access point carries them: both keys are
/// published ([`PUBLISHED_CONDITION_KEYS`]), so a request addressed to the bucket directly
/// answers them as absent and a statement conditioned on them does not admit it.
#[must_use]
pub fn access_point_conditions(
    route: &crate::state::access_points::AccessPointRoute,
) -> Vec<(String, String)> {
    vec![
        (DATA_ACCESS_POINT_ARN_KEY.to_string(), route.arn.clone()),
        (
            DATA_ACCESS_POINT_ACCOUNT_KEY.to_string(),
            route.account_id.clone(),
        ),
    ]
}

/// The number of keys a listing returns when the request names no `max-keys`: "sets the
/// maximum number of keys returned in the response. By default, the action returns up to
/// 1,000 key names" (`ListObjectsV2Request$MaxKeys`), so that is the figure a clause over
/// [`MAX_KEYS_KEY`] is decided against for a request that names none — the listing it would
/// return is that large.
pub const DEFAULT_MAX_KEYS: i32 = 1_000;

/// The members of a LISTING a bucket policy may be written over, as the request carried them.
///
/// One value per published listing condition key; the size of the listing is always known
/// (see [`DEFAULT_MAX_KEYS`]), while a `prefix` and a `delimiter` the request does not carry
/// are absent, which is itself part of the decision — a policy admitting a listing only
/// "when" it names a prefix refuses one that names none.
#[derive(Debug, Default, Clone, Copy)]
pub struct ListingContext<'a> {
    /// `max-keys` — the largest number of keys the listing may return.
    pub max_keys: Option<i32>,
    /// `prefix` — the key prefix the listing is confined to.
    pub prefix: Option<&'a str>,
    /// `delimiter` — the character the listing rolls common prefixes up on.
    pub delimiter: Option<&'a str>,
}

impl ListingContext<'_> {
    /// The request's condition context: the secure transport every endpoint of this emulator
    /// serves, the effective size of the listing, and one pair per member the request
    /// carried.
    #[must_use]
    pub fn conditions(&self) -> Vec<(String, String)> {
        let mut context = ObjectWriteContext::default().conditions();
        context.push((
            MAX_KEYS_KEY.to_string(),
            self.max_keys.unwrap_or(DEFAULT_MAX_KEYS).to_string(),
        ));
        for (key, value) in [(PREFIX_KEY, self.prefix), (DELIMITER_KEY, self.delimiter)] {
            if let Some(value) = value {
                context.push((key.to_string(), value.to_string()));
            }
        }
        context
    }
}

/// The members of an object write a bucket policy may be written over, as the request carried
/// them.
///
/// One value per published S3 request-header condition key, `None` for a header the request
/// does not carry — which is itself part of the decision: a policy denying a write "unless"
/// it names an encryption algorithm is a clause that HOLDS exactly when the header is absent.
#[derive(Debug, Default, Clone, Copy)]
pub struct ObjectWriteContext<'a> {
    /// `x-amz-acl` — the canned ACL the write names.
    pub acl: Option<&'a str>,
    /// `x-amz-storage-class` — the class the write asks the object to be stored in.
    pub storage_class: Option<&'a str>,
    /// `x-amz-server-side-encryption` — the algorithm the write asks for.
    pub server_side_encryption: Option<&'a str>,
    /// `x-amz-server-side-encryption-aws-kms-key-id` — the KMS key the write names.
    pub kms_key_id: Option<&'a str>,
    /// `x-amz-server-side-encryption-customer-algorithm` — the algorithm a write supplying
    /// its OWN key names.
    pub sse_customer_algorithm: Option<&'a str>,
    /// `x-amz-copy-source` — the object a copy reads, for the write half of a copy.
    pub copy_source: Option<&'a str>,
    /// `x-amz-metadata-directive` — whether a copy keeps or replaces the source's metadata.
    pub metadata_directive: Option<&'a str>,
}

impl ObjectWriteContext<'_> {
    /// The request's condition context: the secure transport every endpoint of this emulator
    /// serves, plus one pair per member the request carried.
    pub fn conditions(&self) -> Vec<(String, String)> {
        let mut context = vec![(
            SECURE_TRANSPORT_KEY.to_string(),
            SECURE_TRANSPORT_VALUE.to_string(),
        )];
        for (key, value) in [
            (ACL_KEY, self.acl),
            (STORAGE_CLASS_KEY, self.storage_class),
            (SERVER_SIDE_ENCRYPTION_KEY, self.server_side_encryption),
            (KMS_KEY_ID_KEY, self.kms_key_id),
            (SSE_CUSTOMER_ALGORITHM_KEY, self.sse_customer_algorithm),
            (COPY_SOURCE_KEY, self.copy_source),
            (METADATA_DIRECTIVE_KEY, self.metadata_directive),
        ] {
            if let Some(value) = value {
                context.push((key.to_string(), value.to_string()));
            }
        }
        context
    }
}

/// The condition context a request that carries none of the write members answers with: the
/// secure transport alone, which is what a read's own clauses are judged against.
pub fn read_conditions() -> Vec<(String, String)> {
    ObjectWriteContext::default().conditions()
}

/// Why the bucket's `policy` refuses the calling principal `action` on `resource` — the
/// sentence IAM publishes for an explicit deny in a resource-based policy — or `None` when it
/// leaves the call alone.
///
/// The principal is the one a credentialed request here speaks for
/// ([`caller_principal_arn`]) together with the spellings that name its whole account, since
/// a statement denying the account refuses each of its principals. Only a DENY is reported:
/// this emulator serves one account and the caller is that account, so a document that says
/// nothing about the request leaves the bucket's own owner able to act on it.
///
/// This decision does NOT resolve the signing identity — the principal above is this
/// service's stand-in for the account itself — so it answers only the resource's owning
/// account ([`resource_conditions`]) and publishes [`CALLER_FREE_CONDITION_KEYS`]: a clause
/// over who the caller is stays undecided here rather than being decided against a principal
/// nothing read off the request.
pub fn policy_denial(
    policy: Option<&str>,
    action: &str,
    resource: &str,
    conditions: &[(String, String)],
) -> Option<String> {
    let policy = policy?;
    let caller = caller_principal_arn();
    let mut principals = vec![caller.clone()];
    principals.extend(account_principals(&caller));
    let mut context = conditions.to_vec();
    context.extend(resource_conditions());
    let outcome = resource_outcome_prepared(
        &PreparedDocument::read(policy),
        &Request::new(action, resource, &principals)
            .with_context(&context)
            .with_published_keys(CALLER_FREE_CONDITION_KEYS),
    );
    deny_is_decided(&outcome, CALLER_FREE_CONDITION_KEYS)
        .then(|| explicit_resource_policy_denial(action, resource))
}

/// Whether `outcome` is a deny this decision could DECIDE — the only kind that refuses.
///
/// A key the decision needed and this service does not answer leaves the statement undecided,
/// and the evaluation's reading of an undecided `Deny` is that it still refuses — fail-closed,
/// which is right where a GRANT is at stake and wrong for a refusal, because it would refuse
/// calls on every bucket whose policy conditions anything nothing here models. A key the
/// decision DID publish is decided by its absence, so a report of one says only that the
/// request carried no value for it.
///
/// `published` is the list THIS decision published ([`PUBLISHED_CONDITION_KEYS`] where the
/// signing identity was resolved, [`CALLER_FREE_CONDITION_KEYS`] where it was not), because
/// "answered" is a property of the decision rather than of the service: a principal key the
/// caller-free decision could not answer must leave its statement undecided there even though
/// the full gate answers it.
///
/// One place, so the bucket-policy half ([`policy_denial`]) and the full authorization gate
/// ([`super::request_authorization::denial`]) read an undecided deny the same way.
#[must_use]
pub fn deny_is_decided(outcome: &Outcome, published: &[&str]) -> bool {
    outcome.decision == Decision::Denied
        && !outcome
            .required_context_keys
            .iter()
            .any(|key| !models(key, published))
}

/// Whether the decision that published `published` ANSWERS the condition key `key`, compared
/// the way a condition key's name is compared everywhere.
fn models(key: &str, published: &[&str]) -> bool {
    published
        .iter()
        .any(|known| known.eq_ignore_ascii_case(key))
}

/// Why a request naming `expected` as the account that owns the bucket it addresses is
/// refused, or `None` for one that names the owner or names no account at all.
///
/// "The account ID of the expected bucket owner. If the account ID that you provide does not
/// match the actual owner of the bucket, the request fails with the HTTP status code 403
/// Forbidden (access denied)" — the `ExpectedBucketOwner` member every bucket and object
/// operation of this model binds to `x-amz-expected-bucket-owner`. This emulator serves ONE
/// account, so that account owns every bucket it holds and any other value names an owner no
/// bucket here has.
///
/// One place, so the confused-deputy guard is the same answer on every operation that carries
/// the header, and judged BEFORE the store is reached: a mismatched guard says nothing about
/// what the bucket holds.
pub fn expected_bucket_owner_refusal<E>(
    expected: Option<&str>,
) -> Option<crate::scaffolded::Scaffolded<E>> {
    expected
        .filter(|account| *account != ::engine::emulator_account_id())
        .map(|_| {
            crate::scaffolded::Scaffolded::unmodeled(
                ACCESS_DENIED_CODE,
                ACCESS_DENIED_STATUS,
                ACCESS_DENIED_MESSAGE,
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The two published lists differ by exactly the caller's keys: the full list is what a
    /// decision that resolved the signing identity publishes, and the caller-free one is that
    /// list without the keys only such a decision can answer.
    #[test]
    fn the_two_published_lists_differ_by_the_caller_keys() {
        for key in CALLER_FREE_CONDITION_KEYS {
            assert!(
                PUBLISHED_CONDITION_KEYS.contains(key),
                "case {key:?}: expected every caller-free key to be published by the full \
                 gate too, got none"
            );
        }
        for key in CALLER_CONDITION_KEYS {
            assert!(
                PUBLISHED_CONDITION_KEYS.contains(key) && !CALLER_FREE_CONDITION_KEYS.contains(key),
                "case {key:?}: expected a caller key to be published only where the caller \
                 is resolved, got caller_free={}",
                CALLER_FREE_CONDITION_KEYS.contains(key)
            );
        }
        let difference = PUBLISHED_CONDITION_KEYS.len() - CALLER_FREE_CONDITION_KEYS.len();
        assert_eq!(
            difference,
            CALLER_CONDITION_KEYS.len(),
            "case \"the two lists\": expected them to differ by the {} caller key(s), got \
             {difference}",
            CALLER_CONDITION_KEYS.len()
        );
    }

    /// A clause over who the CALLER is stays undecided on the bucket-policy-only decision,
    /// which never resolved the signing identity: neither a deny that names this service's
    /// stand-in principal nor one that excludes it may refuse the call there.
    #[test]
    fn a_principal_clause_is_undecided_without_a_resolved_caller() {
        let caller = caller_principal_arn();
        let documents: &[(&str, String)] = &[
            (
                "a deny naming the stand-in principal",
                format!(
                    "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Effect\":\"Deny\",\
                     \"Principal\":\"*\",\"Action\":\"s3:PutObject\",\
                     \"Resource\":\"arn:aws:s3:::subject/*\",\"Condition\":\
                     {{\"StringEquals\":{{\"aws:PrincipalArn\":\"{caller}\"}}}}}}]}}"
                ),
            ),
            (
                "a deny over every principal but one",
                "{\"Version\":\"2012-10-17\",\"Statement\":[{\"Effect\":\"Deny\",\
                 \"Principal\":\"*\",\"Action\":\"s3:PutObject\",\
                 \"Resource\":\"arn:aws:s3:::subject/*\",\"Condition\":\
                 {\"StringNotEquals\":{\"aws:PrincipalArn\":\
                 \"arn:aws:iam::000000000000:role/publisher\"}}}]}"
                    .to_string(),
            ),
        ];
        for (case, document) in documents {
            let denial = policy_denial(
                Some(document),
                "s3:PutObject",
                "arn:aws:s3:::subject/reports/held.csv",
                &read_conditions(),
            );
            assert_eq!(
                denial, None,
                "case {case:?}: expected the clause to stay undecided where the caller is not \
                 resolved, got {denial:?}"
            );
        }
    }

    /// The account owning the resource IS answered on that same decision, so a deny scoped to
    /// it refuses — which is what makes a bucket policy written over the owner enforceable.
    #[test]
    fn the_resource_account_is_answered_without_a_resolved_caller() {
        let document = format!(
            "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Effect\":\"Deny\",\
             \"Principal\":\"*\",\"Action\":\"s3:PutObject\",\
             \"Resource\":\"arn:aws:s3:::subject/*\",\"Condition\":\
             {{\"StringEquals\":{{\"aws:ResourceAccount\":\"{}\"}}}}}}]}}",
            ::engine::emulator_account_id()
        );
        let denial = policy_denial(
            Some(&document),
            "s3:PutObject",
            "arn:aws:s3:::subject/reports/held.csv",
            &read_conditions(),
        );
        assert!(
            denial.is_some(),
            "case \"a deny scoped to the resource's owning account\": expected the refusal, got \
             {denial:?}"
        );
    }

    /// A document denying `s3:PutObject` to everyone unless the write names the encryption
    /// algorithm `required`, which is how a bucket makes encryption mandatory.
    fn deny_unencrypted(required: &str) -> String {
        format!(
            "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Effect\":\"Deny\",\
             \"Principal\":\"*\",\"Action\":\"s3:PutObject\",\
             \"Resource\":\"arn:aws:s3:::subject/*\",\"Condition\":\
             {{\"StringNotEquals\":{{\"s3:x-amz-server-side-encryption\":\"{required}\"}}}}}}]}}"
        )
    }

    /// The clause a policy that requires the header's PRESENCE is written as instead.
    const DENY_WITHOUT_THE_HEADER: &str = "{\"Version\":\"2012-10-17\",\"Statement\":\
         [{\"Effect\":\"Deny\",\"Principal\":\"*\",\"Action\":\"s3:PutObject\",\
         \"Resource\":\"arn:aws:s3:::subject/*\",\"Condition\":\
         {\"Null\":{\"s3:x-amz-server-side-encryption\":\"true\"}}}]}";

    /// A condition over a key this service does not answer leaves the deny undecided.
    const DENY_ON_AN_UNMODELLED_KEY: &str = "{\"Version\":\"2012-10-17\",\"Statement\":\
         [{\"Effect\":\"Deny\",\"Principal\":\"*\",\"Action\":\"s3:PutObject\",\
         \"Resource\":\"arn:aws:s3:::subject/*\",\"Condition\":\
         {\"StringEquals\":{\"s3:ExistingObjectTag/stage\":\"production\"}}}]}";

    /// The write's own members decide a clause written over them: the rows differ along which
    /// algorithm the write names (none, the required one, another one) and which form the
    /// clause takes (a negated comparison, an existence test, a key nothing here answers).
    #[test]
    fn a_clause_over_a_request_header_is_decided_by_that_header() {
        let rows: [(&str, String, Option<&str>, bool); 7] = [
            (
                "no encryption header against a policy requiring aws:kms",
                deny_unencrypted("aws:kms"),
                None,
                true,
            ),
            (
                "the required algorithm",
                deny_unencrypted("aws:kms"),
                Some("aws:kms"),
                false,
            ),
            (
                "another algorithm than the required one",
                deny_unencrypted("aws:kms"),
                Some("AES256"),
                true,
            ),
            (
                "no encryption header against a policy requiring AES256",
                deny_unencrypted("AES256"),
                None,
                true,
            ),
            (
                "the required AES256",
                deny_unencrypted("AES256"),
                Some("AES256"),
                false,
            ),
            (
                "no encryption header against a policy requiring its presence",
                DENY_WITHOUT_THE_HEADER.to_string(),
                None,
                true,
            ),
            (
                "a deny over a condition key this service does not answer",
                DENY_ON_AN_UNMODELLED_KEY.to_string(),
                None,
                false,
            ),
        ];
        for (case, policy, algorithm, refused) in rows {
            let context = ObjectWriteContext {
                server_side_encryption: algorithm,
                ..ObjectWriteContext::default()
            };
            let denial = policy_denial(
                Some(&policy),
                "s3:PutObject",
                "arn:aws:s3:::subject/reports/held.csv",
                &context.conditions(),
            );
            assert_eq!(
                denial.is_some(),
                refused,
                "case {case:?}: expected refused={refused}, got {denial:?}"
            );
        }
    }

    /// The guard compares the account the request names against the one account this emulator
    /// serves: the rows differ along whose account it names.
    #[test]
    fn the_expected_bucket_owner_guard_admits_only_the_owner() {
        let owner = ::engine::emulator_account_id().to_string();
        let rows: [(&str, Option<&str>, bool); 4] = [
            ("no account named at all", None, false),
            ("the account this emulator serves", Some(&owner), false),
            ("another account of the same shape", Some("123456789012"), true),
            ("a value that is no account id", Some("not-an-account"), true),
        ];
        for (case, expected, refused) in rows {
            let refusal = expected_bucket_owner_refusal::<()>(expected);
            assert_eq!(
                refusal.is_some(),
                refused,
                "case {case:?}: expected refused={refused}, got {:?}",
                refusal.is_some()
            );
        }
    }
}
