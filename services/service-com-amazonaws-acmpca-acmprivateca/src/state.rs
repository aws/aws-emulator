// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-acmpca-acmprivateca service.
//!
//! Everything the private-CA lifecycle persists lives here, beside the helpers the
//! operation handlers share: ARN parsing, the clock-owed status transitions, validity
//! resolution and the PEM bodies the service hands back. Handlers hold no state of
//! their own — they reach this store through [`service_state`].
//!
//! Several concerns of the store live in files of their own beside this one, and are
//! re-exported from here so every handler still reaches them as `crate::state::*`: the key
//! pair a CA signs with ([`key_material`]), the extensions the certification request it
//! publishes asks for ([`csr_extensions`]), what makes a presented certificate the CA's own
//! ([`imported_certificate`]) and what makes it chain up to the chain presented with it
//! ([`certificate_chain`]), the audit reports a CA produces over the certificates it
//! has issued ([`audit_reports`]) and the CloudWatch series a CA publishes about the
//! certificates it issues ([`issuance_metrics`]).

mod audit_reports;
pub mod authorization;
mod certificate_chain;
mod certificate_templates;
mod csr_extensions;
mod imported_certificate;
mod issuance_metrics;
mod issued_extensions;
mod key_material;
mod managed_issuance;

pub use audit_reports::*;
pub use certificate_chain::*;
pub use certificate_templates::*;
pub use csr_extensions::*;
pub use imported_certificate::*;
pub use issuance_metrics::*;
pub use issued_extensions::*;
pub use key_material::*;
pub use managed_issuance::*;

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};
use x509_cert::attr::AttributeTypeAndValue;
use x509_cert::crl::{CertificateList, RevokedCert, TbsCertList};
use x509_cert::der::asn1::{Any, BitString, OctetString, SetOfVec};
use x509_cert::der::{Decode, Encode, Tag as DerTag};
use x509_cert::ext::{Extension, Extensions};
use x509_cert::name::{Name, RdnSequence, RelativeDistinguishedName};
use x509_cert::request::{CertReq, CertReqInfo};
use x509_cert::serial_number::SerialNumber;
use x509_cert::spki::{AlgorithmIdentifierOwned, ObjectIdentifier, SubjectPublicKeyInfoOwned};
use x509_cert::time::{Time, Validity as CertificateValidity};
use x509_cert::{Certificate, TbsCertificate, Version};

use crate::ops::types::{
    ApiPassthrough, ASN1Subject, CertificateAuthority, CertificateAuthorityConfiguration,
    CrlConfiguration, CsrExtensions, KeyUsage, Permission, RevocationConfiguration, Tag,
    Validity,
};

/// A PKCS#10 certification request, as this service reads and writes one.
pub type CertificationRequest = CertReq;

/// The content of a certification request: the subject and the public key a certificate
/// issued for it carries.
pub type CertificationRequestInfo = CertReqInfo;

/// The ARN namespace / sigv4 signing name of this service (`awsService.arnNamespace`).
pub const ARN_SERVICE: &str = "acm-pca";

/// The region used when a request carries no SigV4 credential scope to read one from.
pub const DEFAULT_REGION: &str = "us-east-1";

// `CertificateAuthorityStatus` members this slice uses (see
// `CERTIFICATE_AUTHORITY_STATUS_VALUES` in `ops::types` for the whole enum).
/// Private CA is being created; its CSR is not available yet.
pub const STATUS_CREATING: &str = "CREATING";
/// Private CA exists and waits for its signed CA certificate to be imported.
pub const STATUS_PENDING_CERTIFICATE: &str = "PENDING_CERTIFICATE";
/// Private CA holds its certificate and may issue end-entity certificates.
pub const STATUS_ACTIVE: &str = "ACTIVE";
/// Private CA was deleted; it is restorable while its restoration window is open.
pub const STATUS_DELETED: &str = "DELETED";
/// Private CA was disabled, so it issues nothing until it is enabled again.
pub const STATUS_DISABLED: &str = "DISABLED";
/// Private CA's certificate has expired.
pub const STATUS_EXPIRED: &str = "EXPIRED";
/// Private CA could not be created.
pub const STATUS_FAILED: &str = "FAILED";

/// The `CertificateAuthorityStatus` members a CA can be DELETED from: "A private CA can
/// be deleted if it is in the PENDING_CERTIFICATE, CREATING, EXPIRED, DISABLED, or FAILED
/// state. To delete a CA in the ACTIVE state, you must first disable it, or else the
/// delete request results in an exception." (`DeleteCertificateAuthority` docs).
pub const DELETABLE_STATUSES: &[&str] = &[
    STATUS_PENDING_CERTIFICATE,
    STATUS_CREATING,
    STATUS_EXPIRED,
    STATUS_DISABLED,
    STATUS_FAILED,
];

/// The statuses a deletion opens NO restoration window from: "A private CA deleted in the
/// CREATING or FAILED state has no assigned restoration period and cannot be restored."
/// (`DeleteCertificateAuthority` docs).
pub const UNRESTORABLE_DELETION_STATUSES: &[&str] = &[STATUS_CREATING, STATUS_FAILED];

/// The `CertificateAuthorityStatus` members a CA can be UPDATED in: "Your private CA must
/// be in the ACTIVE or DISABLED state before you can update it. You can disable a private
/// CA that is in the ACTIVE state or make a CA that is in the DISABLED state active
/// again." (`UpdateCertificateAuthority` docs) — the same two members are therefore the
/// only statuses that call can put a CA INTO, since those two transitions are all it
/// performs.
pub const UPDATABLE_STATUSES: &[&str] = &[STATUS_ACTIVE, STATUS_DISABLED];

/// What Amazon Web Services Private CA answers when a CA is asked to be deleted in a
/// status that does not allow deletion: one sentence about the statuses deletion is
/// allowed from, naming no request of its own. The statuses are the ones
/// [`DELETABLE_STATUSES`] quotes from the `DeleteCertificateAuthority` docs.
pub const DELETE_INVALID_STATE_MESSAGE: &str =
    "The certificate authority is not in a valid state for deletion.";

/// What Amazon Web Services Private CA answers when a CA is asked to take a certificate
/// authority certificate in a status that cannot take one: one sentence about the CA, in
/// the form every state refusal of this service takes ([`ISSUANCE_INVALID_STATE_MESSAGE`],
/// [`DELETE_INVALID_STATE_MESSAGE`]) and carrying no ARN, status name or other value out of
/// the request or the store.
///
/// A CA that was soft-deleted is answered the restore sentence instead
/// ([`DELETED_INVALID_STATE_MESSAGE`]), because the action it has to be restored for is
/// what that refusal says.
pub const IMPORT_INVALID_STATE_MESSAGE: &str =
    "The certificate authority is not in a valid state for importing a certificate \
     authority certificate.";

/// What Amazon Web Services Private CA answers when a deleted CA is asked to change:
/// the service's own sentence for this refusal, naming the state the CA is in and the
/// restore that clears it, and carrying no value out of the request or the store. A CA in
/// `DELETED` is waiting to be restored or permanently deleted, so what it holds is not
/// writable until `RestoreCertificateAuthority` puts it back into the status it was
/// deleted from — which is a different refusal from the one a CA outside ACTIVE/DISABLED
/// gets from `UpdateCertificateAuthority`, so the two keep their own wording.
pub const DELETED_INVALID_STATE_MESSAGE: &str =
    "The certificate authority is in the DELETED state and must be restored to complete \
     this action.";

/// What Amazon Web Services Private CA answers when a CA's status or configuration is
/// asked to change from a status that does not allow the change: one sentence naming the
/// two statuses an update is allowed from ([`UPDATABLE_STATUSES`]), carrying no ARN,
/// status name, timestamp or other value out of the request or the store, so it is the
/// same sentence whichever CA and whichever requested change was refused.
///
/// Distinct from [`DELETED_INVALID_STATE_MESSAGE`] on purpose: both are
/// `InvalidStateException`, and a caller tells the two refusals apart by the text.
pub const UPDATE_INVALID_STATE_MESSAGE: &str =
    "The certificate authority must be in the ACTIVE or DISABLED state to be updated";

/// The restoration window a deletion opens when the request names none: "By default, if
/// you do not set the PermanentDeletionTimeInDays parameter, the CA remains restorable
/// for 30 days." (`DeleteCertificateAuthority` docs).
pub const DEFAULT_PERMANENT_DELETION_TIME_IN_DAYS: i32 = 30;

/// The bounds `PermanentDeletionTimeInDays` publishes: the member is modelled
/// `range 7..30` — "You can set the parameter from 7 to 30 days."
/// (`DeleteCertificateAuthority` docs).
pub const MIN_PERMANENT_DELETION_TIME_IN_DAYS: i32 = 7;
/// The upper bound of that range.
pub const MAX_PERMANENT_DELETION_TIME_IN_DAYS: i32 = 30;

/// The unit `PermanentDeletionTimeInDays` counts in.
pub const ONE_DAY: Duration = Duration::from_secs(24 * 60 * 60);

// `ResourceOwner` members a listing filters on (see `RESOURCE_OWNER_VALUES` in
// `ops::types` for the whole enum).
/// The CAs the calling account owns — "The default is SELF."
/// (`ListCertificateAuthoritiesRequest$ResourceOwner` docs).
pub const RESOURCE_OWNER_SELF: &str = "SELF";
/// The CAs another account owns and shared with the calling account.
pub const RESOURCE_OWNER_OTHER_ACCOUNTS: &str = "OTHER_ACCOUNTS";

/// What Amazon Web Services Private CA answers while a CA's creation is still in
/// progress — one sentence, the same for every operation that refuses on `CREATING`
/// rather than one built from the request.
pub const CREATE_IN_PROGRESS_MESSAGE: &str =
    "The request to create certificate authority is still in progress. Try again later.";

/// What Amazon Web Services Private CA answers when a request carries a tag key in the
/// reserved `aws:` namespace: the service's own sentence for the refusal, not one built
/// from the request.
pub const RESERVED_TAG_MESSAGE: &str = "Tags starting with 'aws' are categorized as AWS \
     internal tags and cannot be used for tagging in ACM Private CA service.";

/// What Amazon Web Services Private CA answers for an Amazon Resource Name that is not
/// one of its own: one sentence about the ARN, the same for every operation that reads
/// one.
pub const INVALID_ARN_MESSAGE: &str = "Not a valid arn.";

/// What Amazon Web Services Private CA answers for a certificate body it cannot read:
/// one sentence about the certificate, the same for the certificate and for the chain.
pub const MALFORMED_CERTIFICATE_MESSAGE: &str = "Malformed certificate.";

/// What Amazon Web Services Private CA answers for a certification request it cannot read:
/// one sentence about the request, the counterpart of
/// [`MALFORMED_CERTIFICATE_MESSAGE`] and, like it, naming nothing of the request's own.
pub const MALFORMED_CSR_MESSAGE: &str = "Malformed CSR.";

/// What Amazon Web Services Private CA answers when the certificate a request asks for
/// would still be valid after the issuing CA's own certificate has expired: "The end of the
/// validity period configured on a certificate must not exceed the limit set on its parents
/// in the CA hierarchy." (`IssueCertificateRequest$Validity` docs.)
///
/// Wholly fixed text: neither the requested validity nor the CA's appears in it.
pub const VALIDITY_EXCEEDS_CA_MESSAGE: &str =
    "The certificate validity specified exceeds the certificate authority validity.";

/// What Amazon Web Services Private CA answers when the certificate a request asks for would
/// be valid for longer than the issuing CA's usage mode allows ("Short-lived certificate
/// validity is limited to seven days", `CreateCertificateAuthorityRequest$UsageMode`).
///
/// Wholly fixed text as well: it states neither the requested validity, nor the limit, nor
/// the mode that sets it — what it tells the caller is the two ways out, a shorter validity
/// or a CA of another mode.
pub const VALIDITY_EXCEEDS_USAGE_MODE_MESSAGE: &str =
    "The requested certificate validity exceeds the maximum allowed by the usage mode. Reduce \
     the requested validity period to be equal to or less than that supported by the usage \
     mode for this CA or use a CA with a different usage mode.";

/// The `CertificateAuthorityType` member of a CA that signs its own certificate: "ROOT"
/// (`CreateCertificateAuthorityRequest$CertificateAuthorityType`). A `SUBORDINATE` CA's
/// certificate is signed by another CA, so only this kind self-signs.
pub const CA_TYPE_ROOT: &str = "ROOT";

/// The `TemplateArn` segment of the certificate template that produces a ROOT CA's own
/// certificate: `arn:aws:acm-pca:::template/RootCACertificate/V1` and any later version of
/// it. Matched as the segment rather than as a whole ARN, because the version is the
/// template's own and the rule is about which KIND of certificate was asked for.
pub const ROOT_CA_CERTIFICATE_TEMPLATE: &str = ":template/RootCACertificate/";

/// What Amazon Web Services Private CA answers when a CA is asked to issue a certificate
/// in a state that does not allow issuance: one sentence about the CA, naming no request
/// of its own.
pub const ISSUANCE_INVALID_STATE_MESSAGE: &str =
    "The certificate authority is not in a valid state for issuing certificates";

/// The `CertificateAuthorityStatus` members a CA can REVOKE one of its certificates in.
///
/// Revocation is done with the CA's private key and published in the CA's CRL, so the CA
/// has to hold its certificate and still be usable: "ACTIVE — the CA is active and can be
/// used to issue and revoke certificates", and a CA that was disabled "can no longer issue
/// certificates, but it continues to revoke them and to publish CRLs" (the private CA
/// lifecycle). Every other status either has no certificate of its own yet
/// (`CREATING`, `PENDING_CERTIFICATE`), cannot sign with it any more (`EXPIRED`,
/// `FAILED`), or is waiting to be restored (`DELETED`).
pub const REVOCABLE_STATUSES: &[&str] = &[STATUS_ACTIVE, STATUS_DISABLED];

/// What Amazon Web Services Private CA answers when the private CA a request names is not
/// in a status it could have issued the certificates the request is about from (for a
/// revocation, [`REVOCABLE_STATUSES`]): one sentence about the CA, fixed text around the
/// `CertificateAuthorityArn` the request carried, which is the only value in it.
///
/// It is about the AUTHORITY, so it answers a request that names one of a CA's certificates
/// only where the authority's status is what stands in the way of the action asked for: a
/// revocation signs and publishes with the CA's key ([`REVOCABLE_STATUSES`]), and a CA a
/// deletion withdrew from service reaches none of what it holds until it is restored. A read
/// needs no key, so the certificate the request names is resolved first whatever status the
/// CA rests in: one the CA HOLDS is served, and an ARN it holds no record of is the absence
/// of that certificate ([`CERTIFICATE_NOT_FOUND_MESSAGE`]), which is terminal.
///
/// Named after the SENTENCE rather than after one operation, because it is the answer
/// every operation of this service that reaches for a certificate of a CA in such a status
/// carries.
pub fn not_in_state_to_have_issued_message(certificate_authority_arn: &str) -> String {
    format!(
        "The certificate authority {certificate_authority_arn} is not in the correct \
         state to have issued certificates."
    )
}

/// What Amazon Web Services Private CA answers when the certificate a revocation names
/// has already been revoked: NOTHING — the refusal's `RequestAlreadyProcessedException`
/// carries an empty message, so the code alone says the request was already completed.
pub const CERTIFICATE_ALREADY_REVOKED_MESSAGE: &str = "";

/// What Amazon Web Services Private CA answers when the certificate a revocation names is
/// the self-signed certificate of the root CA that holds it: the refusal's own sentence,
/// wholly fixed text that interpolates no value from the request or the store.
///
/// The RULE is the documented one — "You cannot revoke a root CA self-signed certificate."
/// (`RevokeCertificate` docs) — while the sentence below is the one a recorded refusal of
/// Amazon Web Services Private CA carries for it, which is shorter than the documentation's
/// phrasing of the rule.
pub const ROOT_SELF_SIGNED_REVOCATION_MESSAGE: &str = "Self-signed certificate can not be revoked.";

/// What Amazon Web Services Private CA answers for a certificate ARN no certificate of
/// the CA matches: one sentence about the certificate, naming no ARN of its own.
pub const CERTIFICATE_NOT_FOUND_MESSAGE: &str = "Could not find specified certificate.";

/// What Amazon Web Services Private CA answers when a request names no private CA it can
/// resolve: one sentence about the CA, naming no ARN of its own, the same for every
/// operation that resolves one.
///
/// It says the AUTHORITY is absent, so it is answered only for an ARN no private CA of
/// this service answers to — never for a CA that exists in a status the caller did not
/// expect, and never for a sub-resource such a CA does not carry, which has its own
/// sentence ([`POLICY_NOT_FOUND_MESSAGE`], [`PERMISSION_NOT_FOUND_MESSAGE`]).
pub const CA_NOT_FOUND_MESSAGE: &str = "Could not find specified CA";

/// The prefix every `InvalidPolicyException` message of this service carries, under which
/// the rest of the sentence names the ONE rule the submitted document broke.
///
/// Each refusal of a resource-based policy document is a different diagnosis — the document
/// cannot be read as a policy at all ([`malformed_policy_message`]), a statement does not
/// grant ([`NON_ALLOW_EFFECT_MESSAGE`]), it grants an action a certificate authority cannot
/// be shared for ([`DISALLOWED_POLICY_ACTION_MESSAGE`]), it names an identity that is not
/// here ([`POLICY_PRINCIPAL_NOT_FOUND_MESSAGE`]) or it is about another resource
/// ([`different_resource_message`]) — so no two of them carry the same string and a caller
/// branching on the diagnostic can tell a document to repair from one that can never be
/// accepted.
pub const INVALID_POLICY_PREFIX: &str = "InvalidPolicy: ";

/// What Amazon Web Services Private CA answers for a submitted document it cannot read as a
/// policy AT ALL: the document itself, after [`INVALID_POLICY_PREFIX`].
///
/// A refusal names the rule that was broken, and the rule a document of this class broke is
/// about the document rather than about anything a policy may say — there is no statement to
/// blame, because reading the statements is what failed. So the message is BUILT FROM THE
/// REQUEST: the rejected text verbatim, byte for byte as it was submitted and neither
/// re-rendered nor clipped, which is what lets a caller submitting documents in a loop see
/// WHICH one was rejected.
///
/// The class is the whole of what [`is_well_formed_policy`] reads: a body that is not JSON,
/// and a JSON body that does not carry the policy grammar's required structure (no
/// `Statement`, an empty statement list, a statement that is not an object, a statement with
/// no `Effect` of the policy language or no `Action`). A document that IS a policy and says
/// something a private CA's policy cannot say is named by ITS rule instead, so such a
/// document is never echoed.
pub fn malformed_policy_message(document: &str) -> String {
    format!("{INVALID_POLICY_PREFIX}{document}")
}

/// What Amazon Web Services Private CA answers when a deleted CA has no restoration
/// period to be restored inside: "A private CA deleted in the CREATING or FAILED state
/// has no assigned restoration period and cannot be restored."
/// (`DeleteCertificateAuthority` docs).
pub const RESTORE_WITHOUT_RESTORATION_PERIOD_MESSAGE: &str =
    "The certificate authority is not in a valid state for restoration; it was deleted \
     with no assigned restoration period.";

/// The sentence this service answers a permission-not-found refusal with: one sentence
/// about the permission the request named, in the form the service's other not-found
/// refusals take ([`CERTIFICATE_NOT_FOUND_MESSAGE`]) and naming no principal of its own.
///
/// The wording is the one a recorded refusal of Amazon Web Services Private CA carries
/// for a `DeletePermission` naming a grant the CA does not hold. The
/// `ResourceNotFoundException` code and its HTTP 400 are what the refusal HAS to be; the
/// sentence is an example of the wording.
pub const PERMISSION_NOT_FOUND_MESSAGE: &str = "Could not find a Permission for specified CA.";

/// The `ActionType` members in the order the model DECLARES them, which is the order a
/// refusal of an incomplete grant lists them in
/// ([`INCOMPLETE_PERMISSION_ACTIONS_MESSAGE`]).
///
/// `ops::types::ACTION_TYPE_VALUES` is the same set sorted alphabetically, so it cannot be
/// read for this order; the order below is the enum's, from
/// `smithy_query(query="shape",shape="ActionType")`. A test pins that the two hold the same
/// members, so a member added to the model cannot go unnoticed here.
pub const ACTIONS_IN_MODEL_ORDER: &[&str] =
    &["IssueCertificate", "GetCertificate", "ListPermissions"];

/// What Amazon Web Services Private CA answers a grant that does not name every action a
/// private CA's permission can carry: "in order for ACM to automatically renew private
/// certificates, you must give the ACM service principal all available permissions
/// (IssueCertificate, GetCertificate, and ListPermissions)" (`Permission` docs), so a grant
/// naming only part of that set is refused rather than recorded.
///
/// Entirely fixed text — the bracketed actions are the `ActionType` members in the order
/// the model declares them ([`ACTIONS_IN_MODEL_ORDER`]) and nothing in the sentence is
/// taken from the request.
pub const INCOMPLETE_PERMISSION_ACTIONS_MESSAGE: &str =
    "Permissions must contain all three actions [IssueCertificate, GetCertificate, \
     ListPermissions] for ACM to perform renewals.";

/// What this service answers when the (CA, principal, source account) a grant names
/// already holds a permission allowing a DIFFERENT set of actions: the model's own
/// sentence for `PermissionAlreadyExistsException`, naming no value of the request's own.
///
/// Re-asserting the permission already in effect is not this refusal — it is answered with
/// success ([`PermissionRecord::allows_exactly`]).
pub const PERMISSION_ALREADY_EXISTS_MESSAGE: &str =
    "The designated permission has already been given to the user.";

/// What Amazon Web Services Private CA answers when the private CA a request names is
/// resolved and the resource-based policy asked for is not attached to it.
///
/// `GetPolicy`'s docs make the two absences one CODE — "If either the private CA resource
/// or the policy cannot be found, this action returns a ResourceNotFoundException" — and
/// the sentence is what says WHICH resource was not found: this one is about the POLICY, so
/// it is the answer for an authority that exists and carries none, while an ARN that names
/// no authority of this service is answered with [`CA_NOT_FOUND_MESSAGE`]. Both are
/// recorded (`acm-pca.get-policy-not-found` for a live CA with no policy,
/// `acm-pca.ca-arn-validation-precedes-resource-lookup` for an ARN no CA answers to), and
/// neither names a resource of the request's own.
///
/// It belongs to the READ: `DeletePolicy` is delete-if-exists and reports no absent policy
/// at all ("If there is no policy attached to the private CA, this action will return
/// successful", its docs).
pub const POLICY_NOT_FOUND_MESSAGE: &str = "Could not find policy for specified resource.";

/// `SigningAlgorithm`'s members in the order the model DECLARES them, which is the order
/// Amazon Web Services Private CA lists the usable algorithms in when it refuses a
/// key/signing-algorithm pairing.
///
/// `ops::types::SIGNING_ALGORITHM_VALUES` is the same set sorted alphabetically, so it
/// cannot be read for this; the order below is the enum's, from
/// `smithy_query(query="shape",shape="SigningAlgorithm")`. A test pins that the two hold
/// the same members, so a member added to the model cannot go unnoticed here.
pub const SIGNING_ALGORITHMS_IN_MODEL_ORDER: &[&str] = &[
    "SHA256WITHECDSA",
    "SHA384WITHECDSA",
    "SHA512WITHECDSA",
    "SHA256WITHRSA",
    "SHA384WITHRSA",
    "SHA512WITHRSA",
    "SHA256WITHRSA_PSS",
    "SHA384WITHRSA_PSS",
    "SHA512WITHRSA_PSS",
    "SM3WITHSM2",
    "ML_DSA_44",
    "ML_DSA_65",
    "ML_DSA_87",
];

/// Default `UsageMode` — "The default value is GENERAL_PURPOSE."
/// (`CreateCertificateAuthorityRequest$UsageMode` docs).
pub const DEFAULT_USAGE_MODE: &str = "GENERAL_PURPOSE";

/// Default `KeyStorageSecurityStandard` — "Default: FIPS_140_2_LEVEL_3_OR_HIGHER"
/// (`CreateCertificateAuthorityRequest$KeyStorageSecurityStandard` docs).
pub const DEFAULT_KEY_STORAGE_SECURITY_STANDARD: &str = "FIPS_140_2_LEVEL_3_OR_HIGHER";

/// `SHORT_LIVED_CERTIFICATE` usage mode: "Short-lived certificate validity is limited to
/// seven days." (`CreateCertificateAuthorityRequest$UsageMode` docs).
pub const SHORT_LIVED_MAX_VALIDITY: Duration = Duration::from_secs(7 * 86_400);

/// How far before the issuance instant the validity period of an issued certificate starts
/// when the request names no `ValidityNotBefore`: one hour.
///
/// Amazon Web Services Private CA backdates that start, so a certificate is usable the
/// moment it is issued by a relying party whose clock lags. A request that names a
/// `ValidityNotBefore` has asked for "a customized starting time for the validity period"
/// (`IssueCertificateRequest$ValidityNotBefore`) and gets that instant verbatim instead.
pub const ISSUANCE_BACKDATED_BY: Duration = Duration::from_secs(3600);

/// How long a `CreateCertificateAuthority` / `IssueCertificate` idempotency token is
/// honoured: "Idempotency tokens for CreateCertificateAuthority time out after five
/// minutes." (`CreateCertificateAuthorityRequest$IdempotencyToken` docs; the
/// `IssueCertificateRequest$IdempotencyToken` docs say the same for IssueCertificate).
pub const IDEMPOTENCY_WINDOW: Duration = Duration::from_secs(5 * 60);

/// How long the CA creation the clock owes takes to settle (`CREATING` ->
/// `PENDING_CERTIFICATE`), and likewise an issued certificate becoming retrievable.
///
/// A span this emulator CHOOSES, not one AWS documents: it only has to be non-zero, so a
/// caller that acts immediately after the starting call still observes the transient
/// status and the state errors that go with it (`RequestInProgressException`).
pub const SETTLE_AFTER: Duration = Duration::from_millis(500);

/// Most private CAs this emulator holds at once.
///
/// A BOUND, in the shape of the per-region quota Amazon Web Services Private CA
/// publishes for private CAs: every CA is held in memory for the life of the process, so
/// without a ceiling an unauthenticated caller could grow the store without limit. A
/// create past it answers the modelled `LimitExceededException`, which names the quota.
pub const MAX_CERTIFICATE_AUTHORITIES: usize = 200;

/// Most certificates one CA holds at once, bounded for the same reason.
pub const MAX_CERTIFICATES_PER_CA: usize = 1_000;

/// Most permissions one CA holds at once.
///
/// A BOUND, in the shape of the quota Amazon Web Services Private CA publishes for the
/// permissions attached to one private CA: a permission is held in memory for the life of
/// the process, so without a ceiling a caller naming a fresh principal per call could grow
/// the store without limit. A grant past it answers the modelled `LimitExceededException`,
/// which names the quota.
pub const MAX_PERMISSIONS_PER_CA: usize = 100;

/// Most live idempotency records one map holds, across every token in it. Expired entries
/// are swept on every record, and the oldest live entry is evicted at the ceiling, so a
/// caller sending a fresh token per request — or one token with a fresh request each time —
/// cannot grow the map without limit.
pub const MAX_IDEMPOTENCY_TOKENS: usize = 128;

/// Most items one page of a listing carries, whatever `MaxResults` asks for: "Although
/// the maximum value is 1000, the action only returns a maximum of 100 items."
/// (`ListCertificateAuthoritiesRequest$MaxResults` docs). It is also what a request that
/// names no `MaxResults` gets.
pub const MAX_LIST_PAGE_SIZE: i32 = 100;

/// How many items the page of a listing asked for by `max_results` holds: what the
/// caller asked for, never more than the ceiling the operation publishes, and the
/// ceiling when the caller asked for nothing.
///
/// The modelled `@range(1..1000)` is refused before any handler runs, so a value that
/// reaches here is at least 1.
pub fn page_size(max_results: Option<i32>) -> usize {
    let requested = max_results.unwrap_or(MAX_LIST_PAGE_SIZE);
    requested.clamp(1, MAX_LIST_PAGE_SIZE) as usize
}

/// The listing `ListCertificateAuthorities` pages over, as its cursors name it.
pub const LISTING_CERTIFICATE_AUTHORITIES: &str = "certificate-authorities";

/// What Amazon Web Services Private CA answers for a `NextToken` it did not issue for
/// the listing being paged: its own two-sentence refusal, whose only variable part is
/// `listing_operation` — the operation that mints the tokens of the listing the caller
/// asked to resume. The rejected token is never echoed, because a token is opaque and a
/// caller cannot act on its text.
pub fn invalid_next_token_message(listing_operation: &str) -> String {
    format!(
        "The NextToken you provided is invalid. Use a token returned by \
         {listing_operation}."
    )
}

/// The separator between a cursor's listing and the key it resumes at: an ASCII unit
/// separator, which no ARN and no listing name contains.
const PAGE_TOKEN_SEPARATOR: char = '\u{1f}';

/// Encode a pagination cursor: the `listing` it belongs to and the key of the first item
/// of the NEXT page.
///
/// Hex-encoded so the token is opaque to the caller and carries an ARN's `:` and `/`
/// safely, and stamped with the listing so a token one listing issued cannot resume
/// another.
pub fn encode_page_token(listing: &str, key: &str) -> String {
    ::hex::encode(format!("{listing}{PAGE_TOKEN_SEPARATOR}{key}"))
}

/// The key a cursor of `listing` resumes at, or `None` when `token` is not one this
/// service issued for that listing — which the caller answers with
/// `InvalidNextTokenException` rather than by restarting the listing.
pub fn decode_page_token(listing: &str, token: &str) -> Option<String> {
    let decoded = String::from_utf8(::hex::decode(token).ok()?).ok()?;
    let (stamped, key) = decoded.split_once(PAGE_TOKEN_SEPARATOR)?;
    (stamped == listing && !key.is_empty()).then(|| key.to_string())
}

/// One issued end-entity certificate, held under the CA that issued it.
#[derive(Debug, Clone)]
pub struct IssuedCertificate {
    /// `<ca arn>/certificate/<serial>`.
    pub arn: String,
    /// Hex serial allocated by the issuing CA.
    pub serial: String,
    /// The certificate body `GetCertificate` returns.
    pub certificate_pem: String,
    /// The CA certificate (plus the CA's own chain) this certificate chains up through.
    pub certificate_chain_pem: Option<String>,
    /// The `SigningAlgorithm` the request asked the CA to sign with.
    pub signing_algorithm: String,
    /// The `TemplateArn` the request named, when it named one.
    pub template_arn: Option<String>,
    /// Start of the certificate's validity window (`ValidityNotBefore`, else issuance).
    pub not_before: SystemTime,
    /// End of the certificate's validity window (`Validity`).
    pub not_after: SystemTime,
    /// When `IssueCertificate` accepted the request.
    pub issued_at: SystemTime,
    /// When issuance is due to complete; `GetCertificate` answers
    /// `RequestInProgressException` before it.
    pub available_at: SystemTime,
    /// The revocation of this certificate, once `RevokeCertificate` has accepted one.
    pub revocation: Option<CertificateRevocation>,
}

impl IssuedCertificate {
    /// Whether issuance has completed as of `now`.
    pub fn is_available(&self, now: SystemTime) -> bool {
        now >= self.available_at
    }

    /// Whether this certificate has been revoked.
    pub fn is_revoked(&self) -> bool {
        self.revocation.is_some()
    }
}

/// The revocation of one issued certificate.
///
/// Revocation is terminal — a revoked certificate is never un-revoked — so this is what a
/// second `RevokeCertificate` for the same certificate is refused from, and what the
/// CA's revocation information (its CRL and its audit report) is read off.
#[derive(Debug, Clone)]
pub struct CertificateRevocation {
    /// When `RevokeCertificate` accepted the revocation.
    pub revoked_at: SystemTime,
    /// The `RevocationReason` the request named.
    pub reason: String,
}

/// One permission granted on a private CA to a service principal.
///
/// Identified, as Amazon Web Services Private CA identifies it, by the CA it was granted
/// on together with the principal that holds it and the account that assigned it: those
/// three are what `CreatePermission` keys a grant by and what `DeletePermission` names to
/// revoke one.
#[derive(Debug, Clone)]
pub struct PermissionRecord {
    /// The service or identity that holds the permission.
    pub principal: String,
    /// "The ID of the account that assigned the permission." (`Permission$SourceAccount`).
    pub source_account: String,
    /// The `ActionType` members the principal may call on the CA, in the order granted.
    pub actions: Vec<String>,
    /// When `CreatePermission` accepted the grant.
    pub created_at: SystemTime,
}

impl PermissionRecord {
    /// What identifies this grant among the ones its CA holds.
    pub fn key(&self) -> (&str, &str) {
        (self.principal.as_str(), self.source_account.as_str())
    }

    /// Whether this grant already allows exactly the actions `actions` asks for.
    ///
    /// A grant is the SET of actions it allows, so the order a request lists them in and a
    /// member it repeats make no difference: what this answers is whether the access the
    /// request asks for is the access that already holds.
    pub fn allows_exactly(&self, actions: &[String]) -> bool {
        action_set(&self.actions) == action_set(actions)
    }

    /// The modelled `Permission` structure, read off this record.
    ///
    /// Both reported members that are not stored verbatim are DERIVED from the grant:
    /// `Actions` in the canonical order a grant's actions are reported in
    /// ([`reported_actions`]) and `Policy` as the access the grant itself expresses
    /// ([`permission_policy_document`]).
    pub fn to_wire(&self, certificate_authority_arn: &str) -> Permission {
        Permission {
            certificate_authority_arn: Some(certificate_authority_arn.to_string()),
            created_at: Some(to_date_time(self.created_at)),
            principal: Some(self.principal.clone()),
            source_account: Some(self.source_account.clone()),
            actions: Some(reported_actions(&self.actions)),
            policy: Some(permission_policy_document(
                certificate_authority_arn,
                &self.principal,
                &self.source_account,
                &self.actions,
            )),
        }
    }
}

/// The distinct `ActionType` members a list of granted actions names, which is what a
/// grant IS: `ActionList` is a list, so a request can name a member more than once and
/// in any order.
fn action_set(actions: &[String]) -> HashSet<&str> {
    actions.iter().map(String::as_str).collect()
}

/// Whether `actions` names EVERY action a private CA's permission can carry, which is what
/// a grant has to name to be recorded: an incomplete grant cannot serve the renewals a
/// permission exists for ([`INCOMPLETE_PERMISSION_ACTIONS_MESSAGE`]).
///
/// The set compared against is the model's own
/// (`ops::types::ACTION_TYPE_VALUES`, generated from `ActionType`), so an action the model
/// adds is part of the requirement without this being edited.
pub fn grants_every_action(actions: &[String]) -> bool {
    let granted = action_set(actions);
    crate::ops::types::ACTION_TYPE_VALUES
        .iter()
        .all(|action| granted.contains(action))
}

/// Every Amazon Web Services service principal a permission on a private CA can be granted
/// to: "At this time, the only valid principal is `acm.amazonaws.com`"
/// (`CreatePermissionRequest$Principal` docs, which `DeletePermissionRequest$Principal`
/// repeats for the revocation of one). The CATALOG, so a principal this service starts
/// supporting is added here rather than wherever a grant is judged.
///
/// A permission exists so that Certificate Manager can renew the private certificates a CA
/// issued, which is why that one service principal is what a grant can name: the model
/// constrains only the SHAPE of the member (`Principal`, len 0..128, pattern `^[^*]+$`), so
/// the supported-principal rule is this service's own.
pub const SUPPORTED_PERMISSION_PRINCIPALS: &[&str] = &["acm.amazonaws.com"];

/// Whether a permission on a private CA can be granted to `principal`, which is whether it is
/// one of the principals this service supports ([`SUPPORTED_PERMISSION_PRINCIPALS`]).
///
/// Compared VERBATIM: a service principal is one exact name, so text that merely contains or
/// extends a supported one — another domain, another case, surrounding space — names a
/// different principal, and that one is not supported.
pub fn grants_to_principal(principal: &str) -> bool {
    SUPPORTED_PERMISSION_PRINCIPALS.contains(&principal)
}

/// What Amazon Web Services Private CA answers a grant naming a principal it does not support
/// ([`grants_to_principal`]).
///
/// Entirely fixed text, naming nothing of the request: a refused principal is never echoed
/// back.
pub const UNSUPPORTED_PRINCIPAL_MESSAGE: &str = "The principal specified is not supported.";

/// The `ActionType` members a grant names, in the order a `Permission` REPORTS them:
/// sorted, which is independent of the order `CreatePermission` received them.
///
/// Not the model's declaration order either (`ActionType` declares `IssueCertificate`,
/// `GetCertificate`, `ListPermissions`, in that order): a grant made with all three in
/// that order is reported as `GetCertificate`, `IssueCertificate`, `ListPermissions`, so
/// what the reported order follows is the members' own ordering, not the request's and not
/// the enum's.
pub fn reported_actions(actions: &[String]) -> Vec<String> {
    let mut reported = actions.to_vec();
    reported.sort();
    reported
}

/// The `Version` an IAM policy document of this service carries: "2012-10-17" is the
/// current IAM policy language version, and the one the policy a grant expresses is
/// written in.
pub const POLICY_LANGUAGE_VERSION: &str = "2012-10-17";

/// The `Sid` of the one statement the policy a grant expresses holds.
///
/// An EXAMPLE, not a requirement: a statement id is a label inside the document, and no
/// rule for it is documented by AWS, stated by the model or derivable from the request.
pub const PERMISSION_STATEMENT_SID: &str = "1";

/// The IAM action name an `ActionType` member denotes: this service's action namespace
/// (`awsService.arnNamespace`, [`ARN_SERVICE`]) and the action.
pub fn iam_action(action: &str) -> String {
    format!("{ARN_SERVICE}:{action}")
}

/// The policy document a grant on a private CA expresses, as `Permission$Policy` reports
/// it: a single-statement IAM policy allowing the granted actions on the CA the grant was
/// made on, to the principal that holds it, conditioned on the account that assigned it.
///
/// Everything in it comes from the grant — the CA's ARN as the statement's `Resource`,
/// the holder as its `Principal`, the granted actions (in the order they are reported,
/// each in this service's action namespace) as its `Action`, and the assigning account as
/// the `aws:SourceAccount` the statement is conditioned on — so the document is a view of
/// the grant rather than a second copy of it, and a grant that changes changes it.
///
/// `Principal` is written as a `Service` principal: `CreatePermission` grants to "the
/// Amazon Web Services service or entity that holds the permission"
/// (`Permission$Principal`).
///
/// The JSON is written member by member in the order an IAM policy document is published
/// in, so the document reads as a policy rather than in a serializer's own key order;
/// every value is JSON-encoded, so any text a principal or an account carries is escaped.
pub fn permission_policy_document(
    certificate_authority_arn: &str,
    principal: &str,
    source_account: &str,
    actions: &[String],
) -> String {
    let json_string =
        |value: &str| ::serde_json::Value::String(value.to_string()).to_string();
    let action = ::serde_json::Value::Array(
        reported_actions(actions)
            .iter()
            .map(|action| ::serde_json::Value::String(iam_action(action)))
            .collect(),
    )
    .to_string();
    format!(
        "{{\"Version\":{version},\"Statement\":[{{\"Sid\":{sid},\"Effect\":\"Allow\",\
         \"Principal\":{{\"Service\":{principal}}},\"Action\":{action},\
         \"Resource\":{resource},\"Condition\":{{\"StringEquals\":\
         {{\"aws:SourceAccount\":{account}}}}}}}]}}",
        version = json_string(POLICY_LANGUAGE_VERSION),
        sid = json_string(PERMISSION_STATEMENT_SID),
        principal = json_string(principal),
        resource = json_string(certificate_authority_arn),
        account = json_string(source_account),
    )
}

/// Why the submitted resource policy is not one this service attaches to the CA
/// `certificate_authority_arn` names, or `None` when it is: "If this policy does not
/// contain all required statements or if it includes any statement that is not allowed,
/// the PutPolicy action returns an InvalidPolicyException" (`PutPolicyRequest$Policy`
/// docs).
///
/// What is checked is what that sentence states and what a resource-based policy of a
/// private CA is:
///
/// * the document has to BE a policy — the grammar [`is_well_formed_policy`] reads, whose
///   refusal is the document itself ([`malformed_policy_message`]), because there is no
///   statement left to blame;
/// * each statement has to GRANT, so its `Effect` is `Allow` and nothing else
///   ([`NON_ALLOW_EFFECT_MESSAGE`]);
/// * each statement has to name WHO it is about, in a `Principal` that names an identity
///   ([`unresolvable_principal_reason`]);
/// * each statement has to name WHAT it grants, in actions a certificate authority can be
///   shared for ([`disallowed_action_reason`]);
/// * each statement has to be about the CA the policy is attached TO, so every `Resource`
///   it names is that CA's ARN ([`different_resource_message`]).
///
/// Nothing here rewrites the document.
pub fn invalid_policy_reason(policy: &str, certificate_authority_arn: &str) -> Option<String> {
    if !is_well_formed_policy(policy) {
        return Some(malformed_policy_message(policy));
    }
    // `is_well_formed_policy` read the document as JSON, so it parses, and it is an object
    // carrying statements.
    let document = ::serde_json::from_str::<::serde_json::Value>(policy).ok()?;
    let statement = document.get("Statement")?;
    // "Statement" holds one statement or an array of them, which is how IAM reads it.
    let statements: Vec<&::serde_json::Value> = match statement {
        ::serde_json::Value::Array(statements) => statements.iter().collect(),
        other => vec![other],
    };
    for statement in statements {
        let statement = statement.as_object()?;
        if let Some(reason) = unsupported_effect_reason(statement.get("Effect")) {
            return Some(reason);
        }
        if let Some(reason) = unresolvable_principal_reason(statement.get("Principal")) {
            return Some(reason);
        }
        if let Some(reason) = disallowed_action_reason(statement.get("Action")) {
            return Some(reason);
        }
        if let Some(reason) =
            different_resource_reason(statement.get("Resource"), certificate_authority_arn)
        {
            return Some(reason);
        }
    }
    None
}

/// The `Effect` a statement of a private CA's resource-based policy carries: a resource
/// policy of a private CA GRANTS access to it — "A policy grants access on a private CA to
/// an Amazon Web Services customer account, to Amazon Web Services Organizations, or to an
/// Amazon Web Services Organizations unit" (`PutPolicy` docs) — so the one effect it
/// expresses is the allow.
pub const POLICY_GRANT_EFFECT: &str = "Allow";

/// What Amazon Web Services Private CA answers a resource policy whose statement does not
/// grant, i.e. whose `Effect` is anything other than [`POLICY_GRANT_EFFECT`].
///
/// Wholly fixed text: the refusal is about the effect a private CA's resource policy can
/// express, so neither the offending statement nor the document is echoed in it.
pub const NON_ALLOW_EFFECT_MESSAGE: &str = "InvalidPolicy: Only the Allow effect is supported";

/// What this service answers a policy statement that names no `Resource` at all: a sentence
/// of its own under the one prefix every refusal of a policy document carries
/// ([`INVALID_POLICY_PREFIX`]), so it is no other refusal's string but this rule's.
///
/// No recording covers a statement with no `Resource`, so the RULE is what is pinned — a
/// resource-based policy's statement says which resource it is about — and the sentence is
/// this service's own phrasing of it.
pub const NO_POLICY_RESOURCE_MESSAGE: &str =
    "InvalidPolicy: Each policy statement must name the Resource it is about.";

/// What this service answers a policy statement that grants no action at all, for the reason
/// [`NO_POLICY_RESOURCE_MESSAGE`] carries its own sentence.
pub const NO_POLICY_ACTION_MESSAGE: &str =
    "InvalidPolicy: Each policy statement must name the Action it grants.";

/// Why the `Effect` of a policy statement is not one a private CA's resource policy can
/// carry, or `None` when it grants.
///
/// A resource policy on a private CA exists to GRANT the CA to another identity, so the
/// only statement it can hold is an allow ([`POLICY_GRANT_EFFECT`]); a statement carrying
/// any other effect — the deny IAM's own grammar admits included — is "a statement that is
/// not allowed" (`PutPolicyRequest$Policy` docs) and the whole document is refused
/// ([`NON_ALLOW_EFFECT_MESSAGE`]) with nothing attached.
fn unsupported_effect_reason(effect: Option<&::serde_json::Value>) -> Option<String> {
    if effect.and_then(::serde_json::Value::as_str) == Some(POLICY_GRANT_EFFECT) {
        None
    } else {
        Some(NON_ALLOW_EFFECT_MESSAGE.to_string())
    }
}

/// The JSON type of `value`, as the refusal of a rejected policy element names it: the
/// token of the JSON type system itself, so a member added to a document is reported by
/// what it IS rather than by where it sat.
fn json_type_token(value: &::serde_json::Value) -> &'static str {
    match value {
        ::serde_json::Value::Null => "NULL",
        ::serde_json::Value::Bool(_) => "BOOLEAN",
        ::serde_json::Value::Number(_) => "NUMBER",
        ::serde_json::Value::String(_) => "STRING",
        ::serde_json::Value::Array(_) => "ARRAY",
        ::serde_json::Value::Object(_) => "OBJECT",
    }
}

/// What Amazon Web Services Private CA answers when a statement of the submitted policy
/// is about a resource other than the private CA the policy is being attached to.
///
/// Built from the REQUEST, and it identifies the offending element of the submitted
/// document rather than only the CA: the CA the policy is being attached to (and so the
/// one resource its statements can be about), then the JSON type of the `rejected` value
/// ([`json_type_token`]), then that value written as JSON on a line of its own.
///
/// Fixed text: the prefix, " and type: ", the label "string: " and the two newlines; from
/// the request: the CA's ARN, the type token and the rejected value. The label is the
/// recorded one and does not follow the type token — what the type token is for is saying
/// what the rejected value WAS.
pub fn different_resource_message(
    certificate_authority_arn: &str,
    rejected: &::serde_json::Value,
) -> String {
    format!(
        "InvalidPolicy: The supplied policy contains different resource or incorrect ARN \
         format: {certificate_authority_arn} and type: {token}\nstring: {rejected}\n",
        token = json_type_token(rejected),
    )
}

/// The policy document as Amazon Web Services Private CA holds and displays one: the
/// policy the submitted document states, rendered the way a policy store reports a
/// document ([`::service_common::iam_policy_rendering::rendered`]).
///
/// Two things differ from the submitted text, and nothing else does: the whitespace
/// between the document's JSON tokens is gone, and an element the policy language types as
/// a string or an array of strings (a statement's `Action`/`Resource` and the principal
/// lists) that carries a one-element array of one string is reported as that string. What
/// says the second holds here is the recorded read: a statement whose `Action` arrived as a
/// one-element list is displayed with a scalar `Action`, while a `Statement` holding one
/// statement keeps its one-element array.
///
/// Everything else is the caller's own: the member order, the statement count, the type and
/// spelling of every other value, and every byte inside a string literal, escapes and
/// non-ASCII characters included. The shared walk is one pass over the text with no
/// recursion, and the size it may be handed is `AWSPolicy`'s modelled length.
pub fn reported_policy_document(policy: &str) -> String {
    ::service_common::iam_policy_rendering::rendered(policy)
}

/// Why the `Resource` of a policy statement is not the private CA the policy is attached
/// to, or `None` when every resource it names is.
///
/// A resource-based policy is about the resource it hangs on: `PutPolicy` "attaches a
/// resource-based policy to a private CA" (its docs), so a statement naming another
/// resource — or something that is not that CA's ARN at all, a wildcard included — is not
/// a statement this CA's policy can carry.
fn different_resource_reason(
    resource: Option<&::serde_json::Value>,
    certificate_authority_arn: &str,
) -> Option<String> {
    let Some(resource) = resource else {
        return Some(NO_POLICY_RESOURCE_MESSAGE.to_string());
    };
    // Every entry is judged on its own, so the refusal names the one value that was
    // rejected rather than the element it sat in.
    let entries: Vec<&::serde_json::Value> = match resource {
        ::serde_json::Value::Array(values) => values.iter().collect(),
        other => vec![other],
    };
    if entries.is_empty() {
        return Some(NO_POLICY_RESOURCE_MESSAGE.to_string());
    }
    for entry in entries {
        if entry.as_str() != Some(certificate_authority_arn) {
            return Some(different_resource_message(certificate_authority_arn, entry));
        }
    }
    None
}

/// The actions a resource-based policy on a private CA can grant: the read-and-issue
/// surface a private CA is SHARED for, each in this service's action namespace
/// ([`ARN_SERVICE`]).
///
/// A resource policy on a private CA exists to let another account "issue ACM certificates
/// signed by a CA in another account" (`PutPolicy` docs) — the same thing sharing the CA
/// through Resource Access Manager does — so what it can grant is what a share grants: the
/// three actions a permission on a CA carries (`ActionType`, [`ACTIONS_IN_MODEL_ORDER`])
/// together with the reads of the CA a cross-account issuer has to make to use it (the CA
/// itself, its certificate, its certification request and its tags).
///
/// Everything else this service models is a CA administrator's own: creating, updating,
/// deleting or restoring the authority, writing its tags, permissions or policy, issuing
/// an audit report, revoking a certificate. Granting one of those on a certificate
/// authority is "a statement that is not allowed" (`PutPolicyRequest$Policy` docs), which
/// the recorded refusal names by resource type
/// ([`DISALLOWED_POLICY_ACTION_MESSAGE`]).
pub const POLICY_ALLOWED_ACTIONS: &[&str] = &[
    "DescribeCertificateAuthority",
    "GetCertificate",
    "GetCertificateAuthorityCertificate",
    "GetCertificateAuthorityCsr",
    "IssueCertificate",
    "ListPermissions",
    "ListTags",
];

/// What Amazon Web Services Private CA answers a resource policy that grants an action a
/// certificate authority's policy cannot grant ([`POLICY_ALLOWED_ACTIONS`]).
///
/// Wholly fixed text, the resource type it names included: the refusal is about the kind of
/// resource the policy hangs on, so it carries neither the offending action nor the CA out
/// of the request.
pub const DISALLOWED_POLICY_ACTION_MESSAGE: &str =
    "InvalidPolicy: Input policy containing action that is not allowed for resource type \
     acm-pca:CertificateAuthority";

/// Whether `action` is one a private CA's resource policy can grant: an action of THIS
/// service ([`ARN_SERVICE`]) that is on the shared surface
/// ([`POLICY_ALLOWED_ACTIONS`]). An action of another service, one with no namespace and a
/// wildcard are none of them.
fn policy_grants_this_action(action: &str) -> bool {
    action
        .strip_prefix(&format!("{ARN_SERVICE}:"))
        .is_some_and(|named| POLICY_ALLOWED_ACTIONS.contains(&named))
}

/// Why the `Action` of a policy statement is not one a private CA's resource policy can
/// grant, or `None` when every action it names is.
///
/// A statement that grants no action is not a grant at all, and a policy attached to a
/// private CA grants access ON that CA — `PutPolicy` "attaches a resource-based policy to
/// a private CA" (its docs) — so every action it names has to be one a certificate
/// authority can be shared for ([`POLICY_ALLOWED_ACTIONS`]); anything else is "a statement
/// that is not allowed" (`PutPolicyRequest$Policy` docs).
fn disallowed_action_reason(action: Option<&::serde_json::Value>) -> Option<String> {
    let not_allowed = || Some(DISALLOWED_POLICY_ACTION_MESSAGE.to_string());
    let Some(action) = action else {
        return Some(NO_POLICY_ACTION_MESSAGE.to_string());
    };
    let entries: Vec<&::serde_json::Value> = match action {
        ::serde_json::Value::Array(values) => values.iter().collect(),
        other => vec![other],
    };
    if entries.is_empty() {
        return Some(NO_POLICY_ACTION_MESSAGE.to_string());
    }
    for entry in entries {
        let Some(named) = entry.as_str() else {
            return not_allowed();
        };
        if !policy_grants_this_action(named) {
            return not_allowed();
        }
    }
    None
}

/// What Amazon Web Services Private CA answers when a statement of the submitted policy
/// is about an identity it cannot resolve.
///
/// The RULE is that the service resolves the identity a resource policy is about rather
/// than taking the text on trust, and the sentence is the one the recorded refusal carries
/// for a policy naming an identity that is not there: wholly fixed text, which is why the
/// account id or ARN that failed to resolve is not echoed in it.
pub const POLICY_PRINCIPAL_NOT_FOUND_MESSAGE: &str = "InvalidPolicy: Principal not found.";

/// Why the `Principal` of a policy statement names no identity this service can resolve,
/// or `None` when it does.
///
/// A resource policy says who may do what, so a statement with no `Principal` is about
/// nobody, and one naming an identity that is not there is about nobody either: Amazon
/// Web Services Private CA resolves it and refuses the whole policy when it cannot
/// ([`POLICY_PRINCIPAL_NOT_FOUND_MESSAGE`]).
///
/// What resolves: `*` (every identity), an Amazon Web Services service principal, an
/// account this emulator holds ([`account_resolves`]), and an ARN that names an identity of
/// such an account (an IAM or Organizations ARN). Text that is none of those names nobody,
/// which is what the refusal says.
fn unresolvable_principal_reason(principal: Option<&::serde_json::Value>) -> Option<String> {
    let not_found = || Some(POLICY_PRINCIPAL_NOT_FOUND_MESSAGE.to_string());
    let Some(principal) = principal else {
        return not_found();
    };
    match principal {
        // `"Principal": "*"` is every identity, and `"Principal": "<identity>"` is the
        // shorthand for an Amazon Web Services identity.
        ::serde_json::Value::String(identity) => {
            if aws_identity_resolves(identity) {
                None
            } else {
                not_found()
            }
        }
        ::serde_json::Value::Object(members) => {
            if members.is_empty() {
                return not_found();
            }
            for (kind, value) in members {
                let entries: Vec<&::serde_json::Value> = match value {
                    ::serde_json::Value::Array(values) => values.iter().collect(),
                    other => vec![other],
                };
                if entries.is_empty() {
                    return not_found();
                }
                for entry in entries {
                    let Some(entry) = entry.as_str() else {
                        return not_found();
                    };
                    let resolves = match kind.as_str() {
                        "AWS" => aws_identity_resolves(entry),
                        "Service" => is_service_principal(entry),
                        // `Federated` and `CanonicalUser` name identities outside this
                        // emulator, which therefore resolve to nothing here.
                        _ => false,
                    };
                    if !resolves {
                        return not_found();
                    }
                }
            }
            None
        }
        _ => not_found(),
    }
}

/// Whether an `AWS` principal entry names an identity: every identity (`*`), an account
/// this emulator holds ([`account_resolves`], an account id in `AccountId`'s own twelve-digit
/// form) or an ARN that names an identity of such an account.
fn aws_identity_resolves(identity: &str) -> bool {
    if identity == "*" {
        return true;
    }
    if identity.len() == 12 && identity.chars().all(|digit| digit.is_ascii_digit()) {
        return account_resolves(identity);
    }
    names_an_identity_arn(identity)
}

/// Whether `account` is an Amazon Web Services account this emulator can resolve.
///
/// A resource policy grants access to an account, so the account has to BE there: Amazon
/// Web Services Private CA resolves it and refuses a document naming one that is not
/// ([`POLICY_PRINCIPAL_NOT_FOUND_MESSAGE`]). This emulator presents exactly one account —
/// the one every ARN and every `OwnerAccount` it emits carries
/// (`emulator_server::emulator_account_id`) — so that is the one account a principal can
/// resolve to, and any other twelve-digit id names an account that is not here.
fn account_resolves(account: &str) -> bool {
    account == ::emulator_server::emulator_account_id()
}

/// Whether `identity` is an ARN naming an identity: `arn:<partition>:iam::<account>:<id>`
/// (an account's root, a user or a role) or an Amazon Web Services Organizations ARN,
/// which are the identities a resource policy of a private CA grants to — "A policy grants
/// access on a private CA to an Amazon Web Services customer account, to Amazon Web
/// Services Organizations, or to an Amazon Web Services Organizations unit" (`PutPolicy`
/// docs).
fn names_an_identity_arn(identity: &str) -> bool {
    let fields: Vec<&str> = identity.split(':').collect();
    let [prefix, partition, service, region, account, resource] = fields.as_slice() else {
        return false;
    };
    if *prefix != "arn" || partition.is_empty() || resource.is_empty() {
        return false;
    }
    // An identity namespace is global, so its region is empty.
    if !region.is_empty() {
        return false;
    }
    match *service {
        // An IAM or STS ARN names an entity OF an account — a root, a user, a role, an
        // assumed-role session — so the account that holds it has to be one this emulator
        // resolves, exactly as a bare account id does.
        "iam" | "sts" => account_resolves(account),
        // An Organizations ARN names an organization or one of its units rather than an
        // account, and nothing this service holds resolves one, so what is read of it is
        // the ARN's shape.
        "organizations" => !account.is_empty(),
        _ => false,
    }
}

/// Whether `identity` names an Amazon Web Services service principal, the form
/// `CreatePermission` grants to as well ("the only valid principal is
/// `acm.amazonaws.com`", `CreatePermissionRequest$Principal` docs).
fn is_service_principal(identity: &str) -> bool {
    identity
        .strip_suffix(".amazonaws.com")
        .is_some_and(|service| !service.is_empty())
}


/// One private certificate authority.
#[derive(Debug, Clone)]
pub struct CertificateAuthorityRecord {
    /// `arn:aws:acm-pca:<region>:<account>:certificate-authority/<id>`.
    pub arn: String,
    /// Account that owns the CA.
    pub owner_account: String,
    /// `CertificateAuthorityType`: ROOT or SUBORDINATE.
    pub ca_type: String,
    /// `CertificateAuthorityStatus`.
    pub status: String,
    /// When `CreateCertificateAuthority` accepted the request.
    pub created_at: SystemTime,
    /// When the CA last changed status.
    pub last_state_change_at: SystemTime,
    /// When `CREATING` is due to settle into `PENDING_CERTIFICATE`.
    pub settles_at: SystemTime,
    /// Start of the CA certificate's validity window; set on import.
    pub not_before: Option<SystemTime>,
    /// End of the CA certificate's validity window; set on import.
    pub not_after: Option<SystemTime>,
    /// Why creating the CA failed, when it did.
    pub failure_reason: Option<String>,
    /// The configuration the create request supplied.
    pub configuration: CertificateAuthorityConfiguration,
    /// The revocation configuration the create request supplied.
    pub revocation_configuration: Option<RevocationConfiguration>,
    /// `CertificateAuthorityUsageMode`.
    pub usage_mode: String,
    /// `KeyStorageSecurityStandard`.
    pub key_storage_security_standard: String,
    /// The CSR this CA hands out, derived once at create time and never re-derived.
    pub csr_pem: String,
    /// The key pair this CA signs with, minted once at create time, for the
    /// `KeyAlgorithm` its configuration names ([`generate_key_pair`]); `None` for a
    /// `KeyAlgorithm` the in-process cryptography provider cannot mint.
    pub key_pair: Option<CaKeyPair>,
    /// The CA certificate imported through `ImportCertificateAuthorityCertificate`.
    pub ca_certificate_pem: Option<String>,
    /// The chain imported alongside that certificate.
    pub ca_certificate_chain_pem: Option<String>,
    /// How long a deleted CA can still be restored.
    pub restorable_until: Option<SystemTime>,
    /// The status the CA held when it was deleted: "The status of a restored CA is set to
    /// its pre-deletion status when the RestoreCertificateAuthority action returns."
    /// (`RestoreCertificateAuthority` docs), so the deletion is what has to record it.
    pub status_before_deletion: Option<String>,
    /// Tags attached to the CA.
    pub tags: Vec<Tag>,
    /// The resource-based policy attached to the CA, when one is (`PutPolicy` attaches
    /// it, `GetPolicy` reads it, `DeletePolicy` removes it). It lives ON the CA, so it
    /// goes away with the CA rather than outliving it in a map of its own.
    pub resource_policy: Option<String>,
    /// Counter the next issued serial is minted from.
    pub next_serial: u64,
    /// "X509v3 CRL Number: Decimal sequence number for the CRL" (`CrlConfiguration`
    /// docs): the number the CRL this CA published last carries, so the next one it
    /// publishes carries the number after it.
    pub crl_number: u64,
    /// When this CA last published its CRL, or `None` while it has published none.
    ///
    /// What tells a revoked certificate that has expired apart from one that expired
    /// before the previous CRL went out, which is the difference between the one further
    /// CRL it appears in and the ones it no longer does ([`listed_in_crl`]).
    pub crl_published_at: Option<SystemTime>,
    /// Certificates issued by this CA, keyed by certificate ARN.
    pub certificates: HashMap<String, IssuedCertificate>,
    /// The audit reports this CA has produced over those certificates, oldest first
    /// ([`AuditReportRecord`]).
    pub audit_reports: Vec<AuditReportRecord>,
    /// Permissions granted on this CA, one per (principal, source account) pair.
    pub permissions: Vec<PermissionRecord>,
    /// `IssueCertificate` requests this CA has answered -> the certificate ARN each
    /// minted.
    ///
    /// Keyed by what identifies ONE issuance request ([`IssuedCertificateRequest`]), so a
    /// repeat of a request this CA has already answered is answered with the certificate it
    /// minted then rather than with a second certificate for one logical request: the
    /// operation is `@idempotent`, and "if you call IssueCertificate multiple times with
    /// the same idempotency token … Amazon Web Services Private CA recognizes that you are
    /// requesting only one certificate and will issue only one"
    /// (`IssueCertificateRequest$IdempotencyToken`).
    pub issue_idempotency: IdempotencyRecords<IssuedCertificateRequest>,
}

impl CertificateAuthorityRecord {
    /// Materialise the status change the clock already owed at `now`.
    ///
    /// A reporting operation may call this: the transition was due at `settles_at`
    /// whether or not anybody called, so applying it changes nothing a later caller
    /// could have seen differently.
    pub fn settle(&mut self, now: SystemTime) {
        if self.status == STATUS_CREATING && now >= self.settles_at {
            self.status = STATUS_PENDING_CERTIFICATE.to_string();
            self.last_state_change_at = self.settles_at;
        }
    }

    /// Whether this CA's status allows `DeleteCertificateAuthority`
    /// ([`DELETABLE_STATUSES`]).
    pub fn is_deletable(&self) -> bool {
        DELETABLE_STATUSES.contains(&self.status.as_str())
    }

    /// Whether this CA has been deleted and not restored, so a write to what it holds is
    /// refused with the modelled `InvalidStateException`.
    pub fn is_deleted(&self) -> bool {
        self.status == STATUS_DELETED
    }

    /// Whether this CA's status allows `UpdateCertificateAuthority`
    /// ([`UPDATABLE_STATUSES`]).
    pub fn is_updatable(&self) -> bool {
        UPDATABLE_STATUSES.contains(&self.status.as_str())
    }

    /// The restoration window a deletion of this CA opens when the request asked for
    /// `requested_days`: that many days, 30 when the request named none, and none at all
    /// from a status a deletion assigns no restoration period to
    /// ([`UNRESTORABLE_DELETION_STATUSES`]).
    pub fn restoration_window(&self, requested_days: Option<i32>) -> Option<Duration> {
        if UNRESTORABLE_DELETION_STATUSES.contains(&self.status.as_str()) {
            return None;
        }
        // The member is modelled `range 7..30`, so a value outside it never reaches a
        // handler; the assertion says so, and the clamp keeps the window inside the
        // published bounds if that ever stops holding.
        debug_assert!(
            requested_days.is_none_or(|days| {
                (MIN_PERMANENT_DELETION_TIME_IN_DAYS..=MAX_PERMANENT_DELETION_TIME_IN_DAYS)
                    .contains(&days)
            }),
            "PermanentDeletionTimeInDays is modelled range 7..30: {requested_days:?}"
        );
        let days = requested_days
            .unwrap_or(DEFAULT_PERMANENT_DELETION_TIME_IN_DAYS)
            .clamp(
                MIN_PERMANENT_DELETION_TIME_IN_DAYS,
                MAX_PERMANENT_DELETION_TIME_IN_DAYS,
            );
        Some(ONE_DAY * days.unsigned_abs())
    }

    /// Move this CA into `DELETED`, opening the restoration window `window`.
    ///
    /// The status it held is remembered, because that is the status a restore inside the
    /// window puts it back into.
    pub fn delete(&mut self, now: SystemTime, window: Option<Duration>) {
        self.status_before_deletion =
            Some(::std::mem::replace(&mut self.status, STATUS_DELETED.to_string()));
        self.last_state_change_at = now;
        self.restorable_until = window.and_then(|window| now.checked_add(window));
    }

    /// Whether the restoration window of a deleted CA has closed as of `now`, so the CA
    /// is permanently deleted: "the CA won't be permanently deleted until the restoration
    /// period has passed" (`DeleteCertificateAuthority` docs), and "You cannot restore a
    /// CA after the restoration period has ended" (`RestoreCertificateAuthority` docs).
    ///
    /// A CA deleted from a status that assigns no restoration period is never restorable
    /// and never permanently deleted either: it keeps answering as `DELETED`.
    pub fn is_permanently_deleted(&self, now: SystemTime) -> bool {
        self.status == STATUS_DELETED && self.restorable_until.is_some_and(|until| now >= until)
    }

    /// The permission this CA holds for `principal` and `source_account`, when it holds
    /// one: the grant those two name is unique among the CA's permissions.
    pub fn permission(
        &self,
        principal: &str,
        source_account: &str,
    ) -> Option<&PermissionRecord> {
        self.permissions
            .iter()
            .find(|permission| permission.key() == (principal, source_account))
    }

    /// Every permission this CA holds, ordered by the pair that identifies a grant.
    ///
    /// A total order over that pair is what lets one `ListPermissions` page resume where
    /// the previous one stopped: the position a `NextToken` names is the same on every
    /// call, whatever order the grants were made in and whether or not the grant the
    /// token names is still there.
    pub fn permissions_in_order(&self) -> Vec<&PermissionRecord> {
        let mut ordered: Vec<&PermissionRecord> = self.permissions.iter().collect();
        ordered.sort_by_key(|permission| permission.key());
        ordered
    }

    /// The modelled `CertificateAuthority` structure, read straight off this record.
    pub fn to_wire(&self) -> CertificateAuthority {
        CertificateAuthority {
            arn: Some(self.arn.clone()),
            owner_account: Some(self.owner_account.clone()),
            created_at: Some(to_date_time(self.created_at)),
            last_state_change_at: Some(to_date_time(self.last_state_change_at)),
            r#type: Some(self.ca_type.clone()),
            // "Serial number of your private CA" (`CertificateAuthority$Serial`): the
            // number is the one the certificate this CA was activated with carries, read
            // off that certificate ([`own_certificate_serial`](Self::own_certificate_serial))
            // and stated as the integer it is ([`serial_integer`]). A CA that holds no
            // certificate has no serial number to report.
            serial: self
                .own_certificate_serial()
                .as_deref()
                .and_then(serial_integer),
            status: Some(self.status.clone()),
            not_before: self.not_before.map(to_date_time),
            not_after: self.not_after.map(to_date_time),
            failure_reason: self.failure_reason.clone(),
            certificate_authority_configuration: Some(self.configuration.clone()),
            revocation_configuration: Some(reported_revocation_configuration(
                self.revocation_configuration.as_ref(),
            )),
            restorable_until: self.restorable_until.map(to_date_time),
            key_storage_security_standard: Some(self.key_storage_security_standard.clone()),
            usage_mode: Some(self.usage_mode.clone()),
        }
    }

    /// The chain an end-entity certificate issued by this CA presents: the CA's own
    /// certificate followed by whatever chain was imported with it.
    pub fn issued_chain(&self) -> Option<String> {
        let ca_certificate = self.ca_certificate_pem.as_ref()?;
        Some(match self.ca_certificate_chain_pem.as_ref() {
            Some(chain) => format!("{ca_certificate}{chain}"),
            None => ca_certificate.clone(),
        })
    }

    /// Whether this CA is being asked for the one certificate it can sign before it holds
    /// a certificate of its own: its SELF-SIGNED ROOT certificate.
    ///
    /// A root CA's own certificate is signed by the CA itself, which is why the documented
    /// procedure for creating one has `IssueCertificate` called on the CA that is still
    /// waiting for a certificate — with that CA's OWN certification request, under the
    /// root CA certificate template — and the certificate that comes back is what
    /// `ImportCertificateAuthorityCertificate` then activates it with. So all four have to
    /// hold: the CA waits for its certificate, it is a root, the request names the root CA
    /// certificate template, and the certification request presented is the CA's own.
    /// Anything else asked of a CA outside `ACTIVE` is the state refusal
    /// ([`ISSUANCE_INVALID_STATE_MESSAGE`]).
    pub fn signs_its_own_root_certificate(
        &self,
        template_arn: Option<&str>,
        certification_request: &[u8],
    ) -> bool {
        self.status == STATUS_PENDING_CERTIFICATE
            && self.ca_type == CA_TYPE_ROOT
            && template_arn.is_some_and(|arn| arn.contains(ROOT_CA_CERTIFICATE_TEMPLATE))
            && self.is_own_certification_request(certification_request)
    }

    /// The certificate this CA issued under the serial number `serial` names, when it
    /// holds one.
    ///
    /// A certificate is identified inside its CA by the VALUE of its serial, which a
    /// request carries "in hexadecimal format"
    /// (`RevokeCertificateRequest$CertificateSerial`), so the two spellings of one value
    /// name the same certificate ([`normalized_serial`]).
    pub fn certificate_with_serial(&self, serial: &str) -> Option<&IssuedCertificate> {
        let wanted = normalized_serial(serial)?;
        self.certificates
            .values()
            .find(|certificate| certificate_has_serial(certificate, &wanted))
    }

    /// The same certificate [`certificate_with_serial`](Self::certificate_with_serial)
    /// resolves, to write to.
    pub fn certificate_with_serial_mut(
        &mut self,
        serial: &str,
    ) -> Option<&mut IssuedCertificate> {
        let wanted = normalized_serial(serial)?;
        self.certificates
            .values_mut()
            .find(|certificate| certificate_has_serial(certificate, &wanted))
    }

    /// Whether `serial` names this CA's OWN self-signed certificate, which is the one
    /// certificate of a CA that cannot be revoked: "You cannot revoke a root CA
    /// self-signed certificate." (`RevokeCertificate` docs).
    ///
    /// A root CA's certificate is signed by the CA itself, so revoking it would have the
    /// CA attest its own revocation; a subordinate CA's certificate is signed by its
    /// parent, which revokes it, so only a root holds such a certificate at all. The
    /// serial is read off the certificate the CA was activated with, so it holds whether
    /// or not that certificate was issued through this emulator.
    pub fn is_own_self_signed_serial(&self, serial: &str) -> bool {
        if self.ca_type != CA_TYPE_ROOT {
            return false;
        }
        let Some(wanted) = normalized_serial(serial) else {
            return false;
        };
        self.own_certificate_serial()
            .is_some_and(|own| own == wanted)
    }

    /// The serial number of the certificate this CA was activated with, as the value it
    /// denotes, when the CA holds one this service can read.
    pub fn own_certificate_serial(&self) -> Option<String> {
        let der = parse_pem_block(self.ca_certificate_pem.as_deref()?, PEM_LABEL_CERTIFICATE)?;
        let certificate = Certificate::from_der(&der).ok()?;
        normalized_serial(&::hex::encode(certificate.tbs_certificate.serial_number.as_bytes()))
    }

    /// When the certificate this CA was activated with stops being valid, when the CA holds
    /// one this service can read.
    ///
    /// Read out of that certificate's own validity window
    /// ([`certificate_validity_window`]) rather than off the record, so it is the instant
    /// the certificate itself names — which is what bounds every certificate this CA
    /// issues: "The end of the validity period configured on a certificate must not exceed
    /// the limit set on its parents in the CA hierarchy."
    /// (`IssueCertificateRequest$Validity` docs.)
    pub fn own_certificate_not_after(&self) -> Option<SystemTime> {
        let der = parse_pem_block(self.ca_certificate_pem.as_deref()?, PEM_LABEL_CERTIFICATE)?;
        let certificate = Certificate::from_der(&der).ok()?;
        Some(certificate_validity_window(&certificate).1)
    }

    /// Whether `presented` carries the very certification request this CA published
    /// ([`derive_csr_pem`]), compared as the DER it frames so the PEM's own line breaks
    /// and framing do not decide it.
    fn is_own_certification_request(&self, presented: &[u8]) -> bool {
        let own = parse_pem_block(&self.csr_pem, PEM_LABEL_CSR);
        let presented = ::std::str::from_utf8(presented)
            .ok()
            .and_then(|text| parse_pem_block(text, PEM_LABEL_CSR));
        match (own, presented) {
            (Some(own), Some(presented)) => own == presented,
            _ => false,
        }
    }
}

/// The `RevocationConfiguration` a reported `CertificateAuthority` carries.
///
/// Both sub-configurations are always present: `CrlConfiguration$Enabled` and
/// `OcspConfiguration$Enabled` are `@required` booleans, and "By default, both
/// certificate validation mechanisms are disabled."
/// (`CreateCertificateAuthorityRequest$RevocationConfiguration` docs), so a mechanism the
/// CA was not configured with is reported as the disabled configuration rather than left
/// out. A mechanism the CA WAS configured with is reported exactly as it was stored.
pub fn reported_revocation_configuration(
    configured: Option<&RevocationConfiguration>,
) -> RevocationConfiguration {
    RevocationConfiguration {
        crl_configuration: Some(
            configured
                .and_then(|revocation| revocation.crl_configuration.clone())
                .unwrap_or_default(),
        ),
        ocsp_configuration: Some(
            configured
                .and_then(|revocation| revocation.ocsp_configuration.clone())
                .unwrap_or_default(),
        ),
    }
}

/// Why `revocation` is not a revocation configuration this service will store, classified
/// by the stage that refuses it, or `None` when it is one.
///
/// The rules the `RevocationConfiguration` member documents, identically on
/// `CreateCertificateAuthority` and `UpdateCertificateAuthority`: "A configuration
/// disabling CRLs or OCSP must contain only the `Enabled=False` parameter, and will fail
/// if other parameters such as `CustomCname` or `ExpirationInDays` are included." An
/// enabled CRL configuration conversely needs the bucket it is written to and the period
/// it expires after, and the two members that decide where its distribution point extension
/// points cannot both be set: "This configuration cannot be enabled with a custom CNAME set."
/// (`CrlDistributionPointExtensionConfiguration$OmitExtension`.)
pub fn revocation_configuration_fault(
    revocation: Option<&RevocationConfiguration>,
) -> Option<RevocationConfigurationFault> {
    let revocation = revocation?;
    if let Some(crl) = revocation.crl_configuration.as_ref() {
        if crl.enabled {
            // The bucket the CRLs are published to is a member the configuration's own
            // state requires, so its absence is the request-validation stage's to refuse,
            // naming the member ([`RevocationConfigurationFault::MissingMember`]).
            if crl
                .s3_bucket_name
                .as_deref()
                .is_none_or(|bucket| bucket.trim().is_empty())
            {
                return Some(RevocationConfigurationFault::MissingMember(
                    CRL_BUCKET_MEMBER,
                ));
            }
            if crl.expiration_in_days.is_none() {
                return Some(RevocationConfigurationFault::Unserviceable(
                    "RevocationConfiguration.CrlConfiguration.ExpirationInDays is required \
                     when CRLs are enabled"
                        .to_string(),
                ));
            }
            // "Designates a custom ﬁle path in S3 for CRL(s)"
            // (`CrlConfiguration$CustomPath`): a path is where under the bucket the CRLs
            // are written, so one whose segments name the folder itself or the one above it
            // designates no such place and is not a path this service will publish to.
            if crl
                .custom_path
                .as_deref()
                .is_some_and(path_leaves_its_folder)
            {
                return Some(RevocationConfigurationFault::Unserviceable(
                    "RevocationConfiguration.CrlConfiguration.CustomPath must designate a \
                     path under the CRL bucket, so it cannot contain a '.' or '..' segment"
                        .to_string(),
                ));
            }
            // "This configuration cannot be enabled with a custom CNAME set."
            // (`CrlDistributionPointExtensionConfiguration$OmitExtension`.) A custom CNAME is
            // the alias the distribution point extension names
            // (`CrlConfiguration$CustomCname`), so a configuration that sets one and leaves
            // that extension out asks for an alias of nothing.
            if crl
                .crl_distribution_point_extension_configuration
                .as_ref()
                .is_some_and(|configuration| configuration.omit_extension)
                && crl.custom_cname.is_some()
            {
                return Some(RevocationConfigurationFault::ConflictingMembers(
                    OMITTED_DISTRIBUTION_POINT_WITH_CNAME_MESSAGE,
                ));
            }
        } else if let Some((member, _)) = [
            ("ExpirationInDays", crl.expiration_in_days.is_some()),
            ("CustomCname", crl.custom_cname.is_some()),
            ("S3BucketName", crl.s3_bucket_name.is_some()),
            ("S3ObjectAcl", crl.s3_object_acl.is_some()),
            (
                "CrlDistributionPointExtensionConfiguration",
                crl.crl_distribution_point_extension_configuration.is_some(),
            ),
            ("CrlType", crl.crl_type.is_some()),
            ("CustomPath", crl.custom_path.is_some()),
        ]
        .iter()
        .find(|(_, present)| *present)
        {
            return Some(RevocationConfigurationFault::Unserviceable(format!(
                "RevocationConfiguration.CrlConfiguration must contain only Enabled=false when \
                 CRLs are disabled, but {member} was included"
            )));
        }
    }
    if let Some(ocsp) = revocation.ocsp_configuration.as_ref() {
        if !ocsp.enabled && ocsp.ocsp_custom_cname.is_some() {
            return Some(RevocationConfigurationFault::Unserviceable(
                "RevocationConfiguration.OcspConfiguration must contain only Enabled=false \
                 when OCSP is disabled, but OcspCustomCname was included"
                    .to_string(),
            ));
        }
    }
    None
}

/// The WIRE name of the member an enabled CRL configuration publishes its CRLs to, which is
/// the name a refusal of the request-validation stage states for it.
pub const CRL_BUCKET_MEMBER: &str = "s3BucketName";

/// What is wrong with a submitted `RevocationConfiguration`, as the STAGE that refuses it.
///
/// The two stages answer a caller differently, so which one a rule belongs to is part of
/// the rule. A fault in what the configuration ITSELF states — a member its own state
/// requires, or two members that cannot be stated together — is the request-validation
/// stage's: a schema fault, refused with the generic `ValidationException`, which a caller
/// can repair from the message alone. A combination of members that cannot hold together
/// because of what the SERVICE would then have to do with it got past that stage and is a
/// semantic argument fault, which each caller lifts into its own modelled
/// `InvalidArgsException`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RevocationConfigurationFault {
    /// The configuration carried no value for a member its own state requires, named here
    /// by its wire name.
    MissingMember(&'static str),
    /// Two members the configuration cannot state together, with the sentence saying which
    /// published rule they broke.
    ConflictingMembers(&'static str),
    /// Members that cannot hold together, with the sentence saying which rule they broke.
    Unserviceable(String),
}

/// What this service answers a CRL configuration that both omits the distribution point
/// extension and names the alias that extension would publish: "This configuration cannot be
/// enabled with a custom CNAME set."
/// (`CrlDistributionPointExtensionConfiguration$OmitExtension`.)
///
/// Fixed text for the class, worded as Amazon Web Services Private CA words it: the two
/// members that cannot be stated together are named by their MODELLED names, and nothing of
/// the request — no alias, bucket, ARN or member path — is interpolated.
pub const OMITTED_DISTRIBUTION_POINT_WITH_CNAME_MESSAGE: &str =
    "You can't set a CustomCname if OmitExtension is set to true.";

/// What Amazon Web Services Private CA answers when an S3 bucket a request names is not
/// there: one sentence about the bucket, naming no bucket of its own.
///
/// Shared by every call of this service that is given a bucket to write to — the CRL
/// configuration of a create or an update, and the bucket an audit report is deposited in —
/// so one service does not word the same fault two ways.
pub const BUCKET_ABSENT_MESSAGE: &str = "The S3 bucket you provided does not exist.";

/// The sentence above, under the name the CRL configuration's refusal has always used.
pub const CRL_BUCKET_ABSENT_MESSAGE: &str = BUCKET_ABSENT_MESSAGE;

/// Whether an enabled CRL configuration names an S3 bucket that cannot be resolved, which
/// is what the call carrying it is refused for.
///
/// "Your private CA writes CRLs to an S3 bucket that you specify in the `S3BucketName`
/// parameter […] Your S3 bucket policy must give write permission to Amazon Web Services
/// Private CA" (`CrlConfiguration`): the bucket is where the CA's CRLs go, so the
/// configuration is resolved against object storage WHEN IT IS CONFIGURED — a CA accepted
/// with a bucket nothing holds would look healthy and publish no CRL at all.
///
/// S3 is reached the way the framework documents a handler reaching another emulated
/// service ([`smithy_server_runtime::OperationContext::callback`]): an AWS SDK client over
/// the in-process callback's HTTP client and credentials, so this crate names no other
/// service crate and the bucket is resolved against the emulator's OWN S3 state. The
/// question is put with `HeadBucket`, which S3 documents for exactly it ("You can use this
/// operation to determine if a bucket exists").
///
/// `false` means the call may proceed. That includes every answer that is not S3 reporting
/// the bucket absent — a router with no S3 mounted, or any other failure to ask: nothing
/// is then known about the bucket, and a refusal has to be grounded in what S3 answered
/// rather than assumed. A configuration whose CRLs are disabled, or that names no bucket
/// at all, has no bucket to resolve.
pub async fn unresolvable_crl_bucket(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    revocation: Option<&RevocationConfiguration>,
) -> bool {
    let Some(crl) = revocation.and_then(|revocation| revocation.crl_configuration.as_ref()) else {
        return false;
    };
    if !crl.enabled {
        return false;
    }
    let Some(bucket) = crl.s3_bucket_name.as_ref() else {
        return false;
    };
    bucket_absent(ctx, region, bucket).await
}

/// Whether S3 reports that this account holds no bucket named `bucket`.
///
/// The ONE place this service asks object storage that question, for every call that is
/// given a bucket to write to. `false` means the call may proceed: that includes every
/// answer that is not S3 reporting the bucket absent — a router with no S3 mounted, or any
/// other failure to ask — since nothing is then known about the bucket and a refusal has to
/// be grounded in what S3 answered rather than assumed.
pub async fn bucket_absent(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    bucket: &str,
) -> bool {
    let answer = s3_client(ctx, region)
        .head_bucket()
        .bucket(bucket)
        .send()
        .await;
    match answer {
        // S3 holds the bucket, so the call may write to it.
        Ok(_) => false,
        // S3 says this account holds no such bucket: the refusal is grounded.
        Err(error) if bucket_reported_absent(&error) => true,
        Err(error) => {
            ::tracing::warn!(
                bucket = %bucket,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "the bucket could not be resolved against S3"
            );
            false
        }
    }
}

/// The S3 client this service resolves a CRL bucket through: the AWS SDK driven over the
/// emulator's own in-process transport, signing in the emulator's name — the bucket is
/// read on this service's own behalf, not the caller's, exactly as Amazon Web Services
/// Private CA reads it with its own service principal.
///
/// NOTHING here opens a socket or leaves the host: the transport is
/// `ctx.callback().http_client()`, the emulator's in-process channel into its own services,
/// and the endpoint is only the address that channel resolves S3 by. The credentials are
/// the emulator's own and are used to sign this one call; they are never returned or
/// logged.
fn s3_client(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> ::aws_sdk_s3::Client {
    let config = ::aws_sdk_s3::Config::builder()
        .behavior_version(::aws_sdk_s3::config::BehaviorVersion::latest())
        .http_client(ctx.callback().http_client())
        .credentials_provider(ctx.callback().aws_credentials_provider())
        .region(::aws_sdk_s3::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://s3.{region}.amazonaws.com"))
        // The emulator serves one endpoint for every service, so the bucket travels in the
        // path rather than in a per-bucket host the router could not resolve.
        .force_path_style(true)
        // The transport is in-process: there is nothing to retry, time out or watch for a
        // stalled stream, and each of those would need a sleep implementation this client
        // is not given. The same goes for caching the identity: the credentials are handed
        // over directly.
        .retry_config(::aws_sdk_s3::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_s3::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_s3::config::IdentityCache::no_cache())
        .stalled_stream_protection(::aws_sdk_s3::config::StalledStreamProtectionConfig::disabled())
        .build();
    ::aws_sdk_s3::Client::from_conf(config)
}

/// Whether an S3 `HeadBucket` failure is S3 reporting that the bucket does not exist
/// (`NotFound`, the only error that operation models), as opposed to the call not reaching
/// S3 at all.
fn bucket_reported_absent(
    error: &::aws_sdk_s3::error::SdkError<
        ::aws_sdk_s3::operation::head_bucket::HeadBucketError,
        ::aws_smithy_runtime_api::client::orchestrator::HttpResponse,
    >,
) -> bool {
    error
        .as_service_error()
        .is_some_and(::aws_sdk_s3::operation::head_bucket::HeadBucketError::is_not_found)
}

// ---------------------------------------------------------------------------
// The CRLs a CA publishes
// ---------------------------------------------------------------------------

/// The folder a CA's CRL object is written under when its configuration names no custom
/// path: "crl/<ca-id>.crl" under the configured bucket.
///
/// `CrlConfiguration$CustomPath` "designates a custom file path in S3 for CRL(s). For
/// example, `http://<CustomName>/<CustomPath>/<CrlPartition_GUID>.crl`" — a custom path
/// REPLACES this folder, and the object's own name is the CA's id either way.
pub const DEFAULT_CRL_PATH: &str = "crl";

/// The media type a DER-encoded CRL object is stored with: "application/pkix-crl" (RFC
/// 2585 section 4.2, which registers that type for a DER CRL and the `.crl` extension
/// this object's key carries).
pub const CRL_CONTENT_TYPE: &str = "application/pkix-crl";

/// The `S3ObjectAcl` a CRL object is written with when the configuration names none: "If
/// no value is specified, the default is `PUBLIC_READ`." (`CrlConfiguration$S3ObjectAcl`).
pub const DEFAULT_S3_OBJECT_ACL: &str = "PUBLIC_READ";

/// The object-storage write a CA's CRL goes out as, built while this service's store is
/// held and performed once it has been released.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CrlPublication {
    /// The bucket `CrlConfiguration$S3BucketName` names.
    pub bucket: String,
    /// The object key the CRL is written at ([`crl_object_key`]).
    pub key: String,
    /// The canned object ACL the write carries ([`crl_object_canned_acl`]).
    pub canned_acl: &'static str,
    /// The DER-encoded RFC 5280 CRL itself.
    pub body: Vec<u8>,
}

/// The key a CA's CRL object is written at: the CA's id under the configuration's path.
///
/// `<CustomPath>/<ca-id>.crl` when the configuration names a custom path, else
/// `crl/<ca-id>.crl` ([`DEFAULT_CRL_PATH`]). A `CrlType` of `PARTITIONED` splits a CA's
/// revocation information across partitions, each with a partition id of its own; this
/// service keeps ONE partition per CA, so the object's name is the CA's id whichever type
/// the configuration names, and the type stays readable on the configuration.
///
/// The key is assembled from the custom path's SEGMENTS, keeping only the ones that name a
/// step down into it: an empty segment, and a `.` or `..` segment that would resolve
/// somewhere other than under the designated path, is not one, so no key this builds can
/// name an object outside `<bucket>/<path>`. A configuration carrying such a segment is
/// refused when it is submitted ([`revocation_configuration_fault`]), so this is the second
/// place the rule holds rather than the first.
pub fn crl_object_key(certificate_authority_id: &str, crl: &CrlConfiguration) -> String {
    let custom = crl
        .custom_path
        .as_deref()
        .map(|path| descending_segments(path).join("/"))
        .filter(|path| !path.is_empty());
    let path = custom.as_deref().unwrap_or(DEFAULT_CRL_PATH);
    format!("{path}/{certificate_authority_id}.crl")
}

/// The segments of `path` that each name a step DOWN into it: every non-empty segment other
/// than the two relative ones (`.`, which names the path itself, and `..`, which names the
/// one above it).
pub fn descending_segments(path: &str) -> Vec<&str> {
    path.split('/')
        .filter(|segment| !segment.is_empty() && *segment != "." && *segment != "..")
        .collect()
}

/// Whether `path` names anywhere but a place under the path it is relative to: a segment of
/// `.` or `..`, which is what a path that leaves the designated folder is spelled with.
fn path_leaves_its_folder(path: &str) -> bool {
    path.split('/')
        .any(|segment| segment == "." || segment == "..")
}

/// The canned object ACL a CRL object is written with, for the `S3ObjectAcl` the
/// configuration names (or does not: [`DEFAULT_S3_OBJECT_ACL`]).
///
/// "Determines whether the CRL will be publicly readable or privately held in the CRL
/// Amazon S3 bucket. If you choose PUBLIC_READ, the CRL will be accessible over the public
/// internet. If you choose BUCKET_OWNER_FULL_CONTROL, only the owner of the CRL S3 bucket
/// can access the CRL" (`CrlConfiguration$S3ObjectAcl`) — which are the `public-read` and
/// `bucket-owner-full-control` rows of the canned-ACL table S3 publishes.
pub fn crl_object_canned_acl(s3_object_acl: Option<&str>) -> &'static str {
    match s3_object_acl.unwrap_or(DEFAULT_S3_OBJECT_ACL) {
        "BUCKET_OWNER_FULL_CONTROL" => "bucket-owner-full-control",
        _ => "public-read",
    }
}

/// The `CRLReason` code a modelled `RevocationReason` names, or `None` for a value the
/// enum does not declare.
///
/// The two catalogs are one: `RevocationReason` publishes the reasons of the "X509v3 CRL
/// Reason Code" a CRL entry carries (`CrlConfiguration` docs), whose values are the
/// `CRLReason` enumeration of RFC 5280 section 5.3.1 — `unspecified(0)`,
/// `keyCompromise(1)`, `cACompromise(2)`, `affiliationChanged(3)`, `superseded(4)`,
/// `cessationOfOperation(5)`, `certificateHold(6)`, `removeFromCRL(8)`,
/// `privilegeWithdrawn(9)`, `aACompromise(10)`. The two the model does not publish
/// (`certificateHold`, `removeFromCRL`) belong to a suspension this service has no
/// operation for, so no reason maps to them.
pub fn crl_reason_code(revocation_reason: &str) -> Option<u8> {
    Some(match revocation_reason {
        "UNSPECIFIED" => 0,
        "KEY_COMPROMISE" => 1,
        "CERTIFICATE_AUTHORITY_COMPROMISE" => 2,
        "AFFILIATION_CHANGED" => 3,
        "SUPERSEDED" => 4,
        "CESSATION_OF_OPERATION" => 5,
        "PRIVILEGE_WITHDRAWN" => 9,
        "A_A_COMPROMISE" => 10,
        _ => return None,
    })
}

/// The CRL this CA publishes as of `now`, when it publishes one at all.
///
/// "You can enable CRLs for your new or an existing private CA by setting the Enabled
/// parameter to true. Your private CA writes CRLs to an S3 bucket that you specify in the
/// S3BucketName parameter" (`CrlConfiguration`): publication is the CA's OWN effect of
/// holding an enabled configuration — no operation publishes a CRL on a caller's behalf —
/// so every call that changes what the CRL says, or which bucket it goes to, asks for the
/// CRL it now owes here.
///
/// `None` when this CA publishes nothing: CRLs disabled or never configured, no bucket to
/// write to, or a CA that does not hold the certificate it signs its CRL under yet. The
/// CRL number is advanced and the publication instant recorded here, so each CRL that goes
/// out carries the next number of the sequence.
pub fn due_crl_publication(
    record: &mut CertificateAuthorityRecord,
    now: SystemTime,
) -> Option<CrlPublication> {
    let crl = record
        .revocation_configuration
        .as_ref()
        .and_then(|revocation| revocation.crl_configuration.clone())
        .filter(|crl| crl.enabled)?;
    let bucket = crl.s3_bucket_name.clone()?;
    // A CRL is signed by the CA under the certificate it holds, so a CA still waiting for
    // one publishes nothing yet; its activation is what asks again.
    record.ca_certificate_pem.as_ref()?;
    let id = parse_certificate_authority_arn(&record.arn)?.id;
    let number = record.crl_number.saturating_add(1);
    let body = encode_crl(record, &crl, number, now)?;
    record.crl_number = number;
    record.crl_published_at = Some(now);
    Some(CrlPublication {
        bucket,
        key: crl_object_key(&id, &crl),
        canned_acl: crl_object_canned_acl(crl.s3_object_acl.as_deref()),
        body,
    })
}

/// The CA's FIRST CRL, when the call at hand is what brings it into being.
///
/// A CA that has already published one says nothing new when it merely issues another
/// certificate — a CRL lists the certificates that were REVOKED — so an issuance refreshes
/// nothing, while the first issuance of a CA whose CRLs are enabled is what the CRL a
/// relying party fetches starts existing at.
pub fn initial_crl_publication(
    record: &mut CertificateAuthorityRecord,
    now: SystemTime,
) -> Option<CrlPublication> {
    if record.crl_published_at.is_some() {
        return None;
    }
    due_crl_publication(record, now)
}

/// Deposit a CA's CRL in object storage, through the same in-process S3 client the CRL
/// bucket is resolved with ([`s3_client`]).
///
/// AWS Private CA takes minutes over this ("A CRL is typically updated approximately 30
/// minutes after a certificate is revoked"); emulated, the object is there by the time the
/// call that changed the CRL answers, so a client that reads the CRL next sees it.
///
/// A write object storage refuses is reported to the emulator's own log and nothing else:
/// the CA keeps its configuration and reports it, exactly as a CA whose CRL bucket is
/// misconfigured does, and the next change to its revocation information tries again.
pub async fn publish_crl(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    publication: CrlPublication,
) {
    let answer = s3_client(ctx, region)
        .put_object()
        .bucket(&publication.bucket)
        .key(&publication.key)
        .content_type(CRL_CONTENT_TYPE)
        .acl(::aws_sdk_s3::types::ObjectCannedAcl::from(
            publication.canned_acl,
        ))
        .body(::aws_sdk_s3::primitives::ByteStream::from(
            publication.body,
        ))
        .send()
        .await;
    if let Err(error) = answer {
        ::tracing::warn!(
            bucket = %publication.bucket,
            key = %publication.key,
            error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
            "the certificate revocation list could not be written to object storage"
        );
    }
}

/// The DER-encoded CRL a CA publishes: the RFC 5280 section 5.1 `CertificateList` whose
/// fields are the ones `CrlConfiguration` lists.
///
/// `Version` v2, the signature algorithm the CA was created with, the `Issuer` the CA's
/// own X.500 distinguished name, `thisUpdate` the instant it is published at, `nextUpdate`
/// that instant plus `ExpirationInDays` ("Your private CA uses the value in the
/// ExpirationInDays parameter to calculate the nextUpdate field in the CRL"), the revoked
/// certificates each with its serial, its revocation date and its reason code, and the
/// CRL's own number.
///
/// As with the certificates this service writes, the signature is made with the CA's own
/// private key ([`object_signature`]).
fn encode_crl(
    ca: &CertificateAuthorityRecord,
    crl: &CrlConfiguration,
    number: u64,
    now: SystemTime,
) -> Option<Vec<u8>> {
    let algorithm = signature_algorithm_identifier(&ca.configuration.signing_algorithm)?;
    let revoked = revoked_certificates(ca, now)?;
    let tbs_cert_list = TbsCertList {
        // "Version: The current version number defined in RFC 5280 is V2. The integer
        // value is 0x1." (`CrlConfiguration` docs.)
        version: Version::V2,
        signature: algorithm.clone(),
        issuer: subject_name(&ca.configuration.subject)?,
        this_update: certificate_time(now)?,
        next_update: Some(certificate_time(crl_next_update(crl, now)?)?),
        // RFC 5280 section 5.1.2.6: the field is absent when no certificate is revoked.
        revoked_certificates: (!revoked.is_empty()).then_some(revoked),
        crl_extensions: Some(crl_extensions(ca, number)?),
    };
    let signature = object_signature(
        ca.key_pair.as_ref(),
        &ca.configuration.signing_algorithm,
        &tbs_cert_list.to_der().ok()?,
    )?;
    CertificateList {
        tbs_cert_list,
        signature_algorithm: algorithm,
        signature,
    }
    .to_der()
    .ok()
}

/// When the CRL published at `now` says the next one is due: `ExpirationInDays` days on,
/// which is the "validity period of the CRL in days"
/// (`CrlConfiguration$ExpirationInDays`).
fn crl_next_update(crl: &CrlConfiguration, now: SystemTime) -> Option<SystemTime> {
    let days = crl.expiration_in_days?;
    now.checked_add(ONE_DAY * u32::try_from(days).ok()?)
}

/// The entries the CRL published at `now` lists, in the order they were revoked.
fn revoked_certificates(
    ca: &CertificateAuthorityRecord,
    now: SystemTime,
) -> Option<Vec<RevokedCert>> {
    let mut revoked: Vec<(SystemTime, &str, RevokedCert)> = Vec::new();
    for certificate in ca.certificates.values() {
        let Some(revocation) = certificate.revocation.as_ref() else {
            continue;
        };
        if !listed_in_crl(certificate, ca.crl_published_at, now) {
            continue;
        }
        revoked.push((
            revocation.revoked_at,
            certificate.serial.as_str(),
            RevokedCert {
                serial_number: serial_number(&certificate.serial)?,
                revocation_date: certificate_time(revocation.revoked_at)?,
                crl_entry_extensions: Some(vec![crl_reason_extension(&revocation.reason)?]),
            },
        ));
    }
    // A total order over the revocation and the serial, so the same revocation information
    // always encodes to the same list however the certificates are held.
    revoked.sort_by(|left, right| (left.0, left.1).cmp(&(right.0, right.1)));
    Some(revoked.into_iter().map(|(_, _, entry)| entry).collect())
}

/// Whether a revoked certificate is listed in the CRL published at `now`.
///
/// "When a certificate is revoked, it appears in the CRL until the certificate expires,
/// and then in one additional CRL after expiration" (`CrlConfiguration` docs): a
/// certificate still inside its validity window is listed, and one that expired since the
/// CRL that went out before this one is listed that once more.
fn listed_in_crl(
    certificate: &IssuedCertificate,
    previous_publication: Option<SystemTime>,
    now: SystemTime,
) -> bool {
    certificate.not_after > now
        || previous_publication.is_some_and(|previous| certificate.not_after > previous)
}

/// The CRL's own extensions: its number, and the key identifier of the CA that signed it
/// when the CA's certificate publishes one.
///
/// "CRL Extensions: X509v3 Authority Key Identifier: Identifies the public key associated
/// with the private key used to sign the certificate. X509v3 CRL Number: Decimal sequence
/// number for the CRL." (`CrlConfiguration` docs.)
fn crl_extensions(ca: &CertificateAuthorityRecord, number: u64) -> Option<Extensions> {
    let mut extensions: Extensions = vec![crl_number_extension(number)?];
    if let Some(identifier) = authority_key_identifier_extension(ca) {
        extensions.push(identifier);
    }
    Some(extensions)
}

/// The `cRLNumber` extension (RFC 5280 section 5.2.3): a non-critical INTEGER that
/// increases with every CRL the CA issues.
fn crl_number_extension(number: u64) -> Option<Extension> {
    Some(Extension {
        extn_id: oid::CRL_NUMBER,
        critical: false,
        extn_value: OctetString::new(Encode::to_der(&number).ok()?).ok()?,
    })
}

/// The `reasonCode` CRL entry extension (RFC 5280 section 5.3.1): the non-critical
/// `CRLReason` ENUMERATED the revocation's reason names ([`crl_reason_code`]).
fn crl_reason_extension(revocation_reason: &str) -> Option<Extension> {
    let reason = Any::new(DerTag::Enumerated, [crl_reason_code(revocation_reason)?].as_slice())
        .ok()?;
    Some(Extension {
        extn_id: oid::CRL_REASON,
        critical: false,
        extn_value: OctetString::new(reason.to_der().ok()?).ok()?,
    })
}

/// The `authorityKeyIdentifier` extension (RFC 5280 section 5.2.1) of a CRL this CA signs:
/// the identifier of the key the CRL was signed with ([`issuer_key_identifier`]), which is
/// the key the CA's OWN certificate holds.
///
/// `None` when the CA holds no certificate this service can read.
fn authority_key_identifier_extension(ca: &CertificateAuthorityRecord) -> Option<Extension> {
    let key_identifier = issuer_key_identifier(ca)?;
    let identifier = ::x509_cert::ext::pkix::AuthorityKeyIdentifier {
        key_identifier: Some(key_identifier),
        authority_cert_issuer: None,
        authority_cert_serial_number: None,
    };
    Some(Extension {
        extn_id: oid::AUTHORITY_KEY_IDENTIFIER,
        critical: false,
        extn_value: OctetString::new(identifier.to_der().ok()?).ok()?,
    })
}

/// How many bytes a derived key identifier carries: 160 bits, the length RFC 7093 section
/// 2 derives from a SHA-256 digest and the length RFC 5280 section 4.2.1.2's own methods
/// produce.
const KEY_IDENTIFIER_BYTES: usize = 20;

/// The key identifier the certificates and CRLs this CA signs name as their authority key
/// identifier: the identifier of the key it signs with.
///
/// The one the CA's own certificate publishes as its subject key identifier, since that is
/// the value a relying party matches the extension against; a certificate that publishes
/// none is identified by the key it holds ([`derived_key_identifier`]).
///
/// `None` when the CA holds no certificate this service can read.
pub fn issuer_key_identifier(ca: &CertificateAuthorityRecord) -> Option<OctetString> {
    let der = parse_pem_block(ca.ca_certificate_pem.as_deref()?, PEM_LABEL_CERTIFICATE)?;
    let certificate = Certificate::from_der(&der).ok()?;
    match published_key_identifier(&certificate) {
        Some(published) => Some(published),
        None => derived_key_identifier(&certificate.tbs_certificate.subject_public_key_info),
    }
}

/// The key identifier of `public_key`: the leftmost 160 bits of the SHA-256 digest over its
/// `subjectPublicKey` BIT STRING, which is method 1 of RFC 7093 section 2 ("Other methods of
/// generating unique numbers are also acceptable", RFC 5280 section 4.2.1.2).
///
/// The ONE derivation of a key identifier in this service, so the identifier a certificate
/// publishes for a key and the identifier the certificates signed with that key name are the
/// same value.
pub fn derived_key_identifier(public_key: &SubjectPublicKeyInfoOwned) -> Option<OctetString> {
    OctetString::new(
        &Sha256::digest(public_key.subject_public_key.raw_bytes())[..KEY_IDENTIFIER_BYTES],
    )
    .ok()
}

/// The key identifier `certificate` publishes as its subject key identifier (RFC 5280
/// section 4.2.1.2), when it publishes one.
fn published_key_identifier(certificate: &Certificate) -> Option<OctetString> {
    let extension = certificate
        .tbs_certificate
        .extensions
        .as_ref()?
        .iter()
        .find(|extension| extension.extn_id == oid::SUBJECT_KEY_IDENTIFIER)?;
    OctetString::from_der(extension.extn_value.as_bytes()).ok()
}

/// Whether `document` is a resource-based policy this service will attach to a private CA.
///
/// "If this policy does not contain all required statements or if it includes any statement
/// that is not allowed, the PutPolicy action returns an InvalidPolicyException. For
/// information about IAM policy and statement structure, see Overview of JSON Policies."
/// (`PutPolicyRequest$Policy` docs) — so the grammar an IAM resource policy has to satisfy:
/// a JSON object carrying a `Statement` that is a non-empty list of statement objects (or
/// a single statement object), each with an `Effect` of exactly `Allow` or `Deny` and an
/// `Action`. The document is read as JSON by `serde_json`, never matched as text.
///
/// This is the GRAMMAR the policy language sets, not what a private CA's policy may say:
/// a well-formed statement that does not grant is refused on its own
/// ([`NON_ALLOW_EFFECT_MESSAGE`], from `invalid_policy_reason`).
///
/// The same grammar is checked by Amazon S3's `is_valid_policy_document`
/// (`services/service-com-amazonaws-s3-amazons3/src/state.rs`) for `PutBucketPolicy`; both
/// are service-private today, and lifting the one grammar into `services/common` is a
/// cross-service refactor of its own rather than part of this service's policy operations.
pub fn is_well_formed_policy(document: &str) -> bool {
    let Ok(parsed) = serde_json::from_str::<serde_json::Value>(document) else {
        return false;
    };
    let Some(members) = parsed.as_object() else {
        return false;
    };
    // An IAM policy's statements are its `Statement` element: one statement object, or an
    // array of them that is not empty.
    let statements: Vec<&serde_json::Value> = match members.get("Statement") {
        Some(serde_json::Value::Array(list)) => list.iter().collect(),
        Some(single @ serde_json::Value::Object(_)) => vec![single],
        _ => return false,
    };
    !statements.is_empty()
        && statements.iter().all(|statement| {
            let Some(statement) = statement.as_object() else {
                return false;
            };
            let effect = statement.get("Effect").and_then(serde_json::Value::as_str);
            matches!(effect, Some("Allow") | Some("Deny")) && statement.contains_key("Action")
        })
}

// ---------------------------------------------------------------------------
// Tags
// ---------------------------------------------------------------------------

/// Most tags ONE private CA holds at once: "You can associate up to 50 tags with a
/// private CA." (`TooManyTagsException` and the `Tag` shape's documentation).
///
/// `TagList`'s `len 1..50` bounds a single REQUEST's list, which the boundary validator
/// already refuses past; this bounds the RESOURCE, so two accepted requests of thirty
/// tags each cannot leave a CA holding sixty.
pub const MAX_TAGS_PER_CERTIFICATE_AUTHORITY: usize = 50;

/// Why `tags` is not a usable tag set, or `None` when it is.
///
/// Two rules hold for every tag set this service reads, whichever operation carries it:
/// the `aws:` prefix is reserved for the tags Amazon Web Services itself applies, and a
/// key identifies one tag of a resource, so one request cannot name the same key twice.
/// Each caller lifts the reason into its own modelled `InvalidTagException`.
pub fn invalid_tag_reason(tags: &[Tag]) -> Option<String> {
    let mut seen: HashSet<&str> = HashSet::new();
    for tag in tags {
        if tag.key.starts_with("aws:") {
            return Some(RESERVED_TAG_MESSAGE.to_string());
        }
        if !seen.insert(tag.key.as_str()) {
            return Some(format!("the tag key '{}' is given twice", tag.key));
        }
    }
    None
}

/// How many tags a resource holding `existing` would hold once `tags` were applied to
/// it, counted BEFORE anything is written so a refusal leaves the resource untouched.
///
/// A key the resource already carries takes a new value rather than becoming a second
/// tag, so only the keys it does not carry yet add to the count.
pub fn tag_count_after(existing: &[Tag], tags: &[Tag]) -> usize {
    let mut keys: HashSet<&str> = existing.iter().map(|tag| tag.key.as_str()).collect();
    for tag in tags {
        keys.insert(tag.key.as_str());
    }
    keys.len()
}

/// Apply `tags` to the tag set `existing`: a key the resource already carries takes the
/// new value (in place, so the resource's tag order does not change), and a key it does
/// not carry is appended.
pub fn apply_tags(existing: &mut Vec<Tag>, tags: Vec<Tag>) {
    for tag in tags {
        match existing.iter_mut().find(|held| held.key == tag.key) {
            Some(held) => held.value = tag.value,
            None => existing.push(tag),
        }
    }
}

/// Remove `tags` from the tag set `existing`.
///
/// "If you do not specify the value portion of the tag when calling this action, the tag
/// will be removed regardless of value. If you specify a value, the tag is removed only
/// if it is associated with the specified value."
/// (`UntagCertificateAuthority` documentation.) `TagValue`'s `len 0..256` makes the empty
/// string a value a tag can carry, and a tag stored without one carries exactly that, so
/// the two compare equal.
pub fn remove_tags(existing: &mut Vec<Tag>, tags: &[Tag]) {
    existing.retain(|held| {
        !tags.iter().any(|removed| {
            removed.key == held.key
                && removed.value.as_ref().is_none_or(|value| {
                    value.as_str() == held.value.as_deref().unwrap_or_default()
                })
        })
    });
}

/// What identifies ONE `IssueCertificate` call, so a repeat of it is answered with the
/// certificate the first call minted instead of a second certificate.
///
/// `IssueCertificate` is `@idempotent`, and the certificate a call asks for is wholly
/// described by the request: the certification request being signed, the algorithm the CA
/// is asked to sign with, the template that chooses the certificate's extensions, the
/// validity window, and the `ApiPassthrough` content to be carried into it. Two calls
/// agreeing on all of those ask the CA for the same certificate; a call differing in any of
/// them asks for a different one and is issued its own.
///
/// The CA is not a field here: these records live ON the CA that answered them, so a request
/// repeated against another CA is a request of that CA's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssuedCertificateRequest {
    /// The `Csr` member, as the bytes the request carried.
    pub csr: Vec<u8>,
    /// The `SigningAlgorithm` member.
    pub signing_algorithm: String,
    /// The `TemplateArn` member, when the request named one.
    pub template_arn: Option<String>,
    /// The `Validity` member.
    pub validity: (i64, String),
    /// The `ValidityNotBefore` member, when the request named one.
    pub validity_not_before: Option<(i64, String)>,
    /// The `ApiPassthrough` member, when the request carried one.
    pub api_passthrough: Option<ApiPassthrough>,
}

impl IssuedCertificateRequest {
    /// The identity of a request asking for a certificate over `csr`.
    pub fn new(
        csr: Vec<u8>,
        signing_algorithm: String,
        template_arn: Option<String>,
        validity: &Validity,
        validity_not_before: Option<&Validity>,
        api_passthrough: Option<ApiPassthrough>,
    ) -> Self {
        Self {
            csr,
            signing_algorithm,
            template_arn,
            validity: validity_key(validity),
            validity_not_before: validity_not_before.map(validity_key),
            api_passthrough,
        }
    }
}

/// A `Validity` as the pair of values it is: the period's length and the type it counts in.
fn validity_key(validity: &Validity) -> (i64, String) {
    (validity.value, validity.r#type.clone())
}

/// What identifies ONE `CreateCertificateAuthority` call, so a repeat of it is answered with
/// the CA the first call minted instead of a second CA.
///
/// The configuration is what the caller asked for, and the account and region are WHOSE CA it
/// is: a private CA belongs to the account that created it ([`CertificateAuthorityRecord`]'s
/// `owner_account`, which is what a listing filters a `ResourceOwner` by) and lives in the
/// region its ARN names. The idempotency records of this operation sit on the store as a
/// whole rather than on one account's CAs, so all three are part of the identity: a token is
/// honoured as a replay only for a request that asks the same account, in the same region,
/// for the same configuration, and a token presented for anything else mints a CA of its own
/// rather than handing back a handle to a CA that is not the caller's.
#[derive(Debug, Clone, PartialEq)]
pub struct CreatedAuthorityRequest {
    /// The account the CA belongs to.
    pub owner_account: String,
    /// The region the CA lives in.
    pub region: String,
    /// The `CertificateAuthorityConfiguration` member the CA is built from.
    pub configuration: CertificateAuthorityConfiguration,
}

impl CreatedAuthorityRequest {
    /// The identity of a request asking `owner_account` for a CA in `region` built from
    /// `configuration`.
    pub fn new(
        owner_account: &str,
        region: &str,
        configuration: CertificateAuthorityConfiguration,
    ) -> Self {
        Self {
            owner_account: owner_account.to_string(),
            region: region.to_string(),
            configuration,
        }
    }
}

/// The bucket an `IssueCertificate` is remembered under: the `IdempotencyToken` it carried,
/// and the empty string for a request that carried none.
///
/// A token distinguishes retries of one call from calls that each want their own
/// certificate — "If you change the idempotency token for each call, Amazon Web Services
/// Private CA recognizes that you are requesting multiple certificates"
/// (`IssueCertificateRequest$IdempotencyToken`) — so it is the bucket, and the request's own
/// identity ([`IssuedCertificateRequest`]) is what is matched inside it. `IdempotencyToken`
/// is modelled `length 1..36`, so no token a caller can send is the empty string: the
/// requests that named none share a bucket of their own and are told apart by their content
/// alone.
pub fn issuance_idempotency_key(token: Option<&String>) -> String {
    token.cloned().unwrap_or_default()
}

/// An idempotency token's record: what it minted, WHAT IT WAS ASKED FOR, and when it
/// stops being honoured.
///
/// `R` is how the operation that holds the map identifies the request a token was honoured
/// for, so the map can tell a repeat of that request from a different one sharing its
/// token. An operation whose token identifies the call on its own carries `()`.
#[derive(Debug, Clone)]
pub struct IdempotencyEntry<R> {
    /// The ARN the first call with this token returned.
    pub arn: String,
    /// The request that ARN was minted for.
    pub request: R,
    /// End of the five-minute window the token is honoured for.
    pub expires_at: SystemTime,
    /// Insertion order, so eviction at the ceiling is the oldest entry rather than an
    /// arbitrary one of several recorded in the same clock tick.
    pub sequence: u64,
}

impl<R> IdempotencyEntry<R> {
    /// A token recorded at `now` for `request`, honoured for [`IDEMPOTENCY_WINDOW`].
    pub fn new(arn: &str, request: R, now: SystemTime) -> Self {
        Self {
            arn: arn.to_string(),
            request,
            expires_at: now + IDEMPOTENCY_WINDOW,
            sequence: next_sequence(),
        }
    }
}

/// An idempotency map: for each token, what each REQUEST that token was honoured for
/// minted.
///
/// A token is honoured per request, not per token, so the map holds one entry per distinct
/// request a token has been seen with inside the window. That is what keeps the record of
/// one call from being displaced by another call sharing its token: the first request stays
/// replayable for the whole of its window however many other requests reuse the token in
/// the meantime.
pub type IdempotencyRecords<R> = HashMap<String, Vec<IdempotencyEntry<R>>>;

/// Look `token` up in an idempotency map, ignoring (and pruning) expired entries.
///
/// A token is a replay only of the request it was recorded for: an entry whose `request`
/// is not the one being made is not this call's answer, so the caller goes on and creates
/// a resource of its own rather than being handed a handle to someone else's. That is what
/// keeps one token reused across two different requests from collapsing them into one
/// resource — the token distinguishes retries of ONE call, it does not name a resource —
/// and the request it does not match is left where it is, so a later retry of THAT request
/// still replays what it minted.
pub fn replay_idempotent<R: PartialEq>(
    tokens: &mut IdempotencyRecords<R>,
    token: Option<&String>,
    request: &R,
    now: SystemTime,
) -> Option<String> {
    let token = token?;
    let entries = tokens.get_mut(token)?;
    entries.retain(|entry| now < entry.expires_at);
    if entries.is_empty() {
        tokens.remove(token);
        return None;
    }
    entries
        .iter()
        .find(|entry| entry.request == *request)
        .map(|entry| entry.arn.clone())
}

/// Remember that `token` minted `arn` for `request`, keeping the map bounded.
///
/// The five-minute window is enforced as a real eviction policy, not only when the same
/// token comes back: every record sweeps the entries that have expired, and at
/// [`MAX_IDEMPOTENCY_TOKENS`] live entries the oldest INSERTED entry (the lowest
/// `sequence`) makes room, whichever token holds it. A caller sending a fresh token per
/// request — or one token with a fresh request each time — therefore cannot grow this map
/// without limit, and an absent token is simply not remembered.
pub fn record_idempotent<R>(
    tokens: &mut IdempotencyRecords<R>,
    token: Option<String>,
    request: R,
    arn: &str,
    now: SystemTime,
) {
    let Some(token) = token else {
        return;
    };
    for entries in tokens.values_mut() {
        entries.retain(|entry| now < entry.expires_at);
    }
    tokens.retain(|_, entries| !entries.is_empty());
    while recorded_requests(tokens) >= MAX_IDEMPOTENCY_TOKENS {
        if !evict_oldest(tokens) {
            break;
        }
    }
    tokens
        .entry(token)
        .or_default()
        .push(IdempotencyEntry::new(arn, request, now));
}

/// How many requests the map remembers across every token it holds, which is what
/// [`MAX_IDEMPOTENCY_TOKENS`] bounds.
fn recorded_requests<R>(tokens: &IdempotencyRecords<R>) -> usize {
    tokens.values().map(Vec::len).sum()
}

/// Drop the entry inserted longest ago (the lowest `sequence`), and the token with it when
/// it was that token's last. `false` when there was nothing left to drop.
fn evict_oldest<R>(tokens: &mut IdempotencyRecords<R>) -> bool {
    let Some((token, position)) = tokens
        .iter()
        .flat_map(|(token, entries)| {
            entries
                .iter()
                .enumerate()
                .map(move |(position, entry)| (entry.sequence, token.clone(), position))
        })
        .min_by_key(|(sequence, _, _)| *sequence)
        .map(|(_, token, position)| (token, position))
    else {
        return false;
    };
    let Some(entries) = tokens.get_mut(&token) else {
        return false;
    };
    entries.remove(position);
    if entries.is_empty() {
        tokens.remove(&token);
    }
    true
}

/// Per-service state for com-amazonaws-acmpca-acmprivateca. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsAcmpcaAcmprivatecaState {
    /// Every private CA, keyed by its ARN.
    pub certificate_authorities: HashMap<String, CertificateAuthorityRecord>,
    /// `CreateCertificateAuthority` idempotency tokens -> the CA ARN each minted, with the
    /// request that CA was created for: a token is honoured as a replay only of a request
    /// asking the same account, in the same region, for the same configuration
    /// ([`CreatedAuthorityRequest`]), and it holds one record per request it has been seen
    /// with ([`replay_idempotent`]).
    pub create_idempotency: IdempotencyRecords<CreatedAuthorityRequest>,
}

impl ComAmazonawsAcmpcaAcmprivatecaState {
    /// The CA named by `arn`, with any status change the clock owed already applied.
    ///
    /// A deleted CA whose restoration window has closed was permanently deleted at the
    /// instant that window ended, whether or not anybody called, so it is dropped here
    /// and answers as no resource at all.
    pub fn certificate_authority_mut(
        &mut self,
        arn: &str,
        now: SystemTime,
    ) -> Option<&mut CertificateAuthorityRecord> {
        self.reap_permanently_deleted(now);
        let record = self.certificate_authorities.get_mut(arn)?;
        record.settle(now);
        Some(record)
    }

    /// Materialise every status change the clock owed at `now`, across every CA.
    ///
    /// What [`certificate_authority_mut`](Self::certificate_authority_mut) does for one
    /// CA, for the whole store: an operation that reports on more than one CA at a time
    /// applies the transitions that were due at the same instant whether or not anybody
    /// called, so no two readers disagree about a CA's current status.
    pub fn settle(&mut self, now: SystemTime) {
        self.reap_permanently_deleted(now);
        for record in self.certificate_authorities.values_mut() {
            record.settle(now);
        }
    }

    /// Drop every CA whose restoration window has closed.
    ///
    /// The permanent deletion came due the instant that window ended, whatever is read
    /// next, so both readers above sweep the whole store rather than only the resource
    /// they were asked about: a listing and a describe can never disagree about whether a
    /// CA is still there, and a reaped CA stops counting against
    /// [`MAX_CERTIFICATE_AUTHORITIES`].
    fn reap_permanently_deleted(&mut self, now: SystemTime) {
        self.certificate_authorities
            .retain(|_, record| !record.is_permanently_deleted(now));
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsAcmpcaAcmprivatecaState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsAcmpcaAcmprivatecaState>)
}

// ---------------------------------------------------------------------------
// ARNs
// ---------------------------------------------------------------------------

/// The CA ARN a request named, when it is well formed for THIS service.
///
/// A segment of an AWS ARN never contains `:`, so splitting on it is exact:
/// `arn:<partition>:acm-pca:<region>:<account>:certificate-authority/<id>`.
pub fn parse_certificate_authority_arn(arn: &str) -> Option<CertificateAuthorityArn> {
    let fields: Vec<&str> = arn.split(':').collect();
    let [prefix, partition, service, region, account, resource] = fields.as_slice() else {
        return None;
    };
    if *prefix != "arn" || partition.is_empty() || *service != ARN_SERVICE {
        return None;
    }
    // `AccountId` is twelve digits (len 12..12, pattern ^[0-9]+$), so an ARN with an
    // empty or non-numeric account segment names no resource of this service.
    if account.len() != 12 || !account.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let segments: Vec<&str> = resource.split('/').collect();
    let ["certificate-authority", id] = segments.as_slice() else {
        return None;
    };
    if id.is_empty() {
        return None;
    }
    Some(CertificateAuthorityArn {
        region: (*region).to_string(),
        account: (*account).to_string(),
        id: (*id).to_string(),
    })
}

/// The parts of a well-formed certificate-authority ARN.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CertificateAuthorityArn {
    /// Region segment (empty for a partition-global ARN, which the pattern allows).
    pub region: String,
    /// Account segment.
    pub account: String,
    /// The CA's id, the last resource segment.
    pub id: String,
}

/// Split a certificate ARN into the ARN of the CA that issued it and its serial.
///
/// `IssueCertificate` returns `<ca arn>/certificate/<serial>`, so the CA ARN is a
/// prefix of the certificate ARN — a certificate ARN minted by one CA therefore never
/// resolves against another.
pub fn parse_certificate_arn(arn: &str) -> Option<(String, String)> {
    let (ca_arn, serial) = arn.rsplit_once("/certificate/")?;
    if serial.is_empty() || serial.contains('/') {
        return None;
    }
    parse_certificate_authority_arn(ca_arn)?;
    Some((ca_arn.to_string(), serial.to_string()))
}

/// The serial number `serial` names, as the hexadecimal VALUE it denotes: lower case,
/// with no leading zeros and no separators, or `None` when `serial` is not a hexadecimal
/// number at all.
///
/// `RevokeCertificateRequest$CertificateSerial` is "the serial number of the certificate
/// to be revoked. This must be in hexadecimal format", and the docs have it read off
/// `openssl x509 -text`, which prints a serial as colon-separated octets
/// (`0a:1b:2c:3d`) — so the separators between octets, the case of the digits and a
/// leading zero are all spellings of one number rather than part of it (the DER integer a
/// certificate carries its serial as drops leading zeros, and carries one only to keep the
/// value positive). The serial a request names and the serial a certificate holds are
/// therefore compared as the numbers they are, not as the text they arrived in.
pub fn normalized_serial(serial: &str) -> Option<String> {
    let digits: String = serial
        .chars()
        .filter(|character| !SERIAL_SEPARATORS.contains(character))
        .collect();
    if digits.is_empty() || !digits.chars().all(|digit| digit.is_ascii_hexdigit()) {
        return None;
    }
    let value = digits.trim_start_matches('0');
    Some(if value.is_empty() {
        "0".to_string()
    } else {
        value.to_ascii_lowercase()
    })
}

/// `serial` as the octet pairs X.509 tooling renders a certificate serial number in: pairs
/// of lower-case hexadecimal digits separated by colons (`0a:1b:2c:3d`).
///
/// This is the rendering `openssl x509 -serial`/`-text` prints and the one
/// `RevokeCertificateRequest$CertificateSerial`'s docs have a serial copied out of, so it is
/// the form a client holds a serial in once it has read the certificate itself — which is
/// what lets it join a serial it holds to what this service states about that certificate.
/// The value is unchanged: only the separators between its octets are written, and a
/// spelling with an odd number of digits names a leading nibble of a leading zero octet.
pub fn serial_octets(serial: &str) -> String {
    let digits: String = serial
        .chars()
        .filter(|character| !SERIAL_SEPARATORS.contains(character))
        .map(|digit| digit.to_ascii_lowercase())
        .collect();
    let digits = if digits.len().is_multiple_of(2) {
        digits
    } else {
        format!("0{digits}")
    };
    digits
        .as_bytes()
        .chunks(2)
        .map(|pair| String::from_utf8_lossy(pair).to_string())
        .collect::<Vec<String>>()
        .join(":")
}

/// `serial` as the DECIMAL integer the number it names is, or `None` when `serial` is not a
/// hexadecimal number at all.
///
/// Amazon Web Services Private CA states a serial number as the integer it is rather than as
/// hexadecimal digits: the serial number in the certificate ARN its own documentation
/// publishes is a 39-digit value (`GetCertificateRequest$CertificateArn`: "The ARN contains
/// the certificate serial number and must be in the following form:
/// …/certificate/286535153982981100925020015808220737245"), which no hexadecimal rendering of
/// a serial number is.
///
/// A serial number is an integer of up to 20 octets (RFC 5280 section 4.1.2.2), wider than any
/// machine integer, so the base change is done over a big integer.
pub fn serial_integer(serial: &str) -> Option<String> {
    let digits = normalized_serial(serial)?;
    Some(
        ::num_bigint::BigUint::parse_bytes(digits.as_bytes(), 16)?
            .to_str_radix(10),
    )
}

/// The character that separates the octets of a serial number rather than spelling its
/// value: the colon `openssl x509 -text` prints between them, which is the listing
/// `RevokeCertificateRequest$CertificateSerial`'s docs have the serial copied out of.
///
/// Nothing else is accepted as a separator: a serial carrying anything but hexadecimal
/// digits and these is not "in hexadecimal format" and names no certificate.
const SERIAL_SEPARATORS: &[char] = &[':'];

/// Whether `certificate`'s serial number is the value `wanted` holds, which is already
/// normalised ([`normalized_serial`]).
fn certificate_has_serial(certificate: &IssuedCertificate, wanted: &str) -> bool {
    normalized_serial(&certificate.serial).as_deref() == Some(wanted)
}

/// Every private CA whose owner account `owned_by` accepts, in the order a listing
/// reports them.
///
/// The order is by ARN: a stable TOTAL order over the set, since a CA's ARN is unique.
/// That is what makes a cursor cut into the listing exact — a page resumes at the first
/// CA at or after the cursor's ARN, so no CA is reported twice or skipped even when CAs
/// are created between two pages. Amazon Web Services Private CA documents no order for
/// this listing, so any stable total order is faithful; an unstable one (a hash map's
/// iteration order) is not.
pub fn certificate_authorities_where(
    state: &ComAmazonawsAcmpcaAcmprivatecaState,
    owned_by: impl Fn(&str) -> bool,
) -> Vec<CertificateAuthority> {
    let mut records: Vec<&CertificateAuthorityRecord> = state
        .certificate_authorities
        .values()
        .filter(|record| owned_by(&record.owner_account))
        .collect();
    records.sort_by(|left, right| left.arn.cmp(&right.arn));
    records.into_iter().map(|record| record.to_wire()).collect()
}

/// The region this request was signed for, which is the region of every ARN it mints.
pub fn request_region(headers: &::http::HeaderMap) -> String {
    ::emulator_server::sigv4_credential_scope(headers)
        .map(|scope| scope.region)
        .filter(|region| !region.is_empty())
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

// ---------------------------------------------------------------------------
// Minted identifiers
// ---------------------------------------------------------------------------

/// Distinguishes two mints that land in the same nanosecond, and orders idempotency
/// records for eviction.
static MINT_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// The next value of the process-wide sequence.
fn next_sequence() -> u64 {
    MINT_SEQUENCE.fetch_add(1, Ordering::Relaxed)
}

/// A fresh RFC 4122 version-4-shaped id, the form Private CA gives a CA resource
/// (`...:certificate-authority/12345678-1234-1234-1234-123456789012`).
pub fn mint_resource_id() -> String {
    let digest = mint_digest("certificate-authority");
    let hex = ::hex::encode(&digest[..16]);
    // Version nibble 4 and a variant nibble from `8..=b`, so the id is shaped like the
    // random uuid AWS mints rather than merely being 32 hex digits.
    let variant = match digest[8] & 0x03 {
        0 => '8',
        1 => '9',
        2 => 'a',
        _ => 'b',
    };
    format!(
        "{}-{}-4{}-{variant}{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[13..16],
        &hex[17..20],
        &hex[20..32]
    )
}

/// A serial number for one of a CA's certificates, minted from `seed`.
///
/// Private CA names an issued certificate by the hexadecimal spelling of its serial — that is
/// the form its certificate ARNs end in — so this is 16 bytes of hex digits.
///
/// Never a leading ZERO byte: a certificate's serial is an INTEGER, so a leading zero is not
/// part of its value, and a client reading the serial off the certificate would read one
/// octet fewer than were minted here — leaving the renderings of the same serial (this one,
/// the certificate's own, the one an audit report row states) unable to be joined.
pub fn mint_serial(seed: &str) -> String {
    let mut bytes = [0u8; SERIAL_BYTES];
    bytes.copy_from_slice(&mint_digest(seed)[..SERIAL_BYTES]);
    if let Some(first) = bytes.first_mut() {
        if *first == 0 {
            *first = 1;
        }
    }
    ::hex::encode(bytes)
}

/// How many bytes a minted serial carries.
const SERIAL_BYTES: usize = 16;

/// A digest over `seed`, the current instant and a process-wide sequence, so two mints
/// with the same seed never collide.
fn mint_digest(seed: &str) -> [u8; 32] {
    let sequence = next_sequence();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_nanos());
    let digest = Sha256::digest(format!("{seed}|{sequence}|{nanos}").as_bytes());
    let mut bytes = [0u8; 32];
    bytes.copy_from_slice(&digest);
    bytes
}

// ---------------------------------------------------------------------------
// PEM bodies
// ---------------------------------------------------------------------------

/// PEM label of a PKCS#10 certification request (RFC 7468 section 7).
pub const PEM_LABEL_CSR: &str = "CERTIFICATE REQUEST";
/// PEM label of an X.509 certificate (RFC 7468 section 5).
pub const PEM_LABEL_CERTIFICATE: &str = "CERTIFICATE";

/// Render `body` as a PEM block of `label`: base64 (the audited
/// `aws_smithy_types::base64` encoder, the one the wire serializer uses) in lines of 64
/// characters with LF endings, as RFC 7468 section 2 specifies.
pub fn encode_pem(label: &str, body: &[u8]) -> String {
    let encoded = ::aws_smithy_types::base64::encode(body);
    let mut out = format!("-----BEGIN {label}-----\n");
    for line in encoded.as_bytes().chunks(64) {
        // base64 is ASCII, so every chunk is valid UTF-8.
        out.push_str(&String::from_utf8_lossy(line));
        out.push('\n');
    }
    out.push_str(&format!("-----END {label}-----\n"));
    out
}

/// The single PEM block `label` frames, when `text` is exactly one well-formed non-empty
/// block of that label. `None` for anything else — the caller refuses that as malformed.
pub fn parse_pem_block(text: &str, label: &str) -> Option<Vec<u8>> {
    match parse_pem_blocks(text, label)?.as_slice() {
        [one] => Some(one.clone()),
        _ => None,
    }
}

/// The body of every PEM block of `label` in `text`, or `None` if any of it is not a
/// well-formed non-empty block of that label.
pub fn parse_pem_blocks(text: &str, label: &str) -> Option<Vec<Vec<u8>>> {
    let blocks = parse_pem_blocks_as(text, label)?;
    (!blocks.is_empty()).then_some(blocks)
}

/// The contents of every PEM block in `text`, requiring the text to be nothing but
/// well-formed non-empty blocks of `label`.
///
/// Only the framing is read here — the body is decoded by the audited
/// `aws_smithy_types::base64` decoder, and the walk is bounded by the input, which the
/// model already caps (`CertificateBodyBlob` 32 KiB, `CertificateChainBlob` 2 MiB).
fn parse_pem_blocks_as(text: &str, label: &str) -> Option<Vec<Vec<u8>>> {
    let begin = format!("-----BEGIN {label}-----");
    let end = format!("-----END {label}-----");
    let mut bodies: Vec<Vec<u8>> = Vec::new();
    let mut rest = text.trim();
    while !rest.is_empty() {
        let (body, remainder) = rest
            .strip_prefix(begin.as_str())?
            .split_once(end.as_str())?;
        // Line endings inside the block are not part of the base64 (RFC 7468), so CRLF
        // and LF framing decode alike.
        let base64: String = body.split_whitespace().collect();
        if base64.is_empty() {
            return None;
        }
        bodies.push(::aws_smithy_types::base64::decode(&base64).ok()?);
        rest = remainder.trim_start();
    }
    Some(bodies)
}

// ---------------------------------------------------------------------------
// X.509 / PKCS#10 objects
// ---------------------------------------------------------------------------

/// Object identifiers this service names in the DER it reads and writes.
///
/// A hand-written catalog of specification constants — no crate publishes the ML-DSA and
/// SM2 arcs this model's algorithm enums reach — each with its source:
///
/// * the X.500 attribute types a distinguished name carries: RFC 4519 section 2
///   (`2.5.4.*`), whose short names are the ones RFC 4514 section 3 prints;
/// * RSA keys and signatures: RFC 8017 appendix C (`1.2.840.113549.1.1.*`);
/// * EC public keys, the named curves and the ECDSA signatures: RFC 5480 sections 2.1.1
///   and 2.1.1.1 and RFC 5758 section 3.2 (`1.2.840.10045.*`, `1.3.132.0.*`);
/// * ML-DSA: the NIST CSOR arc for FIPS 204 (`2.16.840.1.101.3.4.3.17` ..
///   `2.16.840.1.101.3.4.3.19`);
/// * SM2 keys and the SM3-with-SM2 signature: GM/T 0006 (`1.2.156.10197.1.*`);
/// * the certificate and CRL extensions a published CRL names: RFC 5280 sections 4.2.1.2,
///   5.2.1, 5.2.3 and 5.3.1 (`2.5.29.*`).
mod oid {
    use super::ObjectIdentifier;

    // RFC 4519 section 2 attribute types, in the order `distinguished_name` prints them.
    pub const COMMON_NAME: ObjectIdentifier = ObjectIdentifier::new_unwrap("2.5.4.3");
    pub const SERIAL_NUMBER: ObjectIdentifier = ObjectIdentifier::new_unwrap("2.5.4.5");
    pub const ORGANIZATIONAL_UNIT: ObjectIdentifier = ObjectIdentifier::new_unwrap("2.5.4.11");
    pub const ORGANIZATION: ObjectIdentifier = ObjectIdentifier::new_unwrap("2.5.4.10");
    pub const LOCALITY: ObjectIdentifier = ObjectIdentifier::new_unwrap("2.5.4.7");
    pub const STATE_OR_PROVINCE: ObjectIdentifier = ObjectIdentifier::new_unwrap("2.5.4.8");
    pub const COUNTRY: ObjectIdentifier = ObjectIdentifier::new_unwrap("2.5.4.6");
    pub const TITLE: ObjectIdentifier = ObjectIdentifier::new_unwrap("2.5.4.12");
    pub const SURNAME: ObjectIdentifier = ObjectIdentifier::new_unwrap("2.5.4.4");
    pub const GIVEN_NAME: ObjectIdentifier = ObjectIdentifier::new_unwrap("2.5.4.42");
    pub const INITIALS: ObjectIdentifier = ObjectIdentifier::new_unwrap("2.5.4.43");
    pub const PSEUDONYM: ObjectIdentifier = ObjectIdentifier::new_unwrap("2.5.4.65");
    pub const GENERATION_QUALIFIER: ObjectIdentifier = ObjectIdentifier::new_unwrap("2.5.4.44");
    pub const DN_QUALIFIER: ObjectIdentifier = ObjectIdentifier::new_unwrap("2.5.4.46");

    // Public key algorithms.
    pub const RSA_ENCRYPTION: ObjectIdentifier =
        ObjectIdentifier::new_unwrap("1.2.840.113549.1.1.1");
    pub const EC_PUBLIC_KEY: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.10045.2.1");
    pub const PRIME256V1: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.10045.3.1.7");
    pub const SECP384R1: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.3.132.0.34");
    pub const SECP521R1: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.3.132.0.35");
    pub const SM2: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.156.10197.1.301");

    // Signature algorithms.
    pub const SHA256_WITH_RSA: ObjectIdentifier =
        ObjectIdentifier::new_unwrap("1.2.840.113549.1.1.11");
    pub const SHA384_WITH_RSA: ObjectIdentifier =
        ObjectIdentifier::new_unwrap("1.2.840.113549.1.1.12");
    pub const SHA512_WITH_RSA: ObjectIdentifier =
        ObjectIdentifier::new_unwrap("1.2.840.113549.1.1.13");
    pub const RSASSA_PSS: ObjectIdentifier =
        ObjectIdentifier::new_unwrap("1.2.840.113549.1.1.10");
    pub const ECDSA_WITH_SHA256: ObjectIdentifier =
        ObjectIdentifier::new_unwrap("1.2.840.10045.4.3.2");
    pub const ECDSA_WITH_SHA384: ObjectIdentifier =
        ObjectIdentifier::new_unwrap("1.2.840.10045.4.3.3");
    pub const ECDSA_WITH_SHA512: ObjectIdentifier =
        ObjectIdentifier::new_unwrap("1.2.840.10045.4.3.4");
    pub const SM3_WITH_SM2: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.156.10197.1.501");

    // The key purposes `extendedKeyUsage` names, from the `ExtendedKeyUsageType` members
    // this model publishes: the `id-kp-*` purposes of RFC 5280 section 4.2.1.12, the
    // smart-card-logon and document-signing purposes of the Microsoft `1.3.6.1.4.1.311` arc,
    // and the certificate-transparency purpose of RFC 6962 section 3.1.
    pub const TLS_SERVER_AUTH: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.3.6.1.5.5.7.3.1");
    pub const TLS_CLIENT_AUTH: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.3.6.1.5.5.7.3.2");
    pub const CODE_SIGNING: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.3.6.1.5.5.7.3.3");
    pub const EMAIL_PROTECTION: ObjectIdentifier =
        ObjectIdentifier::new_unwrap("1.3.6.1.5.5.7.3.4");
    pub const TIME_STAMPING: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.3.6.1.5.5.7.3.8");
    pub const OCSP_SIGNING: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.3.6.1.5.5.7.3.9");
    pub const SMART_CARD_LOGIN: ObjectIdentifier =
        ObjectIdentifier::new_unwrap("1.3.6.1.4.1.311.20.2.2");
    pub const DOCUMENT_SIGNING: ObjectIdentifier =
        ObjectIdentifier::new_unwrap("1.3.6.1.4.1.311.3.10.3.12");
    pub const CERTIFICATE_TRANSPARENCY: ObjectIdentifier =
        ObjectIdentifier::new_unwrap("1.3.6.1.4.1.11129.2.4.4");

    // The access method an `authorityInformationAccess` description names for an OCSP
    // responder (`id-ad-ocsp`, RFC 5280 section 4.2.2.1) and the qualifier a certificate
    // policy names for a certification practice statement (`id-qt-cps`, section 4.2.1.4).
    pub const OCSP_ACCESS_METHOD: ObjectIdentifier =
        ObjectIdentifier::new_unwrap("1.3.6.1.5.5.7.48.1");
    pub const CPS_QUALIFIER: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.3.6.1.5.5.7.2.1");

    // Certificate and CRL extensions: RFC 5280 sections 4.2.1, 5.2 and 5.3 (`2.5.29.*`).
    pub const SUBJECT_KEY_IDENTIFIER: ObjectIdentifier = ObjectIdentifier::new_unwrap("2.5.29.14");
    pub const SUBJECT_ALT_NAME: ObjectIdentifier = ObjectIdentifier::new_unwrap("2.5.29.17");
    pub const CRL_NUMBER: ObjectIdentifier = ObjectIdentifier::new_unwrap("2.5.29.20");
    pub const CRL_REASON: ObjectIdentifier = ObjectIdentifier::new_unwrap("2.5.29.21");
    pub const AUTHORITY_KEY_IDENTIFIER: ObjectIdentifier =
        ObjectIdentifier::new_unwrap("2.5.29.35");

    // The attribute a PKCS#10 request states the extensions it asks for in: `pkcs-9-at-
    // extensionRequest` (RFC 2985 section 5.4.2).
    pub const EXTENSION_REQUEST: ObjectIdentifier =
        ObjectIdentifier::new_unwrap("1.2.840.113549.1.9.14");

    // ML-DSA names one algorithm for the key and the signature alike.
    pub const ML_DSA_44: ObjectIdentifier =
        ObjectIdentifier::new_unwrap("2.16.840.1.101.3.4.3.17");
    pub const ML_DSA_65: ObjectIdentifier =
        ObjectIdentifier::new_unwrap("2.16.840.1.101.3.4.3.18");
    pub const ML_DSA_87: ObjectIdentifier =
        ObjectIdentifier::new_unwrap("2.16.840.1.101.3.4.3.19");
}

/// The X.509 certificate `der` carries (RFC 5280 section 4.1), or `None` when it is not one
/// — what `MalformedCertificateException` answers.
pub fn parse_x509_certificate(der: &[u8]) -> Option<Certificate> {
    Certificate::from_der(der).ok()
}

/// `certificate` as the PEM block this service hands a certificate out in, which is the
/// canonical framing of the DER it decoded from ([`encode_pem`]).
pub fn encode_certificate_pem(certificate: &Certificate) -> Option<String> {
    Some(encode_pem(
        PEM_LABEL_CERTIFICATE,
        &certificate.to_der().ok()?,
    ))
}

/// The PKCS#10 certification request `der` carries (RFC 2986 section 4), or `None` when
/// it is not one — what `MalformedCSRException` answers.
pub fn parse_certification_request(der: &[u8]) -> Option<CertificationRequest> {
    CertReq::from_der(der).ok()
}

/// The validity window `certificate` carries (RFC 5280 section 4.1.2.5), as the two
/// instants it denotes.
///
/// This is the window a CA activated with that certificate reports as its own
/// `NotBefore`/`NotAfter`, and whose end bounds every certificate the CA then issues
/// ([`VALIDITY_EXCEEDS_CA_MESSAGE`]): both are properties of the certificate, so both are
/// read out of it rather than recorded from the instant it was taken.
pub fn certificate_validity_window(certificate: &Certificate) -> (SystemTime, SystemTime) {
    (
        certificate.tbs_certificate.validity.not_before.to_system_time(),
        certificate.tbs_certificate.validity.not_after.to_system_time(),
    )
}

/// The certification request this CA hands out, derived once at create time.
///
/// A real PKCS#10 `CertificationRequest` (RFC 2986 section 4) over the key pair the CA was
/// minted with: its subject is the CA's own configured subject, its `SubjectPublicKeyInfo`
/// is that key pair's public key, the extensions it asks for are the ones its configured
/// `CsrExtensions` names ([`requested_extension_attributes`]), and its signature is made
/// with the key pair's private half under the `SigningAlgorithm` the CA was created with —
/// so a signing authority can read the key out of it, verify it, and sign a certificate for
/// it, and the certificate this service issues for it (below) carries the same key.
///
/// For a `KeyAlgorithm` the in-process provider cannot mint a key pair for
/// ([`generate_key_pair`]) the CA holds none, and the request falls back to the stand-in
/// public key and signature of the right ASN.1 shape, each the SHA-256 digest of the
/// content it belongs to, so the same CA still always answers with the same request.
///
/// `None` when the configured subject cannot be encoded as an X.500 name.
pub fn derive_csr_pem(
    arn: &str,
    configuration: &CertificateAuthorityConfiguration,
    key_pair: Option<&CaKeyPair>,
) -> Option<String> {
    let info = CertReqInfo {
        version: x509_cert::request::Version::V1,
        subject: subject_name(&configuration.subject)?,
        public_key: match key_pair {
            Some(key_pair) => key_pair.public_key_info()?,
            None => stand_in_public_key_info(&configuration.key_algorithm, arn)?,
        },
        // The extensions the request ASKS FOR are the ones the CA's `CsrExtensions`
        // configures ([`requested_extension_attributes`]).
        attributes: requested_extension_attributes(configuration.csr_extensions.as_ref())
            .unwrap_or_default(),
    };
    let algorithm = signature_algorithm_identifier(&configuration.signing_algorithm)?;
    let signature = object_signature(
        key_pair,
        &configuration.signing_algorithm,
        &info.to_der().ok()?,
    )?;
    let request = CertReq {
        info,
        algorithm,
        signature,
    };
    Some(encode_pem(PEM_LABEL_CSR, &request.to_der().ok()?))
}

/// The certificate this CA issues for a certification request, built from what the CA
/// persisted and from the request it answers.
///
/// A real X.509 `Certificate` (RFC 5280 section 4.1): the serial the CA allocated, the
/// issuer the CA's own subject, the resolved validity window, and — as a CA does — the
/// public key copied out of the certification request being signed.
///
/// The signature is made with the CA's own private key ([`CaKeyPair::signature`]) under the
/// algorithm the request asked for, so the certificate verifies against the public key the
/// CA publishes; for a CA holding no key pair it is the stand-in of the right ASN.1 shape.
///
/// The subject and the extensions come from the three sources an issuance has, in the order
/// of priority `issued_extensions` states: the `TemplateArn` the request named defines the
/// certificate's kind ([`certificate_template`]), an `ApiPassthrough` that applies supplies
/// the values the caller asked to be carried, and the certification request supplies the
/// rest — with the issuing CA's own key identifiers and revocation endpoints beside them.
///
/// `None` when the CA's subject, the allocated serial or the validity window cannot be
/// encoded as the certificate's DER demands.
pub fn derive_certificate_pem(
    ca: &CertificateAuthorityRecord,
    serial: &str,
    content: &IssuedCertificateContent<'_>,
) -> Option<String> {
    let request = content.request;
    let algorithm = signature_algorithm_identifier(content.signing_algorithm)?;
    let tbs_certificate = TbsCertificate {
        version: Version::V3,
        serial_number: serial_number(serial)?,
        signature: algorithm.clone(),
        issuer: subject_name(&ca.configuration.subject)?,
        validity: CertificateValidity {
            not_before: certificate_time(content.not_before)?,
            not_after: certificate_time(content.not_after)?,
        },
        subject: issued_certificate_subject(request, content.passthrough)?,
        subject_public_key_info: request.public_key.clone(),
        issuer_unique_id: None,
        subject_unique_id: None,
        extensions: issued_certificate_extensions(
            ca,
            content.template,
            request,
            content.passthrough,
            &request.public_key,
        ),
    };
    let signature = object_signature(
        ca.key_pair.as_ref(),
        content.signing_algorithm,
        &tbs_certificate.to_der().ok()?,
    )?;
    let certificate = Certificate {
        tbs_certificate,
        signature_algorithm: algorithm,
        signature,
    };
    Some(encode_pem(PEM_LABEL_CERTIFICATE, &certificate.to_der().ok()?))
}

/// The certificate serial `serial` names, as an X.509 `CertificateSerialNumber`.
///
/// The CA allocates its serials as hex ([`mint_serial`]), which is how they appear in a
/// certificate ARN; the DER integer the certificate carries is that same number, so the
/// two always agree. `SerialNumber` keeps it the positive integer RFC 5280 section
/// 4.1.2.2 requires by prepending a zero byte when the leading bit is set, which changes
/// no value.
fn serial_number(serial: &str) -> Option<SerialNumber> {
    SerialNumber::new(&::hex::decode(serial).ok()?).ok()
}

/// `time` as the certificate validity field RFC 5280 section 4.1.2.5 calls for: UTCTime
/// through 2049 and GeneralizedTime from 2050, which is what `Time::try_from` picks.
fn certificate_time(time: SystemTime) -> Option<Time> {
    Time::try_from(time).ok()
}

/// The `AlgorithmIdentifier` of a modelled `SigningAlgorithm`, or `None` for a value the
/// enum does not declare.
///
/// The three `*_RSA_PSS` members share the RSASSA-PSS identifier and this emitter leaves
/// its `RSASSA-PSS-params` out, so they are not told apart in the DER: the parameters
/// describe the digest and salt of a signature, and the emulator makes no signature to
/// describe (see [`stand_in_signature`]). The member the caller asked for is persisted on
/// the issued certificate either way.
fn signature_algorithm_identifier(signing_algorithm: &str) -> Option<AlgorithmIdentifierOwned> {
    let (algorithm, parameters) = match signing_algorithm {
        // RSASSA-PKCS1-v1_5 takes a NULL parameter (RFC 8017 appendix C).
        "SHA256WITHRSA" => (oid::SHA256_WITH_RSA, Some(null_parameters()?)),
        "SHA384WITHRSA" => (oid::SHA384_WITH_RSA, Some(null_parameters()?)),
        "SHA512WITHRSA" => (oid::SHA512_WITH_RSA, Some(null_parameters()?)),
        "SHA256WITHRSA_PSS" | "SHA384WITHRSA_PSS" | "SHA512WITHRSA_PSS" => {
            (oid::RSASSA_PSS, None)
        }
        // ECDSA and ML-DSA identifiers take no parameters (RFC 5758 section 3.2).
        "SHA256WITHECDSA" => (oid::ECDSA_WITH_SHA256, None),
        "SHA384WITHECDSA" => (oid::ECDSA_WITH_SHA384, None),
        "SHA512WITHECDSA" => (oid::ECDSA_WITH_SHA512, None),
        "SM3WITHSM2" => (oid::SM3_WITH_SM2, None),
        "ML_DSA_44" => (oid::ML_DSA_44, None),
        "ML_DSA_65" => (oid::ML_DSA_65, None),
        "ML_DSA_87" => (oid::ML_DSA_87, None),
        _ => return None,
    };
    Some(AlgorithmIdentifierOwned {
        oid: algorithm,
        parameters,
    })
}

/// The `SubjectPublicKeyInfo` of a CA whose `key_algorithm` the in-process provider cannot
/// mint a key pair for.
///
/// The algorithm identifier is the real one for that member of `KeyAlgorithm`; the key
/// bits are a stand-in derived from `seed` (see [`derive_csr_pem`]), because there is no
/// key. `None` for a value `KeyAlgorithm` does not declare.
fn stand_in_public_key_info(
    key_algorithm: &str,
    seed: &str,
) -> Option<SubjectPublicKeyInfoOwned> {
    let (algorithm, parameters) = match key_algorithm {
        "RSA_2048" | "RSA_3072" | "RSA_4096" => (oid::RSA_ENCRYPTION, Some(null_parameters()?)),
        "EC_prime256v1" => (oid::EC_PUBLIC_KEY, Some(curve_parameters(oid::PRIME256V1)?)),
        "EC_secp384r1" => (oid::EC_PUBLIC_KEY, Some(curve_parameters(oid::SECP384R1)?)),
        "EC_secp521r1" => (oid::EC_PUBLIC_KEY, Some(curve_parameters(oid::SECP521R1)?)),
        "SM2" => (oid::EC_PUBLIC_KEY, Some(curve_parameters(oid::SM2)?)),
        "ML_DSA_44" => (oid::ML_DSA_44, None),
        "ML_DSA_65" => (oid::ML_DSA_65, None),
        "ML_DSA_87" => (oid::ML_DSA_87, None),
        _ => return None,
    };
    Some(SubjectPublicKeyInfoOwned {
        algorithm: AlgorithmIdentifierOwned {
            oid: algorithm,
            parameters,
        },
        subject_public_key: BitString::from_bytes(&Sha256::digest(seed.as_bytes())).ok()?,
    })
}

/// The named-curve `parameters` of an EC public key (RFC 5480 section 2.1.1).
fn curve_parameters(curve: ObjectIdentifier) -> Option<Any> {
    Any::new(DerTag::ObjectIdentifier, curve.as_bytes()).ok()
}

/// The explicit NULL an RSA algorithm identifier carries as its parameters.
fn null_parameters() -> Option<Any> {
    Any::new(DerTag::Null, Vec::new()).ok()
}

/// The signature bits of an object this service writes: the one `key_pair` makes over
/// `signed` under `signing_algorithm`, or the stand-in below when the CA holds no key pair
/// that algorithm can be made with.
fn object_signature(
    key_pair: Option<&CaKeyPair>,
    signing_algorithm: &str,
    signed: &[u8],
) -> Option<BitString> {
    match key_pair.and_then(|key_pair| key_pair.signature(signing_algorithm, signed)) {
        Some(signature) => BitString::from_bytes(&signature).ok(),
        None => stand_in_signature(signed),
    }
}

/// The stand-in signature bits of an object a CA holding no key pair writes: the SHA-256
/// digest of the DER the signature covers, which is of the right ASN.1 shape and is what a
/// CA whose `KeyAlgorithm` the in-process provider cannot mint signs with. Nothing in the
/// emulator verifies it.
fn stand_in_signature(signed: &[u8]) -> Option<BitString> {
    BitString::from_bytes(&Sha256::digest(signed)).ok()
}

/// The `SigningAlgorithm` members usable with `key_algorithm`, in the order the model
/// declares them: the ones whose algorithm family is the family of that key.
pub fn signing_algorithms_for_key(key_algorithm: &str) -> Vec<&'static str> {
    let Some(family) = algorithm_family(key_algorithm) else {
        return Vec::new();
    };
    SIGNING_ALGORITHMS_IN_MODEL_ORDER
        .iter()
        .copied()
        .filter(|signing| algorithm_family(signing) == Some(family))
        .collect()
}

/// The subject's relative distinguished names, most specific first: the X.500 attribute
/// type, the short name RFC 4514 section 3 prints for it, and the value.
///
/// The one ordered reading of an `ASN1Subject`, shared by the RFC 4514 string below and
/// by the DER `Name` the certificates carry, so the two can never disagree. An attribute
/// the request left out or left empty names nothing and is not a relative distinguished
/// name.
fn subject_attributes(subject: &ASN1Subject) -> Vec<(ObjectIdentifier, &'static str, &str)> {
    [
        (oid::COMMON_NAME, "CN", &subject.common_name),
        (oid::SERIAL_NUMBER, "SERIALNUMBER", &subject.serial_number),
        (
            oid::ORGANIZATIONAL_UNIT,
            "OU",
            &subject.organizational_unit,
        ),
        (oid::ORGANIZATION, "O", &subject.organization),
        (oid::LOCALITY, "L", &subject.locality),
        (oid::STATE_OR_PROVINCE, "ST", &subject.state),
        (oid::COUNTRY, "C", &subject.country),
        (oid::TITLE, "TITLE", &subject.title),
        (oid::SURNAME, "SN", &subject.surname),
        (oid::GIVEN_NAME, "GIVENNAME", &subject.given_name),
        (oid::INITIALS, "INITIALS", &subject.initials),
        (oid::PSEUDONYM, "PSEUDONYM", &subject.pseudonym),
        (
            oid::GENERATION_QUALIFIER,
            "GENERATIONQUALIFIER",
            &subject.generation_qualifier,
        ),
        (
            oid::DN_QUALIFIER,
            "DNQ",
            &subject.distinguished_name_qualifier,
        ),
    ]
    .into_iter()
    .filter_map(|(attribute, key, value)| {
        value
            .as_deref()
            .filter(|value| !value.is_empty())
            .map(|value| (attribute, key, value))
    })
    .collect()
}

/// The subject's X.500 distinguished name, most specific relative distinguished name
/// first, as RFC 4514 renders one.
pub fn distinguished_name(subject: &ASN1Subject) -> String {
    let mut rdns: Vec<String> = subject_attributes(subject)
        .into_iter()
        .map(|(_, key, value)| format!("{key}={value}"))
        .collect();
    for attribute in subject.custom_attributes.iter().flatten() {
        rdns.push(format!(
            "{}={}",
            attribute.object_identifier, attribute.value
        ));
    }
    rdns.join(",")
}

/// The subject as the DER `Name` a certificate or certification request carries: one
/// relative distinguished name per attribute the subject supplies, in the order
/// [`subject_attributes`] reads them, each a `SET OF` one `AttributeTypeAndValue` whose
/// value is a UTF8String (RFC 5280 section 4.1.2.4).
///
/// `None` when an attribute cannot be encoded — a `CustomAttribute` whose
/// `ObjectIdentifier` is not an object identifier, or a value the DER string type refuses.
fn subject_name(subject: &ASN1Subject) -> Option<Name> {
    let mut rdns: Vec<RelativeDistinguishedName> = Vec::new();
    for (attribute, _, value) in subject_attributes(subject) {
        rdns.push(relative_distinguished_name(attribute, value)?);
    }
    for attribute in subject.custom_attributes.iter().flatten() {
        let custom = ObjectIdentifier::new(&attribute.object_identifier).ok()?;
        rdns.push(relative_distinguished_name(custom, &attribute.value)?);
    }
    Some(Name::from(RdnSequence(rdns)))
}

/// One relative distinguished name: the `SET OF` holding the single attribute type and
/// its UTF8String value.
fn relative_distinguished_name(
    attribute: ObjectIdentifier,
    value: &str,
) -> Option<RelativeDistinguishedName> {
    let pair = AttributeTypeAndValue {
        oid: attribute,
        value: Any::new(DerTag::Utf8String, value.as_bytes()).ok()?,
    };
    Some(RelativeDistinguishedName::from(
        SetOfVec::try_from(vec![pair]).ok()?,
    ))
}

/// Whether `subject` carries at least one relative distinguished name.
pub fn subject_is_empty(subject: &ASN1Subject) -> bool {
    distinguished_name(subject).is_empty()
}

/// The key family a `KeyAlgorithm` or a `SigningAlgorithm` member belongs to, or `None`
/// for a value neither enum declares.
///
/// Derived from the two value lists in `ops::types` (`KEY_ALGORITHM_VALUES` and
/// `SIGNING_ALGORITHM_VALUES`), so a member added to the model keeps its family: every
/// RSA key is `RSA_<bits>` and every RSA signature names `WITHRSA`, every EC key is
/// `EC_<curve>` and every ECDSA signature names `WITHECDSA`, the lattice members share
/// the `ML_DSA_` prefix, and SM2 keys pair with the single SM3WITHSM2 signature.
pub fn algorithm_family(algorithm: &str) -> Option<&'static str> {
    if algorithm.starts_with("ML_DSA_") {
        Some("ML_DSA")
    } else if algorithm.starts_with("RSA_") || algorithm.contains("WITHRSA") {
        Some("RSA")
    } else if algorithm.starts_with("EC_") || algorithm.contains("WITHECDSA") {
        Some("EC")
    } else if algorithm == "SM2" || algorithm == "SM3WITHSM2" {
        Some("SM2")
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Time
// ---------------------------------------------------------------------------

/// `SystemTime` as the modelled `TStamp`.
pub fn to_date_time(time: SystemTime) -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from_millis(
        time.duration_since(UNIX_EPOCH)
            .ok()
            .and_then(|since| i64::try_from(since.as_millis()).ok())
            .unwrap_or(0),
    )
}

/// Whole seconds between the Unix epoch and `time`.
pub fn epoch_seconds(time: SystemTime) -> i64 {
    match time.duration_since(UNIX_EPOCH) {
        Ok(since) => i64::try_from(since.as_secs()).unwrap_or(i64::MAX),
        Err(before) => -i64::try_from(before.duration().as_secs()).unwrap_or(i64::MAX),
    }
}

/// The instant `seconds` after the Unix epoch, or `None` when the platform's clock
/// cannot represent it. Fallible because the value can come straight off the wire.
pub fn from_epoch_seconds(seconds: i64) -> Option<SystemTime> {
    let offset = Duration::from_secs(seconds.unsigned_abs());
    if seconds >= 0 {
        UNIX_EPOCH.checked_add(offset)
    } else {
        UNIX_EPOCH.checked_sub(offset)
    }
}

/// `time` rendered as RFC 3339 by `aws-smithy-types` — the same `DateTime` the wire
/// serializer uses, so there is no second date implementation here. `None` when the
/// instant is not a date that type can express.
fn rfc3339(seconds: i64) -> Option<String> {
    ::aws_smithy_types::DateTime::from_secs(seconds)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .ok()
}

/// The instant an RFC 3339 `YYYY-MM-DDTHH:MM:SSZ` string names, parsed by the same
/// `aws-smithy-types` date type: month lengths, leap years, the hour/minute/second
/// ranges and the range of representable dates are its rules, not restated here.
fn instant_from_rfc3339(text: &str) -> Option<SystemTime> {
    let parsed = ::aws_smithy_types::DateTime::from_str(
        text,
        ::aws_smithy_types::date_time::Format::DateTime,
    )
    .ok()?;
    from_epoch_seconds(parsed.secs())
}

/// The instant `seconds` after the Unix epoch, when it is also a date this service can
/// report. Rendering it IS the range check: the date type refuses a timestamp it cannot
/// express, which is what a caller-chosen `Validity.Value` of absurd magnitude becomes.
fn calendar_instant(seconds: i64) -> Option<SystemTime> {
    rfc3339(seconds)?;
    from_epoch_seconds(seconds)
}

/// `time` as RFC 3339 at second granularity.
pub fn format_timestamp(time: SystemTime) -> String {
    rfc3339(epoch_seconds(time)).unwrap_or_default()
}

/// `start` advanced by `months` calendar months, keeping the time of day and clamping the
/// day of month to the length of the target month (2026-01-31 + 1 month = 2026-02-28).
///
/// Only the month count is arithmetic here; every calendar rule (which days a month has,
/// leap years, the representable range) comes from the `aws-smithy-types` date type,
/// which refuses a date that does not exist. `None` when the result is not such a date —
/// what a caller-chosen `Validity.Value` of absurd magnitude resolves to.
pub fn add_months(start: SystemTime, months: i64) -> Option<SystemTime> {
    let rendered = rfc3339(epoch_seconds(start))?;
    let (date, clock) = rendered.split_once('T')?;
    let mut fields = date.split('-');
    let year: i64 = fields.next()?.parse().ok()?;
    let month: i64 = fields.next()?.parse().ok()?;
    let day: i64 = fields.next()?.parse().ok()?;
    let total = year
        .checked_mul(12)?
        .checked_add(month - 1)?
        .checked_add(months)?;
    let target_year = total.div_euclid(12);
    let target_month = total.rem_euclid(12) + 1;
    // RFC 3339 carries a four-digit year, so anything wider is not a date this service
    // can report.
    if !(1..=9999).contains(&target_year) {
        return None;
    }
    // A day the target month does not have fails to parse, so stepping back from the
    // starting day of month clamps to that month's last day.
    let mut target_day = day;
    loop {
        let candidate = format!("{target_year:04}-{target_month:02}-{target_day:02}T{clock}");
        if let Some(instant) = instant_from_rfc3339(&candidate) {
            return Some(instant);
        }
        // 28 is the shortest month, so a day above it is the only clampable case.
        if target_day > 28 {
            target_day -= 1;
        } else {
            return None;
        }
    }
}

// ---------------------------------------------------------------------------
// Validity
// ---------------------------------------------------------------------------

/// Resolve a modelled `Validity` against `reference` (the start of the validity period).
///
/// The five `ValidityPeriodType` members, exactly as the `Validity$Type` docs define
/// them: `DAYS`/`MONTHS`/`YEARS` are relative to issuance, `ABSOLUTE` is seconds since
/// the Unix epoch, and `END_DATE` is `YYMMDDHHMMSS` (UTCTime — a year field of 50 or
/// more means 19YY, below 50 means 20YY) or `YYYYMMDDHHMMSS` (GeneralizedTime).
///
/// Total for every `i64` the wire can carry: a value that resolves outside the
/// representable range of dates is refused rather than wrapped or panicked on.
///
/// `Err` carries the message of the `InvalidArgsException` the caller must answer with.
pub fn resolve_validity(reference: SystemTime, validity: &Validity) -> Result<SystemTime, String> {
    let value = validity.value;
    let period = validity.r#type.as_str();
    let out_of_range = || {
        format!(
            "Validity.Value {value} with Validity.Type {period} resolves outside the range of \
             dates this service represents"
        )
    };
    match period {
        "DAYS" => value
            .checked_mul(86_400)
            .and_then(|seconds| epoch_seconds(reference).checked_add(seconds))
            .and_then(calendar_instant)
            .ok_or_else(out_of_range),
        "MONTHS" => add_months(reference, value).ok_or_else(out_of_range),
        "YEARS" => value
            .checked_mul(12)
            .and_then(|months| add_months(reference, months))
            .ok_or_else(out_of_range),
        "ABSOLUTE" => calendar_instant(value).ok_or_else(out_of_range),
        "END_DATE" => parse_end_date(value),
        other => Err(format!(
            "Validity.Type '{other}' is not one of ABSOLUTE, DAYS, END_DATE, MONTHS, YEARS"
        )),
    }
}

/// An `END_DATE` validity value: UTCTime `YYMMDDHHMMSS` or GeneralizedTime
/// `YYYYMMDDHHMMSS`. The digits are rearranged into RFC 3339 and handed to the
/// `aws-smithy-types` date parser, so the calendar date and the time of day are
/// validated by that date type and no month-length or leap-year rule is restated here.
fn parse_end_date(value: i64) -> Result<SystemTime, String> {
    let malformed = || {
        format!(
            "Validity.Value {value} is not a UTCTime YYMMDDHHMMSS or GeneralizedTime \
             YYYYMMDDHHMMSS date and time, which Validity.Type END_DATE requires"
        )
    };
    if value <= 0 {
        return Err(malformed());
    }
    let digits = value.to_string();
    let (year, rest) = match digits.len() {
        12 => {
            let two = digits[0..2].parse::<i64>().map_err(|_| malformed())?;
            // UTCTime: "if the year field (YY) is greater than or equal to 50, the year
            // is interpreted as 19YY. If the year field is less than 50, the year is
            // interpreted as 20YY." (`Validity$Type` docs)
            (
                if two >= 50 { 1900 + two } else { 2000 + two },
                &digits[2..],
            )
        }
        14 => (
            digits[0..4].parse::<i64>().map_err(|_| malformed())?,
            &digits[4..],
        ),
        _ => return Err(malformed()),
    };
    // `rest` is exactly the ten digits MMDDHHMMSS, so every slice below is in bounds.
    let candidate = format!(
        "{year:04}-{}-{}T{}:{}:{}Z",
        &rest[0..2],
        &rest[2..4],
        &rest[4..6],
        &rest[6..8],
        &rest[8..10]
    );
    instant_from_rfc3339(&candidate).ok_or_else(malformed)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// How many serials the mint is driven for. A leading zero octet is one draw in 256, so
    /// this many mints draw one with near certainty while the bound stays a COUNT — nothing
    /// here depends on how fast the host runs.
    const MINTS: usize = 4_000;

    /// Every serial this service mints is the canonical spelling of a positive integer:
    /// whole octets of lower-case hexadecimal with no leading zero octet — so the number a
    /// certificate ARN names and an audit-report row states is the number the certificate's
    /// own DER serial carries, whichever draw the mint made.
    #[test]
    fn a_minted_serial_spells_the_number_its_certificate_carries() {
        for index in 0..MINTS {
            let seed = format!("certificate-authority/{index}|{index}");
            let minted = mint_serial(&seed);
            assert!(
                !minted.is_empty() && minted.len().is_multiple_of(2),
                "case {seed:?}: expected whole octets of hex, got {minted:?}"
            );
            assert!(
                minted
                    .chars()
                    .all(|digit| digit.is_ascii_hexdigit() && !digit.is_ascii_uppercase()),
                "case {seed:?}: expected lower-case hexadecimal digits, got {minted:?}"
            );
            assert!(
                !minted.starts_with("00"),
                "case {seed:?}: a positive integer's canonical spelling carries no leading \
                 zero octet, got {minted:?}"
            );
            // And the certificate's own serial IS that number: `SerialNumber` prepends the
            // zero octet RFC 5280 calls for only where the leading bit is set, which changes
            // no value, so the octets it carries past that are the ones minted.
            let number = serial_number(&minted).expect("a minted serial is a serial number");
            let carried: Vec<u8> = number
                .as_bytes()
                .iter()
                .copied()
                .skip_while(|octet| *octet == 0)
                .collect();
            let expected = ::hex::decode(&minted).expect("a minted serial is hex");
            assert_eq!(
                carried, expected,
                "case {seed:?}: expected the certificate to carry the minted octets, got {:?}",
                number.as_bytes()
            );
        }
    }
}
