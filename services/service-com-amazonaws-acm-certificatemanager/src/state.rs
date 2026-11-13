// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-acm-certificatemanager service.
//!
//! One `Certificate` record per ACM certificate, keyed by its ARN, plus the
//! idempotency index `RequestCertificate` consults. Every operation's response is
//! projected from these records — see [`certificate_detail`], [`certificate_summary`]
//! and [`certificate_search_result`] — so `DescribeCertificate`, `ListCertificates`
//! and `SearchCertificates` report what `RequestCertificate` stored rather than
//! echoing their own input. The record carries the certificate's own PEM material
//! beside those facts ([`Certificate::certificate_body`]), so `GetCertificate` hands
//! back the certificate the resource was stored from.
//!
//! What a record does NOT carry is which resources reference the certificate: those
//! resources belong to the services that own them, so the association is read from the
//! SHARED directory those services write
//! ([`certificate_association_directory`]) and is projected into `InUseBy` / `InUse` by
//! the readers rather than copied into a certificate's record.
//!
//! Beside them, one [`AcmeEndpointRecord`] per managed ACME endpoint, keyed by its
//! ARN, plus the idempotency index `CreateAcmeEndpoint` consults
//! ([`AcmeEndpointIdempotency`]). `DescribeAcmeEndpoint` projects that record through
//! [`acme_endpoint_detail`] and `ListAcmeEndpoints` through [`acme_endpoint_summary`],
//! and a freshly created endpoint is `CREATING` until the
//! transition the clock owes it falls due, exactly as a pending certificate is.
//!
//! Beside the endpoints, one [`AcmeAccountRecord`] per ACME account registered with
//! one of them, keyed by the (endpoint ARN, account URL) pair every ACME-account
//! operation addresses an account by. The records are WRITTEN by the endpoint's own ACME
//! data plane (`crate::service::acme_http`), where an ACME client registers with a JWS
//! `newAccount` request (RFC 8555 §7.3) — no modelled operation creates one — and they are
//! REPORTED by `DescribeAcmeAccount` through [`acme_account_detail`] and by
//! `ListAcmeAccounts` through [`acme_account_summary`]. `RevokeAcmeAccount` moves one to
//! [`ACME_ACCOUNT_STATUS_REVOKED`], which is what withdraws its authority on that data
//! plane, and deleting an endpoint drops the accounts registered with it
//! ([`ComAmazonawsAcmCertificatemanagerState::remove_acme_endpoint`]).
//!
//! Beside them, one [`AcmeDomainValidationRecord`] per domain validation an endpoint
//! holds, keyed by its own ARN — which is a PATH under the endpoint's ARN
//! ([`acme_domain_validation_arn`]), so one endpoint's validations are a contiguous
//! range ([`ComAmazonawsAcmCertificatemanagerState::acme_domain_validations_of`]) and
//! deleting the endpoint drops them with it. `DescribeAcmeDomainValidation` projects
//! that record through [`acme_domain_validation_detail`] and
//! `ListAcmeDomainValidations` through [`acme_domain_validation_summary`], including the
//! DNS record the service minted for it ([`acme_prevalidation_resource_record`]), and a freshly
//! created validation is `VALIDATING` until the transition the clock owes it falls due,
//! exactly as a freshly created endpoint is `CREATING`.
//!
//! Beside them, one [`AcmeExternalAccountBindingRecord`] per external account binding an
//! endpoint holds, keyed by its own ARN — which is, like a domain validation's, a PATH
//! under the endpoint's ARN ([`acme_external_account_binding_arn`]), so one endpoint's
//! bindings are a contiguous range
//! ([`ComAmazonawsAcmCertificatemanagerState::acme_external_account_bindings_of`]) and
//! deleting the endpoint drops them with it. `DescribeAcmeExternalAccountBinding`
//! projects that record through [`acme_external_account_binding_detail`] and
//! `ListAcmeExternalAccountBindings` through
//! [`acme_external_account_binding_summary`], and the credentials it carries — the key id an ACME client presents and the MAC key it signs
//! with ([`mint_acme_external_account_binding_mac_key`]) — are minted when the binding is
//! created and never rewritten, so every later read reports the same pair.
//! `GetAcmeExternalAccountBindingCredentials` is the operation that reports that pair —
//! it is no member of [`acme_external_account_binding_detail`] — and reports it only
//! while the binding's authority stands, since a revocation
//! ([`AcmeExternalAccountBindingRecord::is_revoked`]) withdraws the credentials it
//! records the instant of
//! ([`acme_external_account_binding_revoked_message`]).
//!
//! Every URL those two records carry is minted here, at the address the CALLER reached
//! this service on ([`acme_resource_url_at`]): the Region's ACME enrollment host
//! ([`ACME_ENROLLMENT_HOST_LABEL`]) with the endpoint's own id as the first path segment
//! ([`acme_endpoint_url`], [`acme_account_url`]) for a caller that resolves AWS's own
//! hostnames, and the emulator's own endpoint with the plane, the Region and that id in
//! the path ([`acme_local_endpoint_base_url`]) for one that reached this emulator
//! directly — and read back by [`parse_acme_request_path`], [`acme_enrollment_region`]
//! and [`parse_acme_local_path`], so the ACME HTTP surface `crate::service::acme_http`
//! serves resolves a published URL to the record behind it on either address.
//!
//! Beside the certificates sits the account-level singleton
//! [`AccountConfiguration`]: `PutAccountConfiguration` writes it and
//! `GetAccountConfiguration` reports it, so that round trip reads one record too.
//!
//! A requested certificate's record carries the outcome the clock owes it and the
//! instant it falls due ([`PendingValidation`]) — issuance for a request that needs no
//! proof of domain control, the up-front refusal of a name the public DNS delegates to
//! nobody ([`any_name_is_not_in_public_dns_in`]), and otherwise the timeout at the end of the
//! documented [`VALIDATION_WINDOW`], which is what a request waiting for evidence from
//! outside is owed. Beside it — for a certificate ACM validates by EMAIL — sits the
//! instant the initial validation mail it owes falls due
//! ([`Certificate::pending_validation_email`]), and — for one `RenewCertificate` has
//! been called for — the completion of the managed renewal it started ([`Renewal`]).
//! Nothing sleeps for those instants —
//! [`ComAmazonawsAcmCertificatemanagerState::materialize_due`] applies whatever was
//! already due, and every handler calls it before reading or writing the store.
//!
//! What a PUBLIC request waiting on proof of domain control is owed is not fixed for
//! good, though: the proof can arrive. A handler that reads or acts on a certificate
//! goes through [`ComAmazonawsAcmCertificatemanagerState::materialize_due_in`], which
//! first resolves each pending DNS validation against the account's hosted zones in the
//! shared engine (`dns_validation`) — a published CNAME is what takes such a request to
//! issuance.

mod dns_validation;

pub use dns_validation::any_name_is_not_in_public_dns_in;

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    AcmCertificateMetadata, AcmeAccount, AcmeAccountSummary, AcmeDomainValidation,
    AcmeDomainValidationSummary,
    AcmeEndpoint,
    AcmeEndpointSummary,
    AcmeExternalAccountBinding,
    AcmeExternalAccountBindingSummary,
    CertificateAuthority,
    CertificateDetail, CertificateMetadata, CertificateOptions, CertificateSearchResult,
    CertificateSummary, DistinguishedName, DnsPrevalidationDetails, DomainScope, DomainValidation,
    Expiration,
    ExpiryEventsConfiguration,
    ExtendedKeyUsage, FailureDetails, GeneralName, HttpRedirect, KeyUsage, PrevalidationDetails,
    RenewalSummary, ResourceRecord, Tag, X509Attributes,
};

/// This service's SigV4 signing name, and the `acm` segment of every ARN it mints.
pub const SIGNING_NAME: &str = "acm";

/// The region used when a request carries no SigV4 credential scope to read one from.
pub const DEFAULT_REGION: &str = "us-east-1";

/// `KeyAlgorithm` when `RequestCertificate` names none: "RSA is the default key
/// algorithm for ACM certificates" (`RequestCertificateRequest$KeyAlgorithm` doc),
/// and `RSA_2048` is the RSA size ACM issues.
pub const DEFAULT_KEY_ALGORITHM: &str = "RSA_2048";

/// `ValidationMethod=DNS`, the method whose evidence is a published DNS record: it is
/// the only one for which `DomainValidation` carries a `ResourceRecord`, the CNAME
/// "you add to your DNS database" (`ResourceRecord` doc).
pub const VALIDATION_METHOD_DNS: &str = "DNS";

/// `ValidationMethod=EMAIL`, the method whose evidence is a mailbox: it is the only
/// one for which `DomainValidation` carries `ValidationEmails` and
/// `ValidationDomain`, both documented as what "ACM used to send domain validation
/// emails".
pub const VALIDATION_METHOD_EMAIL: &str = "EMAIL";

/// `ValidationMethod` when `RequestCertificate` names none. ACM validates a public
/// certificate by email unless the request asks for DNS or HTTP validation.
pub const DEFAULT_VALIDATION_METHOD: &str = VALIDATION_METHOD_EMAIL;

/// `CertificateStatus`: "A certificate enters status PENDING_VALIDATION upon being
/// requested" (`CertificateDetail$Status` doc).
pub const STATUS_PENDING_VALIDATION: &str = "PENDING_VALIDATION";

/// `CertificateStatus`: "If validation succeeds, the certificate enters status
/// ISSUED" (`CertificateDetail$Status` doc).
pub const STATUS_ISSUED: &str = "ISSUED";

/// `CertificateStatus` of a request ACM could not complete: "A certificate enters
/// status PENDING_VALIDATION upon being requested, unless it fails for any of the
/// reasons given in the troubleshooting topic Certificate request fails"
/// (`CertificateDetail$Status` doc).
pub const STATUS_FAILED: &str = "FAILED";

/// `CertificateStatus` of a request whose domain validation never completed: "ACM
/// makes repeated attempts to validate a certificate for 72 hours and then times out"
/// (`CertificateDetail$Status` doc), which is where a request waiting on proof nobody
/// supplied ends up. `FailureReason` "exists only when the certificate status is
/// FAILED" (`CertificateDetail$FailureReason` doc), so a timed-out request carries
/// none.
pub const STATUS_VALIDATION_TIMED_OUT: &str = "VALIDATION_TIMED_OUT";

/// `CertificateStatus` of a certificate whose revocation ACM has recorded:
/// `RevokedAt` and `RevocationReason` "exist only when the certificate status is
/// REVOKED" (`CertificateDetail$RevokedAt` / `$RevocationReason` docs), so the two
/// are written beside this status and nowhere else.
pub const STATUS_REVOKED: &str = "REVOKED";

/// `DomainStatus` of a domain whose ownership has not been proven yet.
pub const DOMAIN_STATUS_PENDING_VALIDATION: &str = "PENDING_VALIDATION";

/// `DomainStatus` of a domain whose ownership has been proven.
pub const DOMAIN_STATUS_SUCCESS: &str = "SUCCESS";

/// `DomainStatus` of a domain whose validation did not complete.
pub const DOMAIN_STATUS_FAILED: &str = "FAILED";

/// `FailureReason` of a request ACM refuses up front, before it ever waits on proof
/// of domain control: "A certificate enters status PENDING_VALIDATION upon being
/// requested, UNLESS it fails for any of the reasons given in the troubleshooting
/// topic Certificate request fails" (`CertificateDetail$Status` doc). The reason ACM
/// models for a request it cannot process without information it has to obtain
/// elsewhere is `ADDITIONAL_VERIFICATION_REQUIRED`, which is what a request naming a
/// domain the public DNS delegates to nobody carries
/// ([`domain_is_not_in_public_dns`]). `FailureReason` "exists only when the
/// certificate status is FAILED" (`CertificateDetail$FailureReason` doc), so it is set
/// only beside [`STATUS_FAILED`].
pub const FAILURE_REASON_ADDITIONAL_VERIFICATION_REQUIRED: &str =
    "ADDITIONAL_VERIFICATION_REQUIRED";

/// The span this emulator gives a transition it owes a REQUESTED certificate: the least
/// time a transition the clock makes due on its own stays owed for.
///
/// Every such span is measured from the call that started the transition and is never
/// slept on, so the only thing that decides whether a client ever observes the
/// transient state is how long the span is against that client's OWN next calls. A span
/// shorter than a client's call sequence makes the state it guards unobservable over
/// the network — the same request sequence then gets one answer from a fast client and
/// another from a slow one — so it is this floor rather than the wall-clock time AWS
/// happens to take.
pub const OWED_TRANSITION_SPAN: Duration = Duration::from_secs(3);

/// How long after `RequestCertificate` an outcome ACM reaches WITHOUT waiting on proof
/// of domain control falls due — issuance for a request that needs no validation, and
/// the up-front refusal of a name that can never be validated.
///
/// A CHOSEN span, not a documented one: AWS takes seconds to minutes to answer either,
/// which no client could poll through here, so it is [`OWED_TRANSITION_SPAN`] — long
/// enough that a caller that describes and then acts on the certificate straight after
/// requesting it, over the network rather than in process, still observes
/// `PENDING_VALIDATION` and every refusal keyed on it. It is never slept on: the due
/// instant is stored on the record and materialised by whichever call next touches the
/// store (see [`ComAmazonawsAcmCertificatemanagerState::materialize_due`]).
///
/// It sits BELOW [`VALIDATION_EMAIL_DISPATCH_DELAY`], so an outcome ACM reaches
/// without waiting on proof of domain control falls due before the initial validation
/// mail would have: such a request never reaches the domain validation the mail belongs
/// to, so the owed dispatch is dropped rather than sent
/// ([`Certificate::materialize_due`]). That ordering also makes THIS span the one that
/// bounds how long the pre-dispatch resend gate answers for such a request, which is
/// why it carries the same floor as the dispatch itself. The outcome of a request ACM
/// DOES wait on proof for is the timeout at the end of the documented
/// [`VALIDATION_WINDOW`], which the mail falls due long before.
pub const VALIDATION_SETTLE_DELAY: Duration = OWED_TRANSITION_SPAN;

/// How long after `RequestCertificate` the initial domain-validation EMAIL falls due,
/// for a request ACM carries into domain validation.
///
/// A CHOSEN span, not a documented one: AWS dispatches the mail itself, some way after
/// the request is accepted. It is never zero, and it is longer than the time a client
/// takes to make its next call over the network, so a caller that describes or resends
/// straight after requesting still observes the state where no mail has gone out yet —
/// an empty mailbox list, and the resend gate that goes with it. It is never slept on:
/// the due instant is stored on the record ([`Certificate::pending_validation_email`])
/// and materialised by whichever call next touches the store.
///
/// It is at least [`OWED_TRANSITION_SPAN`], for the same reason that span exists, and
/// strictly ABOVE [`VALIDATION_SETTLE_DELAY`], so a request that settles without
/// waiting on proof of domain control is never mailed.
pub const VALIDATION_EMAIL_DISPATCH_DELAY: Duration = Duration::from_secs(4);

/// How long ACM waits for proof of domain control before the request times out.
///
/// A DOCUMENTED span, not a chosen one: "ACM makes repeated attempts to validate a
/// certificate for 72 hours and then times out" (`CertificateDetail$Status` doc). Until
/// it elapses the request stays `PENDING_VALIDATION` with its evidence live — the CNAME
/// to publish, the mailboxes ACM wrote to — and past it the request is
/// [`STATUS_VALIDATION_TIMED_OUT`]. Stored and never slept on, exactly as the other
/// owed transitions are.
pub const VALIDATION_WINDOW: Duration = Duration::from_secs(72 * 60 * 60);

/// The top-level labels the IANA Special-Use Domain Names registry reserves, so that
/// the public DNS delegates them to NOBODY: `.example`, `.invalid`, `.localhost` and
/// `.test` are "reserved for ... testing and documentation" and are never delegated
/// (RFC 2606 §2, RFC 6761 §6), and `.local` (RFC 6762 §3) and `.onion` (RFC 7686)
/// likewise name no domain in the public DNS.
///
/// A name whose top-level label is one of these is no public domain at all, which is
/// what makes a request for it one ACM cannot carry through — see
/// [`domain_is_not_in_public_dns`]. Only WHOLE top-level labels belong here: an
/// ordinary registrable name under a delegated label (`.com`, `.net`, `.org`) is a
/// public domain whoever holds it, so such a request is one ACM waits for evidence on
/// rather than refuses — unless the name the registries reserve for documentation under
/// that label is one of the [`RESERVED_SECOND_LEVEL_NAMES`].
pub const RESERVED_TOP_LEVEL_LABELS: &[&str] = &[
    "example", "invalid", "localhost", "test", "local", "onion",
];

/// The SECOND-level names the registries reserve the same way, so that they too are
/// delegated to no registrant: `example.com`, `example.net` and `example.org` are
/// "reserved for use in documentation" and registered permanently to the registry itself
/// (RFC 2606 §3, IANA "Example Domains"), so nobody holds them and nobody can publish
/// evidence of control over a name under one.
///
/// A request naming such a name is therefore one ACM cannot carry through either, for
/// the same reason a reserved top-level label is ([`domain_is_not_in_public_dns`]). Only
/// names the registries reserve belong here: an ordinary registrable name under a
/// delegated top-level label (`probe-domain.com`) has a holder who could publish the
/// evidence, so its request waits rather than being refused.
pub const RESERVED_SECOND_LEVEL_NAMES: &[&str] =
    &["example.com", "example.net", "example.org"];

/// Whether the REGISTRIES reserve `domain_name` rather than delegating it to a
/// registrant: its top-level label is a reserved one ([`RESERVED_TOP_LEVEL_LABELS`]) or
/// it sits at or under a reserved second-level name ([`RESERVED_SECOND_LEVEL_NAMES`]).
///
/// That is only half of whether the name has a holder: a name the ACCOUNT is itself
/// authoritative for in the public DNS this emulator serves has one whatever the
/// registries say, so the question a certificate request is decided by is
/// [`any_name_is_not_in_public_dns_in`], which reads the DNS as well.
///
/// A certificate request for a name NOBODY holds is one ACM cannot carry through at
/// all, so it is refused up front rather than left waiting for evidence that cannot
/// arrive — "a certificate enters status PENDING_VALIDATION upon being requested, unless
/// it fails for any of the reasons given in the troubleshooting topic Certificate
/// request fails" (`CertificateDetail$Status` doc).
///
/// Every name the public DNS DOES delegate is one whose holder could publish the
/// evidence, whether or not this emulator ever observes it published, so its request
/// enters `PENDING_VALIDATION` and waits the documented [`VALIDATION_WINDOW`] out:
/// "ACM makes repeated attempts to validate a certificate for 72 hours and then times
/// out" (`CertificateDetail$Status` doc). A challenge nobody has published yet is that
/// wait, not a request failure — the reasons a request FAILS are the modelled
/// `FailureReason` values, none of which is an unpublished challenge.
///
/// Matched on DOT BOUNDARIES and case-insensitively, as a DNS suffix is, and through
/// the name ACM actually proves control of ([`validation_target`]), so a wildcard is
/// judged by the domain it covers.
pub fn domain_is_not_in_public_dns(domain_name: &str) -> bool {
    let name = validation_target(domain_name).to_ascii_lowercase();
    RESERVED_TOP_LEVEL_LABELS
        .iter()
        .chain(RESERVED_SECOND_LEVEL_NAMES)
        .any(|reserved| {
            name == *reserved
                || name
                    .strip_suffix(reserved)
                    .is_some_and(|higher| higher.ends_with('.'))
        })
}

/// How long after the domain-validation email was last sent ACM will send it again.
///
/// A DOCUMENTED span, not a chosen one: "you can request that the mail be resent
/// within 72 hours of requesting the ACM certificate. If more than 72 hours have
/// elapsed since your original request or since your last attempt to resend
/// validation mail, you must request a new certificate" (`ResendValidationEmail`
/// doc). Past it there is no mail left to resend, which is the invalid state
/// `ResendValidationEmail` reports.
pub const RESEND_VALIDATION_WINDOW: Duration = Duration::from_secs(72 * 60 * 60);

/// How long a certificate this emulator issues is valid for: ACM certificates are
/// valid for 13 months (395 days) (ACM user guide, "Managed certificate renewal"),
/// which is the span between the `NotBefore` and `NotAfter` of an issued record.
pub const CERTIFICATE_VALIDITY: Duration = Duration::from_secs(395 * 24 * 60 * 60);

/// The `KeyUsageName` values of the X.509 Key Usage extension of a TLS server
/// certificate ACM issues, which a certificate carries only once it has been issued.
pub const ISSUED_KEY_USAGES: &[&str] = &["DIGITAL_SIGNATURE", "KEY_ENCIPHERMENT"];

/// The `KeyUsageName` a certificate whose Key Usage extension is ABSENT is reported and
/// selected under: the sentinel that enum declares beside the single purposes
/// (`smithy_query(query="shape", shape="com.amazonaws.acm#KeyUsageName")`). An X.509
/// certificate carrying no Key Usage extension restricts its key to no purpose at all,
/// so every purpose is open to it, which is the value that sentinel names.
pub const KEY_USAGE_ANY: &str = "ANY";

/// The key usages of a certificate that carries NO Key Usage extension: the
/// [`KEY_USAGE_ANY`] sentinel alone, which is the value that absence has.
///
/// The single place that sentinel is minted, so every reader of a certificate whose
/// X.509 extensions have been read agrees about it.
pub fn no_key_usage() -> Vec<String> {
    vec![KEY_USAGE_ANY.to_string()]
}

/// The key purposes of the X.509 Extended Key Usage extension of a TLS certificate ACM
/// issues — server and client authentication — each as the object identifier RFC 5280
/// section 4.2.1.12 assigns it, with the same lifecycle as [`ISSUED_KEY_USAGES`].
///
/// Stored as the identifiers rather than the names because the identifier is what an
/// X.509 extension carries; the name each one is reported under comes from
/// [`EXTENDED_KEY_USAGE_OIDS`].
pub const ISSUED_EXTENDED_KEY_USAGE_OIDS: &[&str] =
    &["1.3.6.1.5.5.7.3.1", "1.3.6.1.5.5.7.3.2"];

/// One key purpose of a certificate's X.509 Extended Key Usage extension, as ACM
/// publishes it: the object identifier the extension carries and the
/// `ExtendedKeyUsageName` that identifier is named by.
///
/// Both halves are stored because a purpose outside the ones the model names is
/// reported under one name ([`EXTENDED_KEY_USAGE_CUSTOM`]), so the identifier is the
/// only thing that still says WHICH purpose it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtendedKeyUsageRecord {
    /// An `ExtendedKeyUsageName` value.
    pub name: String,
    /// The dotted-decimal object identifier of the key purpose, and the EMPTY string
    /// for the [`EXTENDED_KEY_USAGE_NONE`] sentinel, which is the ABSENCE of the
    /// extension rather than a purpose an extension carries: `ExtendedKeyUsage$OID` is
    /// "an object identifier (OID) for the extension value", and there is no extension
    /// value to identify.
    pub oid: String,
}

/// The `ExtendedKeyUsageName` each key purpose is reported under, as
/// `ExtendedKeyUsage$OID`'s own documentation pairs the two
/// (`smithy_query(query="doc", shape="com.amazonaws.acm#ExtendedKeyUsage",
/// member="OID")` — "1.3.6.1.5.5.7.3.1 (TLS_WEB_SERVER_AUTHENTICATION)" and the eight
/// entries beside it), plus `anyExtendedKeyUsage` (`2.5.29.37.0`, RFC 5280 section
/// 4.2.1.12), which the model names `ANY`.
pub const EXTENDED_KEY_USAGE_OIDS: &[(&str, &str)] = &[
    ("TLS_WEB_SERVER_AUTHENTICATION", "1.3.6.1.5.5.7.3.1"),
    ("TLS_WEB_CLIENT_AUTHENTICATION", "1.3.6.1.5.5.7.3.2"),
    ("CODE_SIGNING", "1.3.6.1.5.5.7.3.3"),
    ("EMAIL_PROTECTION", "1.3.6.1.5.5.7.3.4"),
    ("IPSEC_END_SYSTEM", "1.3.6.1.5.5.7.3.5"),
    ("IPSEC_TUNNEL", "1.3.6.1.5.5.7.3.6"),
    ("IPSEC_USER", "1.3.6.1.5.5.7.3.7"),
    ("TIME_STAMPING", "1.3.6.1.5.5.7.3.8"),
    ("OCSP_SIGNING", "1.3.6.1.5.5.7.3.9"),
    ("ANY", "2.5.29.37.0"),
];

/// The `ExtendedKeyUsageName` a key purpose outside [`EXTENDED_KEY_USAGE_OIDS`] is
/// reported under, the value that enum declares for exactly that.
pub const EXTENDED_KEY_USAGE_CUSTOM: &str = "CUSTOM";

/// The `ExtendedKeyUsageName` a certificate whose Extended Key Usage extension is
/// ABSENT is reported and selected under: the sentinel that enum declares beside the
/// real purposes (`smithy_query(query="shape",
/// shape="com.amazonaws.acm#ExtendedKeyUsageName")`), which is what "this certificate
/// asserts no key purpose" is named by — so a reader can tell it apart from a record
/// whose purposes are not known yet, and the `extendedKeyUsage` filters can select it.
pub const EXTENDED_KEY_USAGE_NONE: &str = "NONE";

/// The key purposes of a certificate that carries NO Extended Key Usage extension:
/// the [`EXTENDED_KEY_USAGE_NONE`] sentinel alone, which is the value that absence has,
/// under the empty identifier — the sentinel stands for an extension the certificate
/// does not carry, so there is no extension value for an identifier to name.
///
/// The single place that sentinel is minted, so every reader of a certificate whose
/// X.509 extensions have been read — its detail, its listing summary, its search
/// result and the filters each of those selects on — agrees about it.
pub fn no_extended_key_usage() -> Vec<ExtendedKeyUsageRecord> {
    vec![ExtendedKeyUsageRecord {
        name: EXTENDED_KEY_USAGE_NONE.to_string(),
        oid: String::new(),
    }]
}

/// The key purpose `oid` names, as ACM publishes it: the identifier itself beside the
/// name [`EXTENDED_KEY_USAGE_OIDS`] pairs it with, or [`EXTENDED_KEY_USAGE_CUSTOM`]
/// for one that catalogue does not name.
///
/// The single place a key purpose becomes a reportable pair, so the purposes read out
/// of an imported certificate's DER and those of a certificate ACM issues cannot be
/// named by two different catalogues.
pub fn extended_key_usage(oid: &str) -> ExtendedKeyUsageRecord {
    let name = EXTENDED_KEY_USAGE_OIDS
        .iter()
        .find(|(_, identifier)| *identifier == oid)
        .map_or(EXTENDED_KEY_USAGE_CUSTOM, |(name, _)| *name);
    ExtendedKeyUsageRecord {
        name: name.to_string(),
        oid: oid.to_string(),
    }
}

/// `CertificateType`: "For certificates provided by ACM, this value is
/// `AMAZON_ISSUED`" (`CertificateDetail$Type` doc).
pub const TYPE_AMAZON_ISSUED: &str = "AMAZON_ISSUED";

/// `CertificateType` of a certificate a client brought into ACM itself:
/// `ImportCertificate` "imports a certificate into Certificate Manager (ACM)"
/// (`ImportCertificate` doc), and `ImportedAt` "exists only when the certificate type
/// is IMPORTED" (`CertificateDetail$ImportedAt` doc).
pub const TYPE_IMPORTED: &str = "IMPORTED";

/// `RenewalEligibility`: "only exported private certificates can be renewed with the
/// RenewCertificate command" (`CertificateDetail$RenewalEligibility` doc), so a
/// public certificate ACM issues is not eligible.
pub const RENEWAL_INELIGIBLE: &str = "INELIGIBLE";

/// `RenewalEligibility` of the one certificate ACM renews on request: "only exported
/// private certificates can be renewed with the RenewCertificate command"
/// (`CertificateDetail$RenewalEligibility` doc) — see
/// [`Certificate::renewal_eligibility`].
pub const RENEWAL_ELIGIBLE: &str = "ELIGIBLE";

/// `RenewalStatus` of a renewal ACM has started and not finished: "the status of ACM's
/// managed renewal of the certificate" (`RenewalSummary$RenewalStatus` doc).
pub const RENEWAL_STATUS_PENDING_AUTO_RENEWAL: &str = "PENDING_AUTO_RENEWAL";

/// `RenewalStatus` of a renewal that completed, which is when the renewed
/// certificate's validity window is the one the record carries.
pub const RENEWAL_STATUS_SUCCESS: &str = "SUCCESS";

/// `RenewalStatus` of a renewal ACM could not complete, which is what a renewal owed to
/// a certificate that stopped being issued before it fell due ends as.
pub const RENEWAL_STATUS_FAILED: &str = "FAILED";

/// How long after `RenewCertificate` the renewal it started falls due.
///
/// A CHOSEN span, not a documented one: ACM's managed renewal takes minutes to hours,
/// which no client could poll through here. It is never zero, so a caller that reads
/// straight after renewing still observes [`RENEWAL_STATUS_PENDING_AUTO_RENEWAL`], and
/// it is never slept on: the due instant is stored on the record ([`Renewal::due_at`])
/// and materialised by whichever call next touches the store.
pub const RENEWAL_SETTLE_DELAY: Duration = Duration::from_millis(750);

/// `CertificateKeyPairOrigin` of a certificate whose key pair ACM generated, which
/// is every certificate `RequestCertificate` creates.
pub const KEY_PAIR_ORIGIN_AWS_MANAGED: &str = "AWS_MANAGED";

/// `CertificateKeyPairOrigin` of a certificate whose key pair the CLIENT generated,
/// which is every certificate `ImportCertificate` stores: the request carries the
/// private key "that matches the public key in the certificate"
/// (`ImportCertificateRequest$PrivateKey` doc).
pub const KEY_PAIR_ORIGIN_CUSTOMER_PROVIDED: &str = "CUSTOMER_PROVIDED";

/// `CertificateExport` when `Options.Export` is absent: export is opt-in
/// (`CertificateOptions$Export` doc).
pub const EXPORT_DISABLED: &str = "DISABLED";

/// `CertificateTransparencyLoggingPreference` when `Options` names none: "All public
/// certificates are recorded in a certificate transparency log" and "certificate
/// transparency logging opt-out is no longer available" (`CertificateOptions` doc), so
/// a certificate that asked for nothing is logged.
pub const TRANSPARENCY_LOGGING_ENABLED: &str = "ENABLED";

/// `CertificateDetail$Issuer` ("the name of the certificate authority that issued and
/// signed the certificate") for a public certificate ACM issues: Amazon's own CA
/// signs it.
pub const ISSUER_AMAZON: &str = "Amazon";

/// `CertificateExport` of a certificate whose owner opted in to export: "You can opt
/// in to allow the export of your certificates by specifying ENABLED. You cannot
/// update the value of Export after the the certificate is created"
/// (`CertificateOptions$Export` doc) — so this is fixed when the certificate is
/// created and is what decides whether it may ever be exported, and therefore
/// revoked.
pub const EXPORT_ENABLED: &str = "ENABLED";

/// `RecordType` of the DNS validation record: "Currently this can be `CNAME`"
/// (`ResourceRecord$Type` doc).
pub const RECORD_TYPE_CNAME: &str = "CNAME";

/// The zone ACM publishes DNS-validation CNAME targets under.
pub const ACM_VALIDATIONS_ZONE: &str = "acm-validations.aws";

/// The path a CA fetches an HTTP domain-validation token from — the
/// `/.well-known/pki-validation/` location the CA/Browser Forum baseline
/// requirements define, which is where `HttpRedirect$RedirectFrom` points.
pub const HTTP_VALIDATION_PATH: &str = "/.well-known/pki-validation/";

/// The `KeyAlgorithm` values `ListCertificates` reports when the request supplied no
/// `Includes` structure, which is the only way the nested `keyTypes` filter goes
/// unsupplied: a request that sent an `Includes` chose its filters, so one that left
/// `keyTypes` out of it filters on no key algorithm at all. A certificate of an
/// algorithm outside this set is therefore listed to a request that asks for that
/// algorithm by name, and to one that supplied an `Includes` naming no `keyTypes`.
///
/// Two sentences in this service's model describe the default, and they disagree on
/// `RSA_1024`. The member describing this very filter is the wider one and is what is
/// encoded here — read it verbatim with:
///
/// ```text
/// smithy_query(query="doc", shape="com.amazonaws.acm#Filters", member="keyTypes",
///     model_path="services/service-com-amazonaws-acm-certificatemanager/smithy-model/com-amazonaws-acm-certificatemanager.json")
/// ```
///
/// which answers "Default filtering returns only RSA_1024 and RSA_2048 certificates
/// that have at least one domain. To return other certificate types, provide the
/// desired type signatures in a comma-separated list." The operation's own doc
/// (`smithy_query(query="doc", shape="com.amazonaws.acm#ListCertificates")`) says
/// "Default filtering returns only RSA_2048 certificates"; taking the member sentence
/// means the narrower claim is never the reason a stored certificate is withheld,
/// which is the safer way to be wrong about a filter nobody asked for.
pub const DEFAULT_LISTED_KEY_ALGORITHMS: &[&str] = &["RSA_1024", "RSA_2048"];

/// The `CertificateKeyPairOrigin` values `ListCertificates` reports when the request
/// names no `CertificateKeyPairOrigins`: "Default filtering returns only certificates
/// with key pair origin of AWS_MANAGED and CUSTOMER_PROVIDED"
/// (`ListCertificatesRequest$CertificateKeyPairOrigins` doc), i.e. "by default, this
/// action does not return certificates with a CertificateKeyPairOrigin of ACME"
/// (`ListCertificates` doc).
pub const DEFAULT_LISTED_KEY_PAIR_ORIGINS: &[&str] = &["AWS_MANAGED", "CUSTOMER_PROVIDED"];

/// `SortBy=CREATED_AT`, the only sort key `ListCertificates` models.
pub const SORT_BY_CREATED_AT: &str = "CREATED_AT";

/// `SortOrder=DESCENDING`.
pub const SORT_ORDER_DESCENDING: &str = "DESCENDING";

/// The number of subject alternative names a `CertificateSummary` carries;
/// `HasAdditionalSubjectAlternativeNames` reports whether the certificate has more
/// (`CertificateSummary$HasAdditionalSubjectAlternativeNames` doc).
pub const SUMMARY_SAN_LIMIT: usize = 100;

/// `AcmeEndpointStatus` of an ACME endpoint ACM is still standing up: an endpoint is
/// "a managed ACME server with a unique endpoint URL" (`CreateAcmeEndpoint` doc), and
/// `AcmeEndpointStatus` models `CREATING` beside `ACTIVE` precisely because the server
/// is not serving the instant the call returns.
pub const ACME_ENDPOINT_STATUS_CREATING: &str = "CREATING";

/// `AcmeEndpointStatus` of an ACME endpoint that is serving: "After creation, ACME
/// clients can use the endpoint URL to automate certificate issuance"
/// (`CreateAcmeEndpoint` doc).
pub const ACME_ENDPOINT_STATUS_ACTIVE: &str = "ACTIVE";

/// The most ACME endpoint records this emulator stores, across every Region.
///
/// Not an AWS quota: ACM's own endpoint quota is per Region, and the Region of a
/// request is read off the caller's SigV4 credential scope, so a per-Region bound is a
/// bound the CALLER can partition by signing each request for another Region string.
/// This cap is keyed on nothing in the request, so it bounds
/// [`ComAmazonawsAcmCertificatemanagerState::acme_endpoints`] (and with it the
/// per-request `materialize_due` pass over those records) however the request varies.
/// `CreateAcmeEndpoint` checks it before inserting and answers its modelled quota error
/// past it.
pub const ACME_ENDPOINT_STORE_LIMIT: usize = 1_000;

/// How long after `CreateAcmeEndpoint` the endpoint reaches
/// [`ACME_ENDPOINT_STATUS_ACTIVE`].
///
/// A CHOSEN span, not a documented one: neither the model nor ACM's documentation
/// states how long standing an ACME endpoint up takes. It is never zero, so a caller
/// that describes the endpoint straight after creating it still observes `CREATING`,
/// and it is never slept on: the due instant is stored on the record and materialised
/// by whichever call next touches the store (see
/// [`ComAmazonawsAcmCertificatemanagerState::materialize_due`]).
pub const ACME_ENDPOINT_ACTIVATION_DELAY: Duration = Duration::from_millis(500);

/// `AcmeDomainValidationStatus` of a domain validation whose prevalidation ACM is
/// still working through: `AcmeDomainValidationStatus` models `VALIDATING` beside
/// `VALID` precisely because ACM has not finished proving domain ownership the instant
/// `CreateAcmeDomainValidation` returns
/// (`smithy_query(query="shape", shape="com.amazonaws.acm#AcmeDomainValidationStatus")`
/// declares `VALIDATING | VALID | INVALID | DELETING`).
pub const ACME_DOMAIN_VALIDATION_STATUS_VALIDATING: &str = "VALIDATING";

/// `AcmeDomainValidationStatus` of a domain validation that authorizes its endpoint to
/// issue certificates for the domain: "domain validations authorize the endpoint to
/// issue certificates for specified domain names" (`CreateAcmeDomainValidation` doc).
pub const ACME_DOMAIN_VALIDATION_STATUS_VALID: &str = "VALID";

/// `AcmeDomainValidationStatus` of a domain validation ACM could not complete; it is
/// the status whose `FailureDetails` ("details about the failure, if the validation
/// failed", `AcmeDomainValidation$FailureDetails` doc) is set. No path in this emulator
/// reaches it yet: nothing here can fail a prevalidation, so a created validation
/// settles to [`ACME_DOMAIN_VALIDATION_STATUS_VALID`], and the record's
/// `failure_details` is what an operation that CAN fail one would write beside this
/// status.
pub const ACME_DOMAIN_VALIDATION_STATUS_INVALID: &str = "INVALID";

/// `PrevalidationType` of a domain validation configured with
/// `PrevalidationOptions$DnsPrevalidation` — the only member either union declares.
///
/// The WIRE value, which is what a client branches on to find the DNS challenge record
/// it has to publish: ACM answers `DNS` for a DNS-prevalidated domain validation, while
/// the model's enum declares the older spelling `DNS_PREVALIDATION`
/// (`smithy_query(query="shape", shape="com.amazonaws.acm#PrevalidationType")`). Where
/// the two disagree the value the service really returns is the contract, so every
/// projection of this member reports it from here.
pub const PREVALIDATION_TYPE_DNS: &str = "DNS";

/// How long after `CreateAcmeDomainValidation` — or after an update replaced the
/// prevalidation configuration the domain was proved against — the validation reaches
/// [`ACME_DOMAIN_VALIDATION_STATUS_VALID`].
///
/// A CHOSEN span, not a documented one: neither the model nor ACM's documentation
/// states how long prevalidating a domain takes. It is never zero, so a caller that
/// describes the validation straight after creating it still observes `VALIDATING`, and
/// it is never slept on: the due instant is stored on the record and materialised by
/// whichever call next touches the store (see
/// [`ComAmazonawsAcmCertificatemanagerState::materialize_due`]).
pub const ACME_DOMAIN_VALIDATION_SETTLE_DELAY: Duration = Duration::from_millis(500);

/// The number of domain validations one ACME endpoint may hold.
///
/// Neither the model nor ACM's documentation states this quota, so the NUMBER is
/// chosen — generously, since a bound that is too low refuses a request ACM would
/// accept. What it encodes is that an endpoint's domain validations are a bounded
/// resource and that past the bound `CreateAcmeDomainValidation` answers the quota
/// error it models.
pub const ACME_DOMAIN_VALIDATION_LIMIT: usize = 500;

/// The most ACME domain-validation records this emulator stores, across every endpoint.
///
/// The safety bound, for the same reason [`ACME_ENDPOINT_STORE_LIMIT`] is one: the
/// per-endpoint quota above is keyed on a parent the caller can create more of, so it
/// alone would not bound
/// [`ComAmazonawsAcmCertificatemanagerState::acme_domain_validations`] (nor the
/// per-request `materialize_due` pass over those records).
pub const ACME_DOMAIN_VALIDATION_STORE_LIMIT: usize = 10_000;

/// The number of external account bindings one ACME endpoint may hold.
///
/// Neither the model nor ACM's documentation states this quota, so the NUMBER is
/// chosen — generously, since a bound that is too low refuses a request ACM would
/// accept. What it encodes is that an endpoint's bindings are a bounded resource and
/// that past the bound `CreateAcmeExternalAccountBinding` answers the quota error it
/// models, exactly as [`ACME_DOMAIN_VALIDATION_LIMIT`] does for the other resource an
/// endpoint holds.
pub const ACME_EXTERNAL_ACCOUNT_BINDING_LIMIT: usize = 500;

/// The most ACME external-account-binding records this emulator stores, across every
/// endpoint.
///
/// The safety bound, for the same reason [`ACME_DOMAIN_VALIDATION_STORE_LIMIT`] is one:
/// the per-endpoint quota above is keyed on a parent the caller can create more of, so
/// it alone would not bound
/// [`ComAmazonawsAcmCertificatemanagerState::acme_external_account_bindings`].
pub const ACME_EXTERNAL_ACCOUNT_BINDING_STORE_LIMIT: usize = 10_000;

/// `TimeType=MINUTES`, as `Expiration$Type` spells the unit of a minute
/// (`smithy_query(query="shape", shape="com.amazonaws.acm#TimeType")` declares
/// `MINUTES | HOURS | DAYS`).
pub const TIME_TYPE_MINUTES: &str = "MINUTES";

/// `TimeType=HOURS`.
pub const TIME_TYPE_HOURS: &str = "HOURS";

/// `TimeType=DAYS`.
pub const TIME_TYPE_DAYS: &str = "DAYS";

/// The span one `Expiration` names, or `None` when it names none this clock can
/// represent.
///
/// `Expiration` is a `Value` counted in a `Type` unit (`Expiration$Value` "the numeric
/// value of the expiration", `$Type` "the time unit for the expiration value"), so the
/// span is the product of the two and the instant a resource configured with it expires
/// is that span after it was created. The unit set is [`TIME_TYPE_MINUTES`],
/// [`TIME_TYPE_HOURS`] and [`TIME_TYPE_DAYS`]; the boundary validator has already
/// refused any other `TimeType` and any `Value` below the member's `@range(min: 1)`
/// before a handler runs, so a `None` here is a product no `SystemTime` can hold rather
/// than a value a caller could correct.
pub fn expiration_span(expiration: &Expiration) -> Option<Duration> {
    let unit = match expiration.r#type.as_str() {
        TIME_TYPE_MINUTES => 60,
        TIME_TYPE_HOURS => 60 * 60,
        TIME_TYPE_DAYS => 24 * 60 * 60,
        _ => return None,
    };
    u64::try_from(expiration.value)
        .ok()?
        .checked_mul(unit)
        .map(Duration::from_secs)
}

/// `AcmeAccountStatus` of a registered ACME account that may request and revoke
/// certificates: the status an account carries until it is deactivated or revoked
/// (`smithy_query(query="shape", shape="com.amazonaws.acm#AcmeAccountStatus")`
/// declares `VALID | DEACTIVATED | REVOKED`).
pub const ACME_ACCOUNT_STATUS_VALID: &str = "VALID";

/// `AcmeAccountStatus` of an ACME account its own client deactivated (RFC 8555 §7.3.6
/// account deactivation), which ACM reports beside the status it reaches when ACM
/// revokes it.
pub const ACME_ACCOUNT_STATUS_DEACTIVATED: &str = "DEACTIVATED";

/// `AcmeAccountStatus` of an ACME account `RevokeAcmeAccount` has revoked, which
/// "prevent[s] it from requesting or revoking certificates" (`RevokeAcmeAccount` doc).
pub const ACME_ACCOUNT_STATUS_REVOKED: &str = "REVOKED";

/// The most ACME accounts one endpoint may hold.
///
/// ACM publishes no quota for this, so the bound is the one this store needs: an ACME
/// account is registered by a client of the endpoint's own data plane rather than by a
/// modelled operation, so without it one unauthenticated wire could grow the store
/// without end. The same order as the other per-endpoint bounds this service applies
/// ([`ACME_DOMAIN_VALIDATION_LIMIT`], [`ACME_EXTERNAL_ACCOUNT_BINDING_LIMIT`]).
pub const ACME_ACCOUNT_LIMIT: usize = 500;

/// The most ACME accounts this store holds across every endpoint, for the same reason
/// [`ACME_DOMAIN_VALIDATION_STORE_LIMIT`] exists: the per-endpoint bound alone would let
/// the store grow with the endpoints.
pub const ACME_ACCOUNT_STORE_LIMIT: usize = 10_000;

/// The most anti-replay nonces one endpoint keeps outstanding at a time.
///
/// RFC 8555 §6.5 fixes only that a nonce is single-use; how long a server remembers one
/// it issued is the server's own choice, and this is that choice stated as a COUNT rather
/// than as a span: the endpoint remembers its most recent nonces and forgets the ones
/// behind them ([`AcmeEndpointRecord::issue_nonce`]), so the set cannot grow with the
/// requests a client makes and a client that fetches a nonce and posts with it is always
/// inside the window.
pub const ACME_NONCE_WINDOW: usize = 256;

/// The account-level configuration `PutAccountConfiguration` writes and
/// `GetAccountConfiguration` reports. One per account, so the state holds a
/// singleton rather than a map.
///
/// "The supported configuration option is DaysBeforeExpiry"
/// (`PutAccountConfiguration` doc), so that option is all this record carries.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AccountConfiguration {
    /// `ExpiryEventsConfiguration$DaysBeforeExpiry` as the last write carried it;
    /// `None` when that write named an `ExpiryEvents` with no `DaysBeforeExpiry`.
    pub days_before_expiry: Option<i32>,
}

/// The largest `DaysBeforeExpiry` ACM accepts. The model constrains this option
/// only from below (`PositiveInteger`, `@range(min: 1)`), but the service refuses
/// anything past the default notice window of 45 days with a `ValidationException`
/// saying so, so the accepted range is 1..=45.
pub const MAX_DAYS_BEFORE_EXPIRY: i32 = 45;

/// `ExpiryEventsConfiguration$DaysBeforeExpiry` for an account that has never
/// written one: "By default, accounts receive events starting 45 days before
/// certificate expiration" (`ExpiryEventsConfiguration$DaysBeforeExpiry` doc), so
/// that is the configuration such an account is running under and what
/// `GetAccountConfiguration` reports for it.
pub const DEFAULT_DAYS_BEFORE_EXPIRY: i32 = 45;

/// How long a `PutAccountConfiguration` idempotency token stays live: "Idempotency
/// tokens time out after one hour"
/// (`PutAccountConfigurationRequest$IdempotencyToken` doc). Past it the token names
/// no earlier request, so the next call carrying it is a new request.
pub const IDEMPOTENCY_TOKEN_TTL: Duration = Duration::from_secs(60 * 60);

/// One `PutAccountConfiguration` request ACM has already processed, kept so a call
/// repeating its idempotency token can be recognised as the same request — or, when
/// it asks for something else, refused as a different one.
#[derive(Debug, Clone)]
pub struct AccountConfigurationRequest {
    /// The `ExpiryEvents` that request carried, and `None` when it carried none.
    pub expiry_events: Option<AccountConfiguration>,
    /// When ACM processed it; the token times out [`IDEMPOTENCY_TOKEN_TTL`] later.
    pub processed_at: SystemTime,
}

impl AccountConfigurationRequest {
    /// Whether this request's idempotency token is still live at `now`.
    pub fn token_is_live(&self, now: SystemTime) -> bool {
        now.duration_since(self.processed_at)
            .is_ok_and(|elapsed| elapsed < IDEMPOTENCY_TOKEN_TTL)
            // A record stamped in the future (a clock that stepped back) has not
            // timed out either.
            || self.processed_at > now
    }
}

/// The validation ACM is waiting on for one domain name in a certificate.
#[derive(Debug, Clone)]
pub struct DomainValidationRecord {
    /// The FQDN being validated.
    pub domain_name: String,
    /// A `DomainStatus` value.
    pub validation_status: String,
    /// A `ValidationMethod` value.
    pub validation_method: String,
    /// The domain ACM proves control of for this name — "the domain name that ACM
    /// used to send domain validation emails" — which is the name itself unless the
    /// request named another in `DomainValidationOptions`.
    pub validation_domain: Option<String>,
    /// The validation domain the certificate was REQUESTED with for this name
    /// (`DomainValidationOption$ValidationDomain`), kept whatever method is in force,
    /// so a certificate migrated away from `EMAIL` and back mails the domain the
    /// request named rather than the covered name itself.
    pub requested_validation_domain: Option<String>,
    /// The mailboxes ACM has sent a domain-validation email to: the five role-account
    /// addresses at [`Self::validation_domain`], for a name ACM validates BY EMAIL.
    /// Empty until the mail has actually gone out — the initial dispatch the clock owes
    /// a request ([`Certificate::pending_validation_email`]), or a
    /// `ResendValidationEmail` — and empty for a name validated by DNS or HTTP, where a
    /// mailbox list does not apply at all; [`domain_validation`] reports the difference
    /// from the record's own method.
    pub validation_emails: Vec<String>,
    /// The CNAME to publish, for DNS validation.
    pub resource_record: Option<ResourceRecord>,
    /// The URLs the CA fetches, for HTTP validation.
    pub http_redirect: Option<HttpRedirect>,
}

/// How the validation of a requested certificate resolves.
///
/// Decided by the call that starts the transition, from what the request asks for, and
/// revised by the proof the DNS later carries (`dns_validation`); applied when the
/// transition falls due.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValidationOutcome {
    /// The certificate is issued: "If you are requesting a private certificate,
    /// domain validation is not required" (`RequestCertificate` doc), so there is
    /// nothing left to prove and the request completes — and likewise for a public
    /// request whose domains the published DNS has since proven (`dns_validation`).
    Issued,
    /// The request fails with [`FAILURE_REASON_ADDITIONAL_VERIFICATION_REQUIRED`]
    /// before ACM waits on anything: the request names a domain the public DNS
    /// delegates to nobody ([`domain_is_not_in_public_dns`]), which is one of the
    /// reasons a request fails "upon being requested" rather than entering
    /// `PENDING_VALIDATION` (`CertificateDetail$Status` doc). A name the public DNS
    /// DOES delegate never reaches this outcome, however long its challenge goes
    /// unpublished: that wait is [`Self::TimedOut`].
    Failed,
    /// The request times out unvalidated: "If you are requesting a public certificate,
    /// each domain name that you specify must be validated to verify that you own or
    /// control the domain" (`RequestCertificate` doc) — proof that arrives out of band —
    /// and "ACM makes repeated attempts to validate a certificate for 72 hours and then
    /// times out" (`CertificateDetail$Status` doc). Until then the request stays
    /// `PENDING_VALIDATION`, so everything a pending validation is for (the published
    /// CNAME, the approval mail `ResendValidationEmail` resends) stays reachable.
    ///
    /// It is what a public request is owed WHILE its evidence is missing, not a verdict:
    /// a DNS-validated request whose CNAME is published is owed [`Self::Issued`] instead
    /// (`dns_validation`).
    TimedOut,
}

impl ValidationOutcome {
    /// How long after the request this outcome falls due: an outcome ACM reaches on its
    /// own after [`VALIDATION_SETTLE_DELAY`], and the unvalidated timeout at the end of
    /// the documented [`VALIDATION_WINDOW`].
    pub fn settles_in(self) -> Duration {
        match self {
            Self::Issued | Self::Failed => VALIDATION_SETTLE_DELAY,
            Self::TimedOut => VALIDATION_WINDOW,
        }
    }
}

/// The validation outcome the clock already owes a requested certificate.
///
/// Stored, never slept on: the instant it falls due is fixed by the call that started
/// it, so the outcome is owed at that instant whether or not anybody calls again.
#[derive(Debug, Clone)]
pub struct PendingValidation {
    /// When the outcome falls due.
    pub due_at: SystemTime,
    /// What falls due then.
    pub outcome: ValidationOutcome,
}

impl PendingValidation {
    /// The outcome owed to a certificate requested at `requested_at`, due the span that
    /// outcome takes later ([`ValidationOutcome::settles_in`]).
    pub fn starting_at(requested_at: SystemTime, outcome: ValidationOutcome) -> Self {
        Self {
            due_at: requested_at + outcome.settles_in(),
            outcome,
        }
    }
}

/// The managed renewal ACM has started for one certificate: the status it reports and
/// the instant its completion falls due, while it is still owed.
///
/// Stored, never slept on, exactly as [`PendingValidation`] is: the instant is fixed by
/// the `RenewCertificate` call that started the renewal, so the renewal completes at
/// that instant whether or not anybody calls again.
#[derive(Debug, Clone)]
pub struct Renewal {
    /// A `RenewalStatus` value.
    pub status: String,
    /// "The time at which the renewal summary was last updated"
    /// (`RenewalSummary$UpdatedAt` doc).
    pub updated_at: SystemTime,
    /// When the renewal completes, while it is still owed; `None` once it has.
    pub due_at: Option<SystemTime>,
}

/// One stored ACM certificate. Every field a response reports is read from here.
#[derive(Debug, Clone)]
pub struct Certificate {
    /// `arn:aws:acm:<region>:<account>:certificate/<id>`.
    pub arn: String,
    pub domain_name: String,
    /// `domain_name` first, then the requested SANs, deduplicated.
    pub subject_alternative_names: Vec<String>,
    /// A `CertificateStatus` value.
    pub status: String,
    /// A `CertificateType` value.
    pub cert_type: String,
    /// A `KeyAlgorithm` value, in the model enum's spelling (`RSA_2048`) — the
    /// spelling the request and the `Includes.keyTypes` filter use. Every projection
    /// that REPORTS the algorithm renders it through [`reported_key_algorithm`].
    pub key_algorithm: String,
    /// "The name of the entity that is associated with the public key contained in the
    /// certificate" (`CertificateDetail$Subject`): the subject distinguished name,
    /// which ACM fixes when it accepts the request — see
    /// [`subject_distinguished_name`].
    pub subject: Option<String>,
    /// The name of the CA that signs the certificate ([`issuer_name`]).
    pub issuer: Option<String>,
    /// The algorithm that CA signs with ([`signature_algorithm`]).
    pub signature_algorithm: Option<String>,
    /// A `CertificateKeyPairOrigin` value.
    pub key_pair_origin: String,
    /// A `ValidationMethod` value, for a certificate ACM has to validate.
    pub validation_method: Option<String>,
    /// One entry per name in `subject_alternative_names`.
    pub domain_validations: Vec<DomainValidationRecord>,
    pub options: Option<CertificateOptions>,
    pub certificate_authority_arn: Option<String>,
    /// A `CertificateManagedBy` value.
    pub managed_by: Option<String>,
    /// The managed renewal `RenewCertificate` started for this certificate, while
    /// there is one: "contains information about the status of ACM's managed renewal
    /// for the certificate" (`CertificateDetail$RenewalSummary` doc). `None` for a
    /// certificate no renewal was ever asked for.
    pub renewal: Option<Renewal>,
    /// A `FailureReason` value, which "exists only when the certificate status is
    /// FAILED" (`CertificateDetail$FailureReason` doc).
    pub failure_reason: Option<String>,
    /// The validation outcome the clock owes this certificate, while it is still
    /// `PENDING_VALIDATION`. `None` once the certificate has reached a terminal
    /// status — there is nothing left for the clock to do.
    pub pending_validation: Option<PendingValidation>,
    /// The `KeyUsageName` values of the certificate's X.509 Key Usage extensions.
    /// Those extensions exist only once the certificate has been issued, so this is
    /// empty while it is pending — which is what the `Includes.keyUsage` filter
    /// compares against.
    pub key_usages: Vec<String>,
    /// The key purposes of the certificate's X.509 Extended Key Usage extension, each
    /// as the name/identifier pair ACM publishes it as
    /// ([`ExtendedKeyUsageRecord`]), with the same lifecycle as [`Self::key_usages`].
    pub extended_key_usages: Vec<ExtendedKeyUsageRecord>,
    /// Whether the certificate has ever been exported. Only an exported certificate
    /// may be revoked (`RevokeCertificate` doc: "You can only revoke certificates
    /// that have been previously exported").
    pub exported: bool,
    /// When ACM last sent this certificate's domain-validation email: the instant the
    /// initial dispatch fell due, moved forward by each `ResendValidationEmail`. `None`
    /// while no mail has gone out yet — a certificate ACM validates by DNS or HTTP, and
    /// one whose initial dispatch is still owed. It is what the documented
    /// [`RESEND_VALIDATION_WINDOW`] is measured from.
    pub validation_email_sent_at: Option<SystemTime>,
    /// When the INITIAL domain-validation email ACM owes this request falls due, for a
    /// certificate it validates by EMAIL. `None` once the mail has gone out, for a
    /// certificate ACM sends no validation mail for at all, and once the request has
    /// settled with the mail still owed — a request ACM has finished processing gets no
    /// validation mail afterwards, which is what a request whose outcome needs no proof
    /// of domain control has done before this span elapses.
    ///
    /// Stored, never slept on, exactly as [`Self::pending_validation`] is: the instant
    /// is fixed by the call that accepted the request, so the mail is owed at that
    /// instant whether or not anybody calls again.
    pub pending_validation_email: Option<SystemTime>,
    /// When the certificate was revoked, which "exists only when the certificate
    /// status is REVOKED" (`CertificateDetail$RevokedAt` doc).
    pub revoked_at: Option<SystemTime>,
    /// A `RevocationReason` value — the reason the caller gave `RevokeCertificate` —
    /// which "exists only when the certificate status is REVOKED"
    /// (`CertificateDetail$RevocationReason` doc).
    pub revocation_reason: Option<String>,
    pub created_at: SystemTime,
    pub issued_at: Option<SystemTime>,
    /// When the certificate was IMPORTED into ACM, which "exists only when the
    /// certificate type is IMPORTED" (`CertificateDetail$ImportedAt` doc) — so this is
    /// `Some` exactly for a record whose type is [`TYPE_IMPORTED`], and is re-stamped
    /// by a reimport that replaces the certificate stored under the same ARN.
    pub imported_at: Option<SystemTime>,
    /// "The serial number of the certificate" (`CertificateDetail$Serial` doc), which
    /// the certificate authority that signed it assigned, rendered as
    /// [`serial_number`] renders one. `None` for a record this emulator holds no
    /// signed certificate for — it signs none itself — and `Some` for an imported one,
    /// read out of the X.509 body the import carried.
    pub serial: Option<String>,
    pub not_before: Option<SystemTime>,
    pub not_after: Option<SystemTime>,
    /// The PEM-encoded X.509 certificate this ACM resource holds, as the call that
    /// stored it carried it, so a read reports the certificate itself rather than a
    /// re-encoding of what was parsed out of it.
    ///
    /// `None` for a record no signed certificate exists for: this emulator operates no
    /// certificate authority and signs nothing, so the only certificate material it
    /// holds is the material a client imported. `GetCertificate` reports this member,
    /// and answers the documented "has not yet been issued" refusal
    /// ([`REQUEST_IN_PROGRESS_MESSAGE`]) while it is absent.
    pub certificate_body: Option<String>,
    /// The PEM-encoded chain stored beside [`Self::certificate_body`] — "the
    /// certificate of the issuing CA and the intermediate certificates of any other
    /// subordinate CAs" (`GetCertificate` doc) — as the call that stored it carried it.
    ///
    /// `None` for a resource stored without one, which holds no issuing authority's
    /// certificate: the chain of trust `GetCertificate` then reports is the
    /// certificate itself.
    pub certificate_chain: Option<String>,
    /// Tag key -> value, as `ListTagsForCertificate` reports them.
    pub tags: BTreeMap<String, String>,
}

impl Certificate {
    /// Whether this record is a certificate a client imported, which is what decides
    /// the two instants that exist only for one kind of certificate: `ImportedAt`
    /// "exists only when the certificate type is IMPORTED" and `IssuedAt` "only when
    /// the certificate type is AMAZON_ISSUED" (`CertificateDetail` docs).
    pub fn is_imported(&self) -> bool {
        self.cert_type == TYPE_IMPORTED
    }

    /// Whether this certificate covers the host `server_name`.
    ///
    /// The names a certificate covers are its subject name and its subject alternative
    /// names, which is what [`Self::subject_alternative_names`] holds
    /// ([`Self::domain_name`] first, then the rest) — so WHICH hosts a certificate serves
    /// is answered from the record this service stores rather than re-read from X.509 by
    /// whoever asks. Asked by the services that hold a resource configured to serve a
    /// hostname under one of this account's certificates, which is a fact about the
    /// CERTIFICATE and so is decided here.
    ///
    /// One name covers one host by the match a TLS client applies to a certificate's
    /// names, including a leading wildcard label
    /// ([`::service_common::acm_certificate_key_material::name_covers`]), so a certificate
    /// covers a host for this answer exactly when it would serve that host's handshake.
    pub fn covers(&self, server_name: &str) -> bool {
        std::iter::once(&self.domain_name)
            .chain(self.subject_alternative_names.iter())
            .any(|name| {
                ::service_common::acm_certificate_key_material::name_covers(name, server_name)
            })
    }

    /// Apply everything the clock already owes this certificate at `now`: the initial
    /// domain-validation email, then the validation outcome.
    ///
    /// A certificate ACM reports as `PENDING_VALIDATION` is waiting on work ACM does
    /// on its own; the instant each piece of that work completes was fixed when the
    /// certificate was requested, so applying it here materialises a change the clock
    /// already made rather than a side effect of whoever happened to call.
    ///
    /// The two are applied in DUE ORDER, so the record passes through the same states
    /// it would have if somebody had called in between.
    pub fn materialize_due(&mut self, now: SystemTime) {
        // The managed renewal of an already issued certificate, which is owed
        // independently of the validation outcomes below: a renewal is started only on
        // an issued certificate, and such a certificate has no validation outcome left
        // to reach.
        self.materialize_renewal_due(now);
        if let Some(due) = self.pending_validation_email {
            // The outcome is applied first when it falls due first: a request ACM has
            // finished processing sends no validation mail afterwards.
            let outcome_first = self
                .pending_validation
                .as_ref()
                .is_some_and(|pending| pending.due_at < due);
            if now >= due && !outcome_first {
                self.pending_validation_email = None;
                self.dispatch_validation_email(due);
            }
        }
        let Some(pending) = self
            .pending_validation
            .clone()
            .filter(|pending| now >= pending.due_at)
        else {
            return;
        };
        self.pending_validation = None;
        // The request has reached an outcome, so no validation mail is owed for it any
        // more.
        self.pending_validation_email = None;
        match pending.outcome {
            ValidationOutcome::Issued => self.issue(pending.due_at),
            ValidationOutcome::Failed => {
                self.fail(FAILURE_REASON_ADDITIONAL_VERIFICATION_REQUIRED);
            }
            ValidationOutcome::TimedOut => self.time_out(),
        }
    }

    /// Apply the managed renewal the clock already owes this certificate at `now`.
    ///
    /// A renewal ACM reports as `PENDING_AUTO_RENEWAL` is work ACM does on its own; the
    /// instant it completes was fixed by the call that started it, so applying it here
    /// materialises a change the clock already made. The renewed certificate's validity
    /// window opens at that instant — "the time at which the certificate was issued"
    /// (`CertificateDetail$IssuedAt` doc) is the issuance the renewal performed — and
    /// nothing else about the certificate changes.
    ///
    /// A certificate that stopped being issued while the renewal was owed — a
    /// revocation is permanent (`RevokeCertificate` doc) — has nothing left to renew, so
    /// the owed renewal is dropped rather than handing that record a fresh validity
    /// window: "the reason that a renewal request was unsuccessful"
    /// (`RenewalSummary$RenewalStatusReason` doc) is what the summary then reports it
    /// as.
    fn materialize_renewal_due(&mut self, now: SystemTime) {
        let Some(renewed_at) = self
            .renewal
            .as_ref()
            .and_then(|renewal| renewal.due_at)
            .filter(|due_at| now >= *due_at)
        else {
            return;
        };
        let still_issued = self.status == STATUS_ISSUED;
        if let Some(renewal) = self.renewal.as_mut() {
            renewal.due_at = None;
            renewal.status = if still_issued {
                RENEWAL_STATUS_SUCCESS.to_string()
            } else {
                RENEWAL_STATUS_FAILED.to_string()
            };
            renewal.updated_at = renewed_at;
        }
        if !still_issued {
            return;
        }
        self.issued_at = Some(renewed_at);
        self.not_before = Some(renewed_at);
        self.not_after = Some(renewed_at + CERTIFICATE_VALIDITY);
    }

    /// Whether ACM renews this certificate on request: "at this time, only exported
    /// private certificates can be renewed with the RenewCertificate command"
    /// (`CertificateDetail$RenewalEligibility` doc).
    ///
    /// A private certificate is one a private certificate authority issued (the record
    /// carries that authority's ARN); the export is what put a copy of it outside ACM,
    /// the same fact `RevokeCertificate` requires. A certificate that is no longer
    /// issued has nothing to renew.
    pub fn renewal_eligibility(&self) -> &'static str {
        if self.certificate_authority_arn.is_some() && self.exported && self.status == STATUS_ISSUED
        {
            RENEWAL_ELIGIBLE
        } else {
            RENEWAL_INELIGIBLE
        }
    }

    /// Whether a renewal of this certificate is still owed, so there is nothing to
    /// start: "the certificate request is in process" is what the request to start
    /// another one is answered with.
    pub fn renewal_in_progress(&self) -> bool {
        self.renewal
            .as_ref()
            .is_some_and(|renewal| renewal.due_at.is_some())
    }

    /// Record the managed renewal `RenewCertificate` started at `at`: it is owed from
    /// here on, and [`Self::materialize_renewal_due`] applies it once it falls due.
    pub fn start_renewal(&mut self, at: SystemTime) {
        self.renewal = Some(Renewal {
            status: RENEWAL_STATUS_PENDING_AUTO_RENEWAL.to_string(),
            updated_at: at,
            due_at: Some(at + RENEWAL_SETTLE_DELAY),
        });
    }

    /// Record the domain-validation email ACM sent at `at` for every name this
    /// certificate validates BY EMAIL.
    ///
    /// "ACM sends email to the the following five addresses: admin@<suffix>
    /// administrator@<suffix> hostmaster@<suffix> postmaster@<suffix>
    /// webmaster@<suffix>" (`ResendValidationEmailRequest$ValidationDomain` doc), at
    /// each name's own validation domain — the suffix the record already carries. A
    /// name validated by DNS or HTTP gets no mail, so its mailbox list is left as it is.
    ///
    /// `at` is the instant the mail went out, which is what the documented
    /// [`RESEND_VALIDATION_WINDOW`] is measured from — not the instant somebody
    /// happened to read the record.
    pub fn dispatch_validation_email(&mut self, at: SystemTime) {
        for validation in &mut self.domain_validations {
            if validation.validation_method != VALIDATION_METHOD_EMAIL {
                continue;
            }
            let Some(mail_suffix) = validation.validation_domain.clone() else {
                continue;
            };
            validation.validation_emails = validation_mailboxes(&mail_suffix);
        }
        self.validation_email_sent_at = Some(at);
    }

    /// Whether export was allowed for this certificate when it was created: "You can
    /// opt in to allow the export of your certificates by specifying ENABLED. You
    /// cannot update the value of Export after the the certificate is created"
    /// (`CertificateOptions$Export` doc).
    pub fn export_allowed(&self) -> bool {
        export_option(self) == EXPORT_ENABLED
    }

    /// Whether this certificate is in the state `RevokeCertificate` applies to.
    ///
    /// "Revokes a public ACM certificate. You can only revoke certificates that have
    /// been previously exported" and "revoking a certificate is permanent"
    /// (`RevokeCertificate` doc): the export is what put the private key outside ACM,
    /// and a certificate whose revocation is already recorded has nothing left to
    /// revoke.
    pub fn is_revocable(&self) -> bool {
        self.export_allowed() && self.exported && !self.is_revoked()
    }

    /// Whether this certificate's revocation is already recorded.
    pub fn is_revoked(&self) -> bool {
        self.status == STATUS_REVOKED || self.revoked_at.is_some()
    }

    /// Record this certificate as revoked at `at` for `reason`.
    ///
    /// The validity window and the issuance members stay as issued — the certificate
    /// was issued and still carries those facts; what changes is its status and the
    /// two members that exist only beside it.
    pub fn revoke(&mut self, at: SystemTime, reason: &str) {
        self.status = STATUS_REVOKED.to_string();
        self.revoked_at = Some(at);
        self.revocation_reason = Some(reason.to_string());
    }

    /// Record this certificate as issued at `at`: every domain is proven, the
    /// validity window opens, and the X.509 extensions an issued certificate carries
    /// come into existence.
    fn issue(&mut self, at: SystemTime) {
        self.status = STATUS_ISSUED.to_string();
        self.failure_reason = None;
        for validation in &mut self.domain_validations {
            validation.validation_status = DOMAIN_STATUS_SUCCESS.to_string();
        }
        self.issued_at = Some(at);
        self.not_before = Some(at);
        self.not_after = Some(at + CERTIFICATE_VALIDITY);
        self.key_usages = ISSUED_KEY_USAGES
            .iter()
            .map(|name| (*name).to_string())
            .collect();
        self.extended_key_usages = ISSUED_EXTENDED_KEY_USAGE_OIDS
            .iter()
            .map(|oid| extended_key_usage(oid))
            .collect();
    }

    /// Record this certificate's request as failed for `reason`: no domain was
    /// proven, and nothing was issued, so the issuance members stay absent.
    ///
    /// `ValidationEmails` is "a list of email addresses that ACM used to send domain
    /// validation emails" (`DomainValidation$ValidationEmails` doc) — a record of mail
    /// ACM HAS sent, which a request ending unproven does not retract, so each list is
    /// left exactly as the dispatches that wrote it left it. A request that failed
    /// before its mail went out never had one: the owed dispatch is dropped by
    /// [`Self::materialize_due`], so that list is still empty.
    fn fail(&mut self, reason: &str) {
        self.status = STATUS_FAILED.to_string();
        self.failure_reason = Some(reason.to_string());
        for validation in &mut self.domain_validations {
            validation.validation_status = DOMAIN_STATUS_FAILED.to_string();
        }
    }

    /// Record this certificate's request as timed out: "ACM makes repeated attempts to
    /// validate a certificate for 72 hours and then times out"
    /// (`CertificateDetail$Status` doc), so no domain was proven and nothing was
    /// issued.
    ///
    /// `FailureReason` stays absent — it "exists only when the certificate status is
    /// FAILED" (`CertificateDetail$FailureReason` doc) — and each domain's
    /// `ValidationStatus` is the `DomainStatus` of a validation that did not complete,
    /// exactly as in [`Self::fail`]. The mailbox lists are left as the dispatches that
    /// wrote them left them, for the same reason.
    fn time_out(&mut self) {
        self.status = STATUS_VALIDATION_TIMED_OUT.to_string();
        self.failure_reason = None;
        for validation in &mut self.domain_validations {
            validation.validation_status = DOMAIN_STATUS_FAILED.to_string();
        }
    }
}

/// The status transition the clock already owes an ACME endpoint.
///
/// Stored, never slept on: the instant it falls due is fixed by the call that started
/// the transition, so the endpoint reaches that status at that instant whether or not
/// anybody calls again.
#[derive(Debug, Clone)]
pub struct PendingAcmeEndpointStatus {
    /// When the transition falls due.
    pub due_at: SystemTime,
    /// The `AcmeEndpointStatus` the endpoint carries from then on.
    pub status: String,
}

impl PendingAcmeEndpointStatus {
    /// The transition owed to an endpoint created at `created_at`: it becomes
    /// [`ACME_ENDPOINT_STATUS_ACTIVE`] [`ACME_ENDPOINT_ACTIVATION_DELAY`] later.
    pub fn activating_at(created_at: SystemTime) -> Self {
        Self {
            due_at: created_at + ACME_ENDPOINT_ACTIVATION_DELAY,
            status: ACME_ENDPOINT_STATUS_ACTIVE.to_string(),
        }
    }
}

/// One stored ACME endpoint. Every field `DescribeAcmeEndpoint` reports is read from
/// here, so no response echoes its own request.
#[derive(Debug, Clone)]
pub struct AcmeEndpointRecord {
    /// `arn:aws:acm:<region>:<account>:acme-endpoint/<id>`.
    pub arn: String,
    /// The server-managed ACME directory URL ([`acme_endpoint_url`]).
    pub endpoint_url: String,
    /// An `AcmeEndpointStatus` value.
    pub status: String,
    /// "The reason the ACME endpoint failed, if applicable"
    /// (`AcmeEndpoint$FailureReason` doc), so it is set only beside a `FAILED` status.
    pub failure_reason: Option<String>,
    /// An `AcmeAuthorizationBehavior` value, as the create request configured it.
    pub authorization_behavior: String,
    /// An `AcmeContact` value, as the create request configured it.
    pub contact: String,
    /// The certificate authority the endpoint issues through.
    pub certificate_authority: CertificateAuthority,
    /// Tag key -> value applied to certificates issued through this endpoint.
    pub certificate_tags: BTreeMap<String, String>,
    /// Tag key -> value on the endpoint resource itself, as `ListTagsForResource`
    /// reports them.
    pub tags: BTreeMap<String, String>,
    /// The status transition the clock owes this endpoint. `None` once it has
    /// reached a status the clock owes nothing beyond.
    pub pending_status: Option<PendingAcmeEndpointStatus>,
    /// The anti-replay nonces this endpoint's ACME data plane has issued and not yet seen
    /// back, newest last and at most [`ACME_NONCE_WINDOW`] of them.
    ///
    /// It lives on the endpoint because a nonce is issued BY one endpoint's ACME server
    /// (RFC 8555 §7.2) and is spent by a JWS posted to that same server, so one
    /// endpoint's nonces are no use at another and are dropped with the endpoint itself.
    pub issued_nonces: VecDeque<String>,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
}

impl AcmeEndpointRecord {
    /// Record `nonce` as outstanding for this endpoint, forgetting the oldest once the
    /// window is full.
    ///
    /// Writing here is what `newNonce` DOES: "the server MUST include a Replay-Nonce
    /// header field containing a fresh nonce" (RFC 8555 §7.2), and a nonce nothing
    /// remembers is one no later POST could be checked against.
    pub fn issue_nonce(&mut self, nonce: String) {
        self.issued_nonces.push_back(nonce);
        while self.issued_nonces.len() > ACME_NONCE_WINDOW {
            self.issued_nonces.pop_front();
        }
    }

    /// Spend `nonce`: whether this endpoint had issued it and had not yet seen it back.
    ///
    /// The nonce is forgotten by the same call, which is what makes it single-use — "the
    /// server MUST consider each nonce valid only once" (RFC 8555 §6.5), so a replay of
    /// the same JWS finds nothing to spend.
    pub fn spend_nonce(&mut self, nonce: &str) -> bool {
        let Some(position) = self
            .issued_nonces
            .iter()
            .position(|issued| issued == nonce)
        else {
            return false;
        };
        self.issued_nonces.remove(position);
        true
    }

    /// Apply the status transition the clock already owes this endpoint at `now`.
    ///
    /// An endpoint ACM reports as `CREATING` is waiting on work ACM does on its own;
    /// the instant that work completes was fixed when the endpoint was created, so
    /// applying it here materialises a change the clock already made rather than a
    /// side effect of whoever happened to call.
    pub fn materialize_due(&mut self, now: SystemTime) {
        let Some(pending) = self
            .pending_status
            .clone()
            .filter(|pending| now >= pending.due_at)
        else {
            return;
        };
        self.pending_status = None;
        self.status = pending.status;
        // The endpoint changed at the instant the transition fell due, which is what
        // `UpdatedAt` ("the time at which the ACME endpoint was last updated") reports
        // — not the instant somebody happened to read it.
        self.updated_at = pending.due_at;
    }
}

/// Which call started the work an ACME domain validation is still in the middle of.
///
/// The two are refused differently when a caller asks to update the record mid-flight:
/// work the service started on its own is a precondition on the status the record
/// carries, while work ANOTHER caller's update started is that caller's modification
/// still in progress, which is the optimistic-concurrency conflict a client retries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcmeDomainValidationWork {
    /// The prevalidation the create started: the service proving the domain on its own.
    Prevalidation,
    /// The reconfiguration an `UpdateAcmeDomainValidation` call started, which is in
    /// flight until the prevalidation it re-opened falls due.
    Reconfiguration,
}

/// The status transition the clock already owes an ACME domain validation.
///
/// Stored, never slept on, for the same reason [`PendingAcmeEndpointStatus`] is: the
/// instant it falls due is fixed by the call that started the prevalidation, so the
/// validation reaches that status at that instant whether or not anybody calls again.
#[derive(Debug, Clone)]
pub struct PendingAcmeDomainValidationStatus {
    /// When the transition falls due.
    pub due_at: SystemTime,
    /// The `AcmeDomainValidationStatus` the validation carries from then on.
    pub status: String,
    /// What started the work this transition completes.
    pub work: AcmeDomainValidationWork,
}

impl PendingAcmeDomainValidationStatus {
    /// The transition owed to a validation whose prevalidation the create started at
    /// `started_at`: the prevalidation completes
    /// [`ACME_DOMAIN_VALIDATION_SETTLE_DELAY`] later and it becomes
    /// [`ACME_DOMAIN_VALIDATION_STATUS_VALID`].
    pub fn validating_at(started_at: SystemTime) -> Self {
        Self {
            due_at: started_at + ACME_DOMAIN_VALIDATION_SETTLE_DELAY,
            status: ACME_DOMAIN_VALIDATION_STATUS_VALID.to_string(),
            work: AcmeDomainValidationWork::Prevalidation,
        }
    }

    /// The same transition, owed to a validation whose prevalidation configuration an
    /// update replaced at `started_at` so the domain has to be proved again. It is the
    /// REQUESTING call's work, so it is what a second update racing it conflicts with.
    pub fn reconfigured_at(started_at: SystemTime) -> Self {
        Self {
            work: AcmeDomainValidationWork::Reconfiguration,
            ..Self::validating_at(started_at)
        }
    }
}

/// One stored ACME domain validation: the authorization of one ACME endpoint to issue
/// certificates for one domain name ("domain validations authorize the endpoint to
/// issue certificates for specified domain names", `CreateAcmeDomainValidation` doc).
///
/// Every field `DescribeAcmeDomainValidation` reports is read from here, so no response
/// echoes its own request — including the DNS record the caller must publish, which is
/// server-managed ([`acme_prevalidation_resource_record`]).
#[derive(Debug, Clone)]
pub struct AcmeDomainValidationRecord {
    /// `arn:aws:acm:<region>:<account>:acme-endpoint/<endpoint>/acme-domain-validation/<id>`
    /// ([`acme_domain_validation_arn`]).
    pub arn: String,
    /// The ACME endpoint this validation authorizes — the ARN prefix of [`Self::arn`],
    /// carried so the record reports it without re-parsing.
    pub acme_endpoint_arn: String,
    /// The domain name being validated, exactly as the create named it.
    pub domain_name: String,
    /// A `PrevalidationType` value: which `PrevalidationOptions` member the create
    /// carried.
    pub prevalidation_type: String,
    /// "The scope of domains covered by this prevalidation"
    /// (`DnsPrevalidationOptions$DomainScope` doc), as the create configured it.
    pub domain_scope: Option<DomainScope>,
    /// "The Route 53 hosted zone ID for DNS validation"
    /// (`DnsPrevalidationOptions$HostedZoneId` doc), as the create named it.
    pub hosted_zone_id: Option<String>,
    /// "The DNS resource record to create for domain validation"
    /// (`DnsPrevalidationDetails$ResourceRecord` doc) — minted by the service from this
    /// validation's own identity, never from anything the caller sent.
    pub resource_record: Option<ResourceRecord>,
    /// An `AcmeDomainValidationStatus` value.
    pub status: String,
    /// "Details about the failure, if the validation failed"
    /// (`AcmeDomainValidation$FailureDetails` doc), so it is set only beside an
    /// [`ACME_DOMAIN_VALIDATION_STATUS_INVALID`] status.
    pub failure_details: Option<FailureDetails>,
    /// Tag key -> value on the validation resource itself, as `ListTagsForResource`
    /// reports them.
    pub tags: BTreeMap<String, String>,
    /// The status transition the clock owes this validation. `None` once it has
    /// reached a status the clock owes nothing beyond.
    pub pending_status: Option<PendingAcmeDomainValidationStatus>,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
}

impl AcmeDomainValidationRecord {
    /// Whether another caller's update of this validation is still in flight: it
    /// re-opened the prevalidation and the transition that completes it has not fallen
    /// due yet.
    ///
    /// Read AFTER [`Self::materialize_due`], so a transition the clock has already made
    /// is no longer somebody's modification in progress.
    pub fn update_in_flight(&self) -> bool {
        self.pending_status
            .as_ref()
            .is_some_and(|pending| pending.work == AcmeDomainValidationWork::Reconfiguration)
    }

    /// Apply the status transition the clock already owes this validation at `now`.
    ///
    /// A validation ACM reports as `VALIDATING` is waiting on prevalidation work ACM
    /// does on its own; the instant that work completes was fixed by the call that
    /// started it, so applying it here materialises a change the clock already made
    /// rather than a side effect of whoever happened to call.
    pub fn materialize_due(&mut self, now: SystemTime) {
        let Some(pending) = self
            .pending_status
            .clone()
            .filter(|pending| now >= pending.due_at)
        else {
            return;
        };
        self.pending_status = None;
        self.status = pending.status;
        // The validation changed at the instant the transition fell due, which is what
        // `UpdatedAt` ("the time at which the domain validation was last updated")
        // reports — not the instant somebody happened to read it.
        self.updated_at = pending.due_at;
    }
}

/// What one `CreateAcmeDomainValidation` idempotency token has already created.
#[derive(Debug, Clone)]
pub struct AcmeDomainValidationIdempotency {
    /// A canonical rendering of every parameter the validation was created from, so a
    /// re-sent token can be told apart from a changed request carrying the same token.
    pub fingerprint: String,
    /// The ARN the first call answered with.
    pub acme_domain_validation_arn: String,
}

/// One stored ACME external account binding: the credentials "that authorize an ACME
/// client to register an account with the endpoint", bound to "an IAM role that controls
/// what certificate operations the ACME client can perform"
/// (`CreateAcmeExternalAccountBinding` doc).
///
/// Every field `DescribeAcmeExternalAccountBinding` reports is read from here, so no
/// response echoes its own request — including the credentials, which are minted by the
/// service ([`mint_acme_external_account_binding_mac_key`]) and are not members of any
/// request.
#[derive(Clone)]
pub struct AcmeExternalAccountBindingRecord {
    /// `arn:aws:acm:<region>:<account>:acme-endpoint/<endpoint>/acme-external-account-binding/<id>`
    /// ([`acme_external_account_binding_arn`]).
    pub arn: String,
    /// The ACME endpoint this binding registers accounts with — the ARN prefix of
    /// [`Self::arn`], carried so the record reports it without re-parsing.
    pub acme_endpoint_arn: String,
    /// "The Amazon Resource Name (ARN) of the IAM role associated with the external
    /// account binding" (`AcmeExternalAccountBinding$RoleArn` doc), exactly as the create
    /// named it.
    pub role_arn: String,
    /// The `kid` an ACME client presents in the external-account-binding JWS (RFC 8555
    /// §7.3.4), as `GetAcmeExternalAccountBindingCredentials` reports it. Server-minted:
    /// it is this binding's own id, so it is unique per binding, identical on every later
    /// read and carries nothing the caller sent.
    pub key_id: String,
    /// The HMAC key that JWS is signed with, as
    /// `GetAcmeExternalAccountBindingCredentials` reports it. Server-minted and
    /// immutable: no request carries one, and nothing rewrites it once the binding
    /// exists.
    pub mac_key: String,
    /// "The time at which the external account binding expires"
    /// (`AcmeExternalAccountBinding$ExpiresAt` doc) — the span the create's `Expiration`
    /// named after the instant it created the binding ([`expiration_span`]), and `None`
    /// for a create that named no expiration.
    pub expires_at: Option<SystemTime>,
    /// "The time at which the external account binding was revoked" (`$RevokedAt` doc),
    /// so it is set only for a binding that has been revoked.
    pub revoked_at: Option<SystemTime>,
    /// "The time at which the external account binding was last used" (`$LastUsedAt`
    /// doc) — set when an ACME client registers an account with these credentials, and
    /// `None` until one has.
    pub last_used_at: Option<SystemTime>,
    /// Tag key -> value on the binding resource itself, as `ListTagsForResource`
    /// reports them.
    pub tags: BTreeMap<String, String>,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
}

impl AcmeExternalAccountBindingRecord {
    /// Whether a revocation has already withdrawn the authority these credentials
    /// carried, which is exactly what [`Self::revoked_at`] records the instant of.
    pub fn is_revoked(&self) -> bool {
        self.revoked_at.is_some()
    }
}

/// Written by hand rather than derived, so the MAC key cannot travel into a formatter:
/// it is the secret an ACME client signs with, and a derived `Debug` would carry it into
/// anything that renders this record or the store holding it.
impl ::std::fmt::Debug for AcmeExternalAccountBindingRecord {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        f.debug_struct("AcmeExternalAccountBindingRecord")
            .field("arn", &self.arn)
            .field("acme_endpoint_arn", &self.acme_endpoint_arn)
            .field("role_arn", &self.role_arn)
            .field("key_id", &self.key_id)
            .field("mac_key", &"<redacted>")
            .field("expires_at", &self.expires_at)
            .field("revoked_at", &self.revoked_at)
            .field("last_used_at", &self.last_used_at)
            .field("tags", &self.tags)
            .field("created_at", &self.created_at)
            .field("updated_at", &self.updated_at)
            .finish()
    }
}

/// What one `CreateAcmeExternalAccountBinding` idempotency token has already created.
#[derive(Debug, Clone)]
pub struct AcmeExternalAccountBindingIdempotency {
    /// A canonical rendering of every parameter the binding was created from, so a
    /// re-sent token can be told apart from a changed request carrying the same token.
    pub fingerprint: String,
    /// The ARN the first call answered with.
    pub acme_external_account_binding_arn: String,
}

/// The number of characters an external account binding's MAC key carries.
///
/// 44 characters of the base64 alphabet are the encoding of exactly 33 bytes
/// (44 * 6 == 33 * 8), so the key needs no padding and has no trailing bits: a client
/// that decodes it reads those 33 bytes back, and re-encoding them answers the same 44
/// characters.
pub const ACME_MAC_KEY_LEN: usize = 44;

/// A fresh MAC key for one external account binding.
///
/// The value is a secret handed to an ACME client — it signs the external-account-binding
/// JWS of its registration with it (RFC 8555 §7.3.4) — but it is NOT a credential of this
/// emulator and must never be accepted as one. Nothing in a service may construct a
/// secret itself, so the key material is cut from a secret minted by the one component
/// that generates secrets, the emulator's credential store, which is then removed from
/// that store again: the client gets the key, the emulator keeps no credential it would
/// accept, and nothing accumulates per call.
///
/// Only the ASCII alphanumeric characters of that secret are kept, which are the
/// characters the standard and URL-safe base64 alphabets SHARE (RFC 4648 §§4-5 differ
/// only in the last two), so an ACME client decoding the key under either alphabet reads
/// the same [`ACME_MAC_KEY_LEN`]-character encoding of the same key bytes.
///
/// `None` when the store minted no secret, or one with fewer than [`ACME_MAC_KEY_LEN`]
/// usable characters, which leaves the operation to refuse rather than report a value it
/// made up.
pub fn mint_acme_external_account_binding_mac_key(
    store: &::emulator_server::CredentialStore,
) -> Option<String> {
    let minted = store.create(&::emulator_server::aws_credentials::AWS_SESSION);
    let key: String = minted
        .secret(::emulator_server::aws_credentials::SESSION_TOKEN)
        .unwrap_or_default()
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .take(ACME_MAC_KEY_LEN)
        .collect();
    // The MAC key is not a credential of this emulator; see above. A store that could
    // not drop the minted credential would leave one it DOES accept behind, whose secret
    // this key was cut from, so that is a failure to mint rather than a key to hand over.
    if store.delete(&minted.id).is_err() {
        return None;
    }
    (key.chars().count() == ACME_MAC_KEY_LEN).then_some(key)
}

/// One ACME account registered with one of this account's ACME endpoints.
///
/// "ACME accounts are created when clients use external account binding credentials
/// to register" (`ListAcmeAccounts` doc), i.e. by an ACME client's own `newAccount`
/// request to the endpoint's directory URL (RFC 8555 §7.3) — no operation in this
/// service's model creates one. So this map is written by whatever serves that
/// registration, and every ACME-account operation (`ListAcmeAccounts` today) REPORTS
/// these records rather than echoing its own request.
///
/// An account is identified by the key it registered with, which is why the record
/// carries the thumbprint of that key beside the `AccountUrl` the server assigned it.
#[derive(Debug, Clone)]
pub struct AcmeAccountRecord {
    /// The ACME endpoint the account is registered with: the ARN half of this
    /// record's key in [`ComAmazonawsAcmCertificatemanagerState::acme_accounts`].
    pub acme_endpoint_arn: String,
    /// The server-assigned URL of the account resource ([`acme_account_url`]), which
    /// is how every ACME-account operation addresses it.
    pub account_url: String,
    /// "The thumbprint of the public key associated with the ACME account"
    /// (`AcmeAccount$PublicKeyThumbprint` doc) — the account's identity.
    pub public_key_thumbprint: String,
    /// The account key itself, as the canonical JWK the thumbprint above is the digest of
    /// (`service_common::jwt::canonical_jwk`).
    ///
    /// Kept because an ACME account's authority IS that key: every later request the
    /// account makes is a JWS signed with it and naming the account's URL (RFC 8555
    /// §6.2), so the server has to hold the key to check one. Only the PUBLIC parameters
    /// are kept, which is all the canonical form carries.
    pub public_key_jwk: String,
    /// An `AcmeAccountStatus` value.
    pub status: String,
    /// The external account binding the registration presented, when it presented
    /// one (`AcmeAccount$AcmeExternalAccountBindingArn` doc).
    pub acme_external_account_binding_arn: Option<String>,
    /// The contact URIs the account registered with; empty when it registered none,
    /// which an endpoint whose `Contact` is `NOT_REQUIRED` admits.
    pub contacts: Vec<String>,
    pub created_at: SystemTime,
}

/// What one `CreateAcmeEndpoint` idempotency token has already created.
#[derive(Debug, Clone)]
pub struct AcmeEndpointIdempotency {
    /// A canonical rendering of every parameter the endpoint was created from, so a
    /// re-sent token can be told apart from a changed request carrying the same token.
    pub fingerprint: String,
    /// The ARN the first call answered with.
    pub acme_endpoint_arn: String,
}

/// What one `RequestCertificate` idempotency token has already created, and when the
/// index recorded it.
///
/// The instant is what lets the index SHRINK again: a token ACM no longer recognises —
/// one older than [`IDEMPOTENCY_TOKEN_TTL`] — stands for nothing, so
/// [`ComAmazonawsAcmCertificatemanagerState::expire_certificate_idempotency_tokens`]
/// drops it and the index cannot accumulate one entry per token a caller ever sent.
#[derive(Debug, Clone)]
pub struct CertificateIdempotency {
    /// The ARN the first call answered with.
    pub certificate_arn: String,
    /// When the index recorded this token; it times out [`IDEMPOTENCY_TOKEN_TTL`]
    /// later, the window `RequestCertificateRequest$IdempotencyToken` publishes ("if
    /// you call RequestCertificate multiple times with the same idempotency token
    /// within one hour").
    pub recorded_at: SystemTime,
}

impl CertificateIdempotency {
    /// Whether this token is still live at `now`.
    pub fn token_is_live(&self, now: SystemTime) -> bool {
        now.duration_since(self.recorded_at)
            .is_ok_and(|elapsed| elapsed < IDEMPOTENCY_TOKEN_TTL)
            // A record stamped in the future (a clock that stepped back) has not timed
            // out either.
            || self.recorded_at > now
    }
}

/// Per-service state for com-amazonaws-acm-certificatemanager. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsAcmCertificatemanagerState {
    /// Certificate ARN -> record. A `BTreeMap`, so an unsorted `ListCertificates`
    /// page and its `NextToken` walk one deterministic order.
    pub certificates: BTreeMap<String, Certificate>,
    /// Idempotency key (token plus a fingerprint of the request parameters) -> what
    /// that token created, so re-sending one `RequestCertificate` returns the
    /// certificate it already created instead of making another.
    ///
    /// BOUNDED three ways: an entry is only ever written beside a new certificate,
    /// [`Self::remove_certificate`] drops the entries pointing at a deleted one,
    /// [`Self::expire_certificate_idempotency_tokens`] drops the ones that have timed
    /// out, and [`CERTIFICATE_STORE_LIMIT`] caps what the index may hold whatever the
    /// request says.
    pub idempotency: HashMap<String, CertificateIdempotency>,
    /// ACME endpoint ARN -> record. A `BTreeMap`, so an unsorted `ListAcmeEndpoints`
    /// page and its `NextToken` walk one deterministic order.
    pub acme_endpoints: BTreeMap<String, AcmeEndpointRecord>,
    /// `CreateAcmeEndpoint` idempotency token -> what that token created. Separate
    /// from [`Self::idempotency`] because the two index different resources: a token
    /// resolves to the endpoint it created, and the two operations' tokens must not
    /// be able to collide.
    pub acme_endpoint_idempotency: HashMap<String, AcmeEndpointIdempotency>,
    /// (ACME endpoint ARN, account URL) -> the ACME account registered with that
    /// endpoint under that URL. Keyed by the pair because an ACME account is addressed
    /// by the two together (`DescribeAcmeAccount` and `RevokeAcmeAccount` both take
    /// `AcmeEndpointArn` + `AccountUrl`), and a `BTreeMap` on the pair so one
    /// endpoint's accounts are a contiguous range walked in one deterministic order —
    /// which is the order `ListAcmeAccounts` pages through
    /// ([`Self::acme_accounts_of`]).
    pub acme_accounts: BTreeMap<(String, String), AcmeAccountRecord>,
    /// ACME domain-validation ARN -> record. A `BTreeMap`, and every validation's ARN
    /// begins with its endpoint's ARN ([`acme_domain_validation_arn`]), so one
    /// endpoint's validations are a contiguous range walked in one deterministic order
    /// ([`Self::acme_domain_validations_of`]).
    pub acme_domain_validations: BTreeMap<String, AcmeDomainValidationRecord>,
    /// (ACME endpoint ARN, `CreateAcmeDomainValidation` idempotency token) -> what that
    /// token created. Keyed by the PAIR rather than a joined string, so no endpoint ARN
    /// and token can run together into another pair's key, and separate from the other
    /// token indexes because the three resolve to different resources.
    pub acme_domain_validation_idempotency:
        HashMap<(String, String), AcmeDomainValidationIdempotency>,
    /// ACME external-account-binding ARN -> record. A `BTreeMap`, and every binding's
    /// ARN begins with its endpoint's ARN ([`acme_external_account_binding_arn`]), so one
    /// endpoint's bindings are a contiguous range walked in one deterministic order
    /// ([`Self::acme_external_account_bindings_of`]) — which is the order
    /// `ListAcmeExternalAccountBindings` pages through.
    pub acme_external_account_bindings: BTreeMap<String, AcmeExternalAccountBindingRecord>,
    /// (ACME endpoint ARN, `CreateAcmeExternalAccountBinding` idempotency token) -> what
    /// that token created, keyed by the PAIR for the same reason
    /// [`Self::acme_domain_validation_idempotency`] is.
    pub acme_external_account_binding_idempotency:
        HashMap<(String, String), AcmeExternalAccountBindingIdempotency>,
    /// The account's expiry-events configuration, as the last successful
    /// `PutAccountConfiguration` wrote it, and `None` until one has — when ACM is
    /// running the account on the documented default,
    /// [`DEFAULT_DAYS_BEFORE_EXPIRY`].
    pub account_configuration: Option<AccountConfiguration>,
    /// `PutAccountConfiguration` idempotency token -> the request it was first sent
    /// with, for as long as that token stays live. Repeating a token with the same
    /// request is the same request; repeating it with another is a conflict.
    pub account_configuration_requests: HashMap<String, AccountConfigurationRequest>,
}

impl ComAmazonawsAcmCertificatemanagerState {
    /// Drop every `PutAccountConfiguration` idempotency token that has timed out by
    /// `now`, so the index holds only tokens ACM would still recognise and cannot
    /// grow without bound.
    pub fn expire_account_configuration_tokens(&mut self, now: SystemTime) {
        self.account_configuration_requests
            .retain(|_, request| request.token_is_live(now));
    }

    /// Drop every `RequestCertificate` idempotency entry that stands for nothing at
    /// `now`: one whose token has timed out ([`IDEMPOTENCY_TOKEN_TTL`]), and one whose
    /// certificate is gone.
    ///
    /// The index therefore holds at most one entry per LIVE token, rather than one per
    /// token any caller ever sent.
    pub fn expire_certificate_idempotency_tokens(&mut self, now: SystemTime) {
        let certificates = &self.certificates;
        self.idempotency.retain(|_, recorded| {
            recorded.token_is_live(now) && certificates.contains_key(&recorded.certificate_arn)
        });
    }

    /// Apply every validation outcome the clock already owes at `now`.
    ///
    /// Call it FIRST in every handler that locks the store, so each operation sees the
    /// state a caller would see after any other. One pass touches at most as many
    /// records as this store may hold ([`CERTIFICATE_STORE_LIMIT`],
    /// [`ACME_ENDPOINT_STORE_LIMIT`], [`ACME_DOMAIN_VALIDATION_STORE_LIMIT`]), and each
    /// is one instant comparison.
    ///
    /// The timed-out idempotency entries go with them: a token ACM would no longer
    /// recognise is one the clock has already retired, so the pass that materialises
    /// what the clock owes drops those too.
    pub fn materialize_due(&mut self, now: SystemTime) {
        for certificate in self.certificates.values_mut() {
            certificate.materialize_due(now);
        }
        for endpoint in self.acme_endpoints.values_mut() {
            endpoint.materialize_due(now);
        }
        for validation in self.acme_domain_validations.values_mut() {
            validation.materialize_due(now);
        }
        self.expire_certificate_idempotency_tokens(now);
    }

    /// The stored ACME endpoint the id `endpoint_id` names in `region`, or `None` when
    /// this account holds no such endpoint there.
    ///
    /// How a request to an endpoint's published ACME URL
    /// ([`acme_endpoint_url`]) resolves to the record behind it: the Region comes off
    /// the enrollment host the request addressed and the id off its path, and together
    /// they are the ARN this map is keyed by — so one Region's enrollment host serves
    /// only that Region's endpoints.
    pub fn acme_endpoint_in(&self, region: &str, endpoint_id: &str) -> Option<&AcmeEndpointRecord> {
        self.acme_endpoints
            .get(&acme_endpoint_arn(region, endpoint_id))
    }

    /// [`Self::acme_endpoint_in`] for a caller that writes the record — the ACME data
    /// plane, which records the nonces the endpoint it serves has issued.
    pub fn acme_endpoint_in_mut(
        &mut self,
        region: &str,
        endpoint_id: &str,
    ) -> Option<&mut AcmeEndpointRecord> {
        self.acme_endpoints
            .get_mut(&acme_endpoint_arn(region, endpoint_id))
    }

    /// The tag map of the non-certificate ACM resource `arn` names, or `None` when
    /// this account holds no such resource.
    ///
    /// The generic tagging operations (`TagResource`, `UntagResource`,
    /// `ListTagsForResource`) apply to "all ACM resource types except the certificate
    /// resource type", so resolution walks the non-certificate resources this service
    /// persists — today the ACME endpoints, keyed by the same ARN the request names —
    /// and a well-formed ARN naming anything else has no record behind it (the caller
    /// answers `ResourceNotFoundException`). A resource type added to this struct
    /// later becomes taggable by being resolved here, not by another copy of the
    /// tagging rules.
    pub fn resource_tags(&self, arn: &str) -> Option<&BTreeMap<String, String>> {
        self.acme_endpoints
            .get(arn)
            .map(|endpoint| &endpoint.tags)
            .or_else(|| {
                self.acme_domain_validations
                    .get(arn)
                    .map(|validation| &validation.tags)
            })
            .or_else(|| {
                self.acme_external_account_bindings
                    .get(arn)
                    .map(|binding| &binding.tags)
            })
    }

    /// [`Self::resource_tags`] for a caller that writes the map.
    pub fn resource_tags_mut(&mut self, arn: &str) -> Option<&mut BTreeMap<String, String>> {
        if let Some(endpoint) = self.acme_endpoints.get_mut(arn) {
            return Some(&mut endpoint.tags);
        }
        if let Some(validation) = self.acme_domain_validations.get_mut(arn) {
            return Some(&mut validation.tags);
        }
        self.acme_external_account_bindings
            .get_mut(arn)
            .map(|binding| &mut binding.tags)
    }

    /// Drop `arn`'s ACME endpoint record and every idempotency token that resolves to
    /// it, so a replayed `CreateAcmeEndpoint` can never answer with an ARN that has no
    /// record behind it.
    pub fn remove_acme_endpoint(&mut self, arn: &str) -> Option<AcmeEndpointRecord> {
        let removed = self.acme_endpoints.remove(arn)?;
        self.acme_endpoint_idempotency
            .retain(|_, seen| seen.acme_endpoint_arn != arn);
        // An ACME account exists only on the endpoint it registered with, so the
        // endpoint's accounts go with it: "after deletion, the endpoint URL is no
        // longer accessible" (`DeleteAcmeEndpoint` doc), and an account left behind
        // would be reachable through an ARN with no record behind it.
        self.acme_accounts
            .retain(|(endpoint_arn, _), _| endpoint_arn != arn);
        // A domain validation authorizes ONE endpoint and its ARN is a path under that
        // endpoint's, so it cannot outlive the endpoint either: the deletion cascades to
        // the endpoint's validations and to the tokens that resolve to them.
        let validations: Vec<String> = self
            .acme_domain_validations_of(arn)
            .map(|validation| validation.arn.clone())
            .collect();
        for validation_arn in validations {
            self.remove_acme_domain_validation(&validation_arn);
        }
        // An external account binding registers accounts with ONE endpoint and its ARN is
        // a path under that endpoint's, so it cannot outlive the endpoint either.
        let bindings: Vec<String> = self
            .acme_external_account_bindings_of(arn)
            .map(|binding| binding.arn.clone())
            .collect();
        for binding_arn in bindings {
            self.remove_acme_external_account_binding(&binding_arn);
        }
        Some(removed)
    }

    /// Every domain validation of `acme_endpoint_arn`, in the order
    /// `ListAcmeDomainValidations` pages through them (by validation ARN).
    ///
    /// A range scan over the key prefix every one of that endpoint's validation ARNs
    /// begins with, so the walk touches only that endpoint's validations however many
    /// other endpoints the store holds.
    pub fn acme_domain_validations_of<'a>(
        &'a self,
        acme_endpoint_arn: &'a str,
    ) -> impl Iterator<Item = &'a AcmeDomainValidationRecord> + 'a {
        let prefix = acme_domain_validation_arn_prefix(acme_endpoint_arn);
        self.acme_domain_validations
            .range(prefix.clone()..)
            .take_while(move |(arn, _)| arn.starts_with(&prefix))
            .map(|(_, validation)| validation)
    }

    /// The domain validation `acme_endpoint_arn` already holds for `domain_name`, if
    /// any.
    ///
    /// One endpoint authorizes one domain through ONE validation, and DNS labels are
    /// case-insensitive ([`same_domain`]), so the lookup is by domain rather than by the
    /// exact text the create carried.
    pub fn acme_domain_validation_for<'a>(
        &'a self,
        acme_endpoint_arn: &'a str,
        domain_name: &'a str,
    ) -> Option<&'a AcmeDomainValidationRecord> {
        self.acme_domain_validations_of(acme_endpoint_arn)
            .find(|validation| same_domain(&validation.domain_name, domain_name))
    }

    /// Drop `arn`'s domain-validation record and every idempotency token that resolves
    /// to it, so a replayed `CreateAcmeDomainValidation` can never answer with an ARN
    /// that has no record behind it.
    pub fn remove_acme_domain_validation(
        &mut self,
        arn: &str,
    ) -> Option<AcmeDomainValidationRecord> {
        let removed = self.acme_domain_validations.remove(arn)?;
        self.acme_domain_validation_idempotency
            .retain(|_, seen| seen.acme_domain_validation_arn != arn);
        Some(removed)
    }

    /// Every ACME account registered with `acme_endpoint_arn`, in the order
    /// `ListAcmeAccounts` pages through them (by account URL).
    ///
    /// A range scan over the key pair's first half, so the walk touches only that
    /// endpoint's accounts however many other endpoints the store holds.
    pub fn acme_accounts_of<'a>(
        &'a self,
        acme_endpoint_arn: &'a str,
    ) -> impl Iterator<Item = &'a AcmeAccountRecord> + 'a {
        self.acme_accounts
            .range((acme_endpoint_arn.to_string(), String::new())..)
            .take_while(move |((endpoint_arn, _), _)| endpoint_arn == acme_endpoint_arn)
            .map(|(_, account)| account)
    }

    /// The ACME account `account_url` names on `acme_endpoint_arn`, or `None` when that
    /// endpoint holds no account under that URL.
    ///
    /// The pair IS the key every ACME-account operation addresses an account by
    /// (`DescribeAcmeAccount` and `RevokeAcmeAccount` both take `AcmeEndpointArn` +
    /// `AccountUrl`), so one lookup serves them and the data plane alike.
    pub fn acme_account_for(
        &self,
        acme_endpoint_arn: &str,
        account_url: &str,
    ) -> Option<&AcmeAccountRecord> {
        self.acme_accounts
            .get(&(acme_endpoint_arn.to_string(), account_url.to_string()))
    }

    /// [`Self::acme_account_for`] for a caller that writes the record.
    pub fn acme_account_for_mut(
        &mut self,
        acme_endpoint_arn: &str,
        account_url: &str,
    ) -> Option<&mut AcmeAccountRecord> {
        self.acme_accounts
            .get_mut(&(acme_endpoint_arn.to_string(), account_url.to_string()))
    }

    /// The account `acme_endpoint_arn` already holds for the public key whose RFC 7638
    /// thumbprint is `public_key_thumbprint`, if any.
    ///
    /// An ACME account IS its key pair: "if the server already has an account registered
    /// with the provided account key, then it MUST return a response with a 200 (OK)
    /// status code and provide the URL of that account" (RFC 8555 §7.3), so a
    /// registration is resolved by key rather than by anything the request names.
    pub fn acme_account_by_thumbprint(
        &self,
        acme_endpoint_arn: &str,
        public_key_thumbprint: &str,
    ) -> Option<&AcmeAccountRecord> {
        // The same range scan [`Self::acme_accounts_of`] walks, written out because the
        // record outlives the borrow of the ARN this call was given.
        self.acme_accounts
            .range((acme_endpoint_arn.to_string(), String::new())..)
            .take_while(|((endpoint_arn, _), _)| endpoint_arn == acme_endpoint_arn)
            .map(|(_, account)| account)
            .find(|account| account.public_key_thumbprint == public_key_thumbprint)
    }

    /// Store `account`, unless storing it would pass [`ACME_ACCOUNT_LIMIT`] for its
    /// endpoint or [`ACME_ACCOUNT_STORE_LIMIT`] for this store; `false` when it would.
    ///
    /// The one place a registration is written, so both bounds are checked once and the
    /// store cannot grow past them however many times a client registers.
    pub fn register_acme_account(&mut self, account: AcmeAccountRecord) -> bool {
        if self.acme_accounts.len() >= ACME_ACCOUNT_STORE_LIMIT
            || self.acme_accounts_of(&account.acme_endpoint_arn).count() >= ACME_ACCOUNT_LIMIT
        {
            return false;
        }
        self.acme_accounts.insert(
            (
                account.acme_endpoint_arn.clone(),
                account.account_url.clone(),
            ),
            account,
        );
        true
    }

    /// Every external account binding of `acme_endpoint_arn`, in the order
    /// `ListAcmeExternalAccountBindings` pages through them (by binding ARN).
    ///
    /// A range scan over the key prefix every one of that endpoint's binding ARNs begins
    /// with, so the walk touches only that endpoint's bindings however many other
    /// endpoints the store holds.
    pub fn acme_external_account_bindings_of<'a>(
        &'a self,
        acme_endpoint_arn: &'a str,
    ) -> impl Iterator<Item = &'a AcmeExternalAccountBindingRecord> + 'a {
        let prefix = acme_external_account_binding_arn_prefix(acme_endpoint_arn);
        self.acme_external_account_bindings
            .range(prefix.clone()..)
            .take_while(move |(arn, _)| arn.starts_with(&prefix))
            .map(|(_, binding)| binding)
    }

    /// Drop `arn`'s external-account-binding record and every idempotency token that
    /// resolves to it, so a replayed `CreateAcmeExternalAccountBinding` can never answer
    /// with an ARN that has no record behind it.
    pub fn remove_acme_external_account_binding(
        &mut self,
        arn: &str,
    ) -> Option<AcmeExternalAccountBindingRecord> {
        let removed = self.acme_external_account_bindings.remove(arn)?;
        self.acme_external_account_binding_idempotency
            .retain(|_, seen| seen.acme_external_account_binding_arn != arn);
        Some(removed)
    }

    /// Drop `arn`'s record and every idempotency entry that resolves to it, so a
    /// replayed `RequestCertificate` can never answer with an ARN that has no
    /// record behind it.
    pub fn remove_certificate(&mut self, arn: &str) -> Option<Certificate> {
        let removed = self.certificates.remove(arn)?;
        self.idempotency
            .retain(|_, recorded| recorded.certificate_arn != arn);
        Some(removed)
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsAcmCertificatemanagerState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsAcmCertificatemanagerState>)
}

/// The SHARED certificate-association directory in the shared engine: call it with a
/// handler's `&*self.engine`.
///
/// Which resources reference a certificate is a fact about the OTHER services' resources
/// — a custom domain name, a listener, a distribution — each of which lives in the store
/// of the service that owns it, so this service cannot see them. The owning service
/// records its resource's references in ONE directory under one shared type
/// ([`::service_common::acm_certificate_associations::CertificateAssociationDirectory`]),
/// which is how `DescribeCertificate` can report `InUseBy`, `ListCertificates` and
/// `SearchCertificates` can report `InUse`, and `DeleteCertificate` and
/// `RevokeCertificate` can refuse a certificate another service is still serving. The
/// engine keys state by TYPE, so a copy of its own inside
/// [`ComAmazonawsAcmCertificatemanagerState`] would be invisible to the writers.
///
/// LOCK ORDER: a reader takes [`service_state`] first and this directory second, and
/// nothing holds this one while taking another store, so the order cannot invert.
///
/// Answer `None` with a modelled server error, never with a fresh directory.
pub fn certificate_association_directory<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<
    ::engine::ServiceState<
        ::service_common::acm_certificate_associations::CertificateAssociationDirectory,
    >,
> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(
            ::engine::InMemoryEngine::state::<
                ::service_common::acm_certificate_associations::CertificateAssociationDirectory,
            >,
        )
}

/// The SHARED certificate key-material directory in the shared engine: call it with a
/// handler's `&*self.engine`.
///
/// A certificate this service holds is PRESENTED by another service — a load balancer
/// listener terminates a connection with it — and the handshake that service runs needs
/// the certificate's own bytes and the private key that matches them, which only a caller
/// that imports key material brings. Those are held in ONE directory under one shared type
/// ([`::service_common::acm_certificate_key_material::CertificateKeyMaterialDirectory`]),
/// written here and read by the presenting service; the engine keys state by TYPE, so a
/// copy of its own inside [`ComAmazonawsAcmCertificatemanagerState`] would be invisible to
/// the reader.
///
/// LOCK ORDER: a writer takes [`service_state`] first and this directory second, and
/// nothing holds this one while taking another store, so the order cannot invert.
///
/// Answer `None` by leaving the directory unwritten: the material is an aid to the service
/// that presents the certificate, never part of this operation's own answer, so an engine
/// carrying no directory still serves the call.
pub fn certificate_key_material_directory<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<
    ::engine::ServiceState<
        ::service_common::acm_certificate_key_material::CertificateKeyMaterialDirectory,
    >,
> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(
            ::engine::InMemoryEngine::state::<
                ::service_common::acm_certificate_key_material::CertificateKeyMaterialDirectory,
            >,
        )
}

/// The region to mint ARNs in: the one the caller signed for, else [`DEFAULT_REGION`].
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| !region.is_empty())
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// `arn:aws:acm:<region>:<account>:certificate/<certificate_id>`.
/// The ARN resource type every ACM certificate ARN carries
/// (`arn:aws:acm:<region>:<account>:certificate/<id>`): the one spelling `CertificateArn`'s
/// own `@pattern` admits, so it is both what this service MINTS and what it names the
/// collection an operation that resolves no single certificate reaches.
pub const CERTIFICATE_RESOURCE_TYPE: &str = "certificate";

pub fn certificate_arn(region: &str, certificate_id: &str) -> String {
    ::engine::emulator_arn(
        SIGNING_NAME,
        region,
        &format!("{CERTIFICATE_RESOURCE_TYPE}/{certificate_id}"),
    )
}

/// The ARN resource type every ACME endpoint ARN carries, as `AcmeEndpointArn`'s own
/// `@pattern` fixes it (`...:[0-9]{12}:acme-endpoint/[a-zA-Z0-9-]+$`): the one spelling
/// the member admits, so it is both what this service MINTS and what it names a request
/// whose ARN it will not resolve.
pub const ACME_ENDPOINT_RESOURCE_TYPE: &str = "acme-endpoint";

/// `arn:aws:acm:<region>:<account>:acme-endpoint/<endpoint_id>`, the form
/// `AcmeEndpointArn`'s `@pattern` requires
/// (`^arn:aws[a-z-]*:acm:[a-z0-9-]+:[0-9]{12}:acme-endpoint/[a-zA-Z0-9-]+$`).
pub fn acme_endpoint_arn(region: &str, endpoint_id: &str) -> String {
    ::engine::emulator_arn(
        SIGNING_NAME,
        region,
        &format!("{ACME_ENDPOINT_RESOURCE_TYPE}/{endpoint_id}"),
    )
}

/// The `acme-domain-validation` segment every ACME domain-validation ARN carries
/// between its endpoint's ARN and its own id, as `AcmeDomainValidationArn`'s
/// `@pattern` requires
/// (`^arn:aws[a-z-]*:acm:[a-z0-9-]+:[0-9]{12}:acme-endpoint/[a-zA-Z0-9-]+/acme-domain-validation/[a-zA-Z0-9-]+$`).
pub const ACME_DOMAIN_VALIDATION_SEGMENT: &str = "acme-domain-validation";

/// `arn:aws:acm:<region>:<account>:acme-endpoint/<endpoint>/acme-domain-validation/<id>`.
///
/// Built by extending the ENDPOINT's own ARN, which is what the pattern above says the
/// ARN of a domain validation is: a path under the endpoint it authorizes. So the two
/// ARNs cannot disagree about the Region or the account, and the store keyed by this
/// ARN groups one endpoint's validations into one contiguous range
/// ([`acme_domain_validation_arn_prefix`]).
pub fn acme_domain_validation_arn(acme_endpoint_arn: &str, validation_id: &str) -> String {
    format!(
        "{}{validation_id}",
        acme_domain_validation_arn_prefix(acme_endpoint_arn)
    )
}

/// The prefix every domain-validation ARN of `acme_endpoint_arn` begins with, and
/// nothing else does: the endpoint's ARN plus the
/// [`ACME_DOMAIN_VALIDATION_SEGMENT`] path. An endpoint id admits no `/`
/// (`[a-zA-Z0-9-]+`), so one endpoint's prefix is never a prefix of another's.
pub fn acme_domain_validation_arn_prefix(acme_endpoint_arn: &str) -> String {
    format!("{acme_endpoint_arn}/{ACME_DOMAIN_VALIDATION_SEGMENT}/")
}

/// The `ResourceNotFoundException` message for an ACME domain-validation ARN that no
/// stored record answers to, built from the ARN the request asked for.
///
/// `ResourceNotFoundException` carries one modelled member (`message: String`) and the
/// model documents no text for it, so the sentence follows the template every other
/// lookup in this service renders ([`certificate_not_found_message`]): one complete
/// sentence naming the requested ARN and terminated by a period.
pub fn acme_domain_validation_not_found_message(acme_domain_validation_arn: &str) -> String {
    format!("Could not find ACME domain validation {acme_domain_validation_arn}.")
}

/// The `ConflictException` message for a second domain validation of a domain one ACME
/// endpoint already validates, built from the domain name the request asked for.
///
/// Recorded behaviour fixes the whole sentence around that one variable part: the name
/// the caller submitted, and nothing of the validation that already holds it — a refused
/// create is told what it asked for, not the identifier of a resource it did not create.
pub fn acme_domain_validation_exists_message(domain_name: &str) -> String {
    format!("An ACME domain validation for {domain_name} already exists.")
}

/// The `ValidationException` message for an update of a domain validation the service is
/// itself still working through, built from the status the record carries.
///
/// Recorded behaviour fixes the sentence around that one variable part: the status is
/// read from the resource's own state, and the request's ARN is not reflected into the
/// refusal.
pub fn acme_domain_validation_status_blocks_update_message(status: &str) -> String {
    format!("Cannot update ACME domain validation in {status} status.")
}

/// The `ConflictException` message for an update that raced another caller's update of
/// the same domain validation, built from the ARN the request named.
///
/// `ConflictException` is the retryable signal ("wait for the previous operation to
/// finish and try again"), so the sentence asks for the retry.
pub fn acme_domain_validation_concurrent_modification_message(
    acme_domain_validation_arn: &str,
) -> String {
    format!(
        "Concurrent modification of ACME domain validation {acme_domain_validation_arn}. \
         Please try again."
    )
}

/// The `ConflictException` message for an `@idempotencyToken` re-sent with parameters
/// that are not the ones it already created a resource from.
///
/// FIXED text, shared by every ACME create that indexes its token: the sentence states
/// the rule the refusal is made by and interpolates nothing — not the token, not a
/// parameter name, and not the resource the first call created, whose identifier the
/// mismatched request never asked for.
pub const IDEMPOTENCY_TOKEN_REUSED_MESSAGE: &str =
    "Idempotency token was already used with different request parameters";

/// The `acme-external-account-binding` segment every ACME external-account-binding ARN
/// carries between its endpoint's ARN and its own id, as
/// `AcmeExternalAccountBindingArn`'s `@pattern` requires
/// (`^arn:aws[a-z-]*:acm:[a-z0-9-]+:[0-9]{12}:acme-endpoint/[a-zA-Z0-9-]+/acme-external-account-binding/[a-zA-Z0-9-]+$`).
pub const ACME_EXTERNAL_ACCOUNT_BINDING_SEGMENT: &str = "acme-external-account-binding";

/// `arn:aws:acm:<region>:<account>:acme-endpoint/<endpoint>/acme-external-account-binding/<id>`.
///
/// Built by extending the ENDPOINT's own ARN, which is what the pattern above says the
/// ARN of an external account binding is: a path under the endpoint it registers
/// accounts with. So the two ARNs cannot disagree about the Region or the account, and
/// the store keyed by this ARN groups one endpoint's bindings into one contiguous range
/// ([`acme_external_account_binding_arn_prefix`]).
pub fn acme_external_account_binding_arn(acme_endpoint_arn: &str, binding_id: &str) -> String {
    format!(
        "{}{binding_id}",
        acme_external_account_binding_arn_prefix(acme_endpoint_arn)
    )
}

/// The prefix every external-account-binding ARN of `acme_endpoint_arn` begins with, and
/// nothing else does: the endpoint's ARN plus the
/// [`ACME_EXTERNAL_ACCOUNT_BINDING_SEGMENT`] path. An endpoint id admits no `/`
/// (`[a-zA-Z0-9-]+`), so one endpoint's prefix is never a prefix of another's.
pub fn acme_external_account_binding_arn_prefix(acme_endpoint_arn: &str) -> String {
    format!("{acme_endpoint_arn}/{ACME_EXTERNAL_ACCOUNT_BINDING_SEGMENT}/")
}

/// The `ResourceNotFoundException` message for an ACME external-account-binding ARN that
/// no stored record answers to, built from the ARN the request asked for.
///
/// `ResourceNotFoundException` carries one modelled member (`message: String`) and the
/// model documents no text for it, so the sentence follows the template every other
/// lookup in this service renders ([`certificate_not_found_message`]): one complete
/// sentence naming the requested ARN and terminated by a period.
pub fn acme_external_account_binding_not_found_message(
    acme_external_account_binding_arn: &str,
) -> String {
    format!(
        "Could not find ACME external account binding {acme_external_account_binding_arn}."
    )
}

/// The `ValidationException` message for a request that asks for the credentials of an
/// ACME external account binding whose revocation has already withdrawn them.
///
/// `ValidationException` carries one modelled member (`message: String`) and the model
/// documents no text for it, so the sentence follows the template every refusal in this
/// service renders: one complete sentence naming the requested ARN and the state that
/// refuses it, terminated by a period.
pub fn acme_external_account_binding_revoked_message(
    acme_external_account_binding_arn: &str,
) -> String {
    format!(
        "The ACME external account binding {acme_external_account_binding_arn} has been revoked."
    )
}

/// The leading label of the host ACM serves its managed ACME endpoints on.
///
/// ACM publishes ONE enrollment host per Region and gives every endpoint in that
/// Region a URL on it: the recorded behaviour grades `AcmeEndpoint$EndpointUrl` as
/// being on `acm-acme-enroll.<region>.<dns suffix>`. The endpoint's own id therefore
/// identifies it in the PATH ([`acme_endpoint_base_url`]) and never as a DNS label —
/// an ACME client is configured with this URL as its ACME server, so a host of a form
/// the service does not publish is one the client's TLS/SNI, and any host-based
/// routing or allow-list built around the enrollment host, cannot match.
pub const ACME_ENROLLMENT_HOST_LABEL: &str = "acm-acme-enroll";

/// The DNS suffix of the AWS endpoint hosts this emulator publishes.
pub const AWS_DNS_SUFFIX: &str = "amazonaws.com";

/// The path the ACME resources of every endpoint hang off: the ROOT of the enrollment
/// host, since an endpoint's own id is the first path segment and nothing of this
/// service's own naming sits between the host and that id.
///
/// It carries no service-specific segment, so it does no filtering on the parse side: what
/// tells an ACME resource path apart from any other path on this host is that its first
/// segment is an endpoint id and a resource follows it ([`parse_acme_request_path`]). The
/// constant remains so the minting and the parsing of a path are written against one
/// place, which is what keeps the URLs this service publishes and the URLs it resolves the
/// same set.
pub const ACME_PATH_ROOT: &str = "/";

/// The ACME `directory` resource: the single URL an ACME client is configured with and
/// starts from, which answers the resource map (RFC 8555 §7.1.1).
pub const ACME_DIRECTORY_RESOURCE: &str = "directory";

/// The ACME `newNonce` resource: it answers a fresh anti-replay nonce in a
/// `Replay-Nonce` header (RFC 8555 §7.2).
pub const ACME_NEW_NONCE_RESOURCE: &str = "new-nonce";

/// The ACME `newAccount` resource, where a client registers an account (RFC 8555 §7.3).
pub const ACME_NEW_ACCOUNT_RESOURCE: &str = "new-account";

/// The ACME `newOrder` resource, where a client asks for a certificate (RFC 8555 §7.4).
pub const ACME_NEW_ORDER_RESOURCE: &str = "new-order";

/// The ACME `revokeCert` resource (RFC 8555 §7.6).
pub const ACME_REVOKE_CERTIFICATE_RESOURCE: &str = "revoke-cert";

/// The ACME `keyChange` resource, where an account rolls its key over (RFC 8555 §7.3.5).
pub const ACME_KEY_CHANGE_RESOURCE: &str = "key-change";

/// The resource-type segment of an ACME account's own URL ([`acme_account_url`]).
pub const ACME_ACCOUNT_RESOURCE: &str = "acct";

/// `acm-acme-enroll.<region>.amazonaws.com`, the host ACM serves the Region's ACME
/// endpoints on ([`ACME_ENROLLMENT_HOST_LABEL`]).
pub fn acme_enrollment_host(region: &str) -> String {
    format!("{ACME_ENROLLMENT_HOST_LABEL}.{region}.{AWS_DNS_SUFFIX}")
}

/// The Region whose ACME enrollment host `host` is, or `None` for any other host.
///
/// The inverse of [`acme_enrollment_host`], so the host this service PUBLISHES is the
/// host it recognises: the first label must be [`ACME_ENROLLMENT_HOST_LABEL`], the
/// second is the Region, and at least one further non-empty label must follow it (the
/// DNS suffix). The Region is held to the character set `AcmeEndpointArn`'s own
/// `@pattern` allows for it (`[a-z0-9-]+`), because it is then the Region half of the
/// ARN the request's endpoint record is looked up by.
///
/// Any userinfo and port are removed first, as they are part of the authority rather
/// than of the host name.
pub fn acme_enrollment_region(host: &str) -> Option<&str> {
    let host = host.rsplit('@').next().unwrap_or(host);
    let host = host.split(':').next().unwrap_or(host);
    let mut labels = host.split('.');
    if labels.next() != Some(ACME_ENROLLMENT_HOST_LABEL) {
        return None;
    }
    let region = labels.next()?;
    let suffix: Vec<&str> = labels.collect();
    if suffix.is_empty() || suffix.iter().any(|label| label.is_empty()) {
        return None;
    }
    is_acme_region(region).then_some(region)
}

/// Whether `region` is a Region name an ACM ARN may carry
/// (`AcmeEndpointArn`'s `@pattern` spells the Region field `[a-z0-9-]+`).
fn is_acme_region(region: &str) -> bool {
    !region.is_empty()
        && region
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// Whether `endpoint_id` is an endpoint id an ACM ACME endpoint ARN may carry
/// (`AcmeEndpointArn`'s `@pattern` spells the id `[a-zA-Z0-9-]+`).
pub fn is_acme_endpoint_id(endpoint_id: &str) -> bool {
    !endpoint_id.is_empty()
        && endpoint_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

/// The base URL of one endpoint's ACME surface:
/// `https://acm-acme-enroll.<region>.amazonaws.com/<endpoint_id>`.
///
/// Every ACME resource of that endpoint hangs off it, which is why the endpoint's own
/// id sits here — in the path on the Region's one enrollment host — rather than in the
/// host name.
pub fn acme_endpoint_base_url(region: &str, endpoint_id: &str) -> String {
    format!(
        "https://{}{ACME_PATH_ROOT}{endpoint_id}",
        acme_enrollment_host(region)
    )
}

/// The URL of one ACME resource of one endpoint: its base URL
/// ([`acme_endpoint_base_url`]) and the resource beneath it.
pub fn acme_resource_url(region: &str, endpoint_id: &str, resource: &str) -> String {
    format!("{}/{resource}", acme_endpoint_base_url(region, endpoint_id))
}

/// The ACME directory URL an endpoint serves on.
///
/// `AcmeEndpoint$EndpointUrl` is documented only as "The URL of the ACME endpoint",
/// and neither `CreateAcmeEndpoint` nor `UpdateAcmeEndpoint` models a member a caller
/// could propose one with, so the URL is SERVER-MANAGED: it is derived from the
/// endpoint's own id and Region, is unique per endpoint, is identical on every later
/// read, and carries nothing the caller sent. It is minted on the Region's ACME
/// enrollment host ([`ACME_ENROLLMENT_HOST_LABEL`]) and ends at the `directory`
/// resource, the single URL an ACME client is given and starts from (RFC 8555 §7.1.1)
/// — which is the URL this service SERVES (`crate::service::acme_http`).
pub fn acme_endpoint_url(region: &str, endpoint_id: &str) -> String {
    acme_resource_url(region, endpoint_id, ACME_DIRECTORY_RESOURCE)
}

/// The URL of the ACME account resource an endpoint assigned a registration.
///
/// `AcmeAccount$AccountUrl` is documented only as "The URL of the ACME account", and
/// nothing in this service's model lets a caller propose one, so what matters is the
/// invariant: the URL is SERVER-ASSIGNED, sits under the same endpoint base URL as the
/// endpoint's own directory URL ([`acme_endpoint_url`]), and is unique per account. It
/// names the `account` resource an ACME server hands back in the `Location` header of
/// a successful `newAccount` (RFC 8555 §7.3), which the client then uses as its
/// account URL.
pub fn acme_account_url(region: &str, endpoint_id: &str, account_id: &str) -> String {
    acme_resource_url(
        region,
        endpoint_id,
        &format!("{ACME_ACCOUNT_RESOURCE}/{account_id}"),
    )
}

/// What a request path on an ACME enrollment host names: the endpoint it addresses,
/// and the resource of that endpoint beneath it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcmeRequestPath {
    /// The endpoint id in the path, which is the id half of the endpoint's ARN.
    pub endpoint_id: String,
    /// Everything after the endpoint id: `directory`, `new-nonce`, `acct/<id>`, …
    pub resource: String,
}

/// The endpoint and resource an enrollment-host path names, or `None` when the path is
/// not one this service publishes.
///
/// The inverse of [`acme_resource_url`]: the paths recognised are exactly the paths
/// minted, so a URL this service hands out is a URL it can resolve back to an endpoint
/// record.
pub fn parse_acme_request_path(path: &str) -> Option<AcmeRequestPath> {
    let rest = path.strip_prefix(ACME_PATH_ROOT)?;
    let (endpoint_id, resource) = rest.split_once('/')?;
    if !is_acme_endpoint_id(endpoint_id) || resource.is_empty() {
        return None;
    }
    Some(AcmeRequestPath {
        endpoint_id: endpoint_id.to_string(),
        resource: resource.to_string(),
    })
}

/// The emulator's OWN endpoint a request reached, as this service's `Host`-carrying
/// callers reach it — `service_common`'s one reading of it, read off the buffered request
/// parts a handler is handed.
///
/// A URL this service hands a client has to resolve back to where that client is already
/// talking, or it names an address the client cannot reach: an ACME client is a library
/// that takes the directory URL and dials it, and rewrites no host. A caller that reached
/// this emulator at its own endpoint resolves no AWS hostname, so what it is told is that
/// endpoint ([`acme_resource_url_at`]).
pub fn reached_emulator_endpoint_of_parts(parts: &::http::request::Parts) -> Option<String> {
    ::service_common::data_plane_addressing::reached_emulator_endpoint(
        &parts.uri.to_string(),
        parts
            .headers
            .get(::http::header::HOST)
            .and_then(|value| value.to_str().ok()),
    )
}

/// The base URL of one endpoint's ACME surface on the emulator's own `endpoint`:
/// `https://<endpoint>/_aws/acm-acme-enroll/<region>/<endpoint_id>`.
///
/// It carries exactly what the published address carries, with the Region moved from the
/// enrollment host into the path: the plane is named by [`ACME_ENROLLMENT_HOST_LABEL`]
/// behind the reserved path segment every data plane this emulator serves on its own
/// endpoint sits under (`service_common::data_plane_addressing`), so a path under that
/// segment naming another plane is never read as this one's, and the Region and the
/// endpoint id are the two halves of the endpoint's ARN.
pub fn acme_local_endpoint_base_url(endpoint: &str, region: &str, endpoint_id: &str) -> String {
    format!(
        "https://{endpoint}/{}/{ACME_ENROLLMENT_HOST_LABEL}/{region}/{endpoint_id}",
        ::service_common::data_plane_addressing::EMULATOR_PATH_NAMESPACE
    )
}

/// The URL of one ACME resource of one endpoint, at the address the caller being answered
/// reached this service on: the emulator's own endpoint when that is where the caller is
/// talking ([`reached_emulator_endpoint_of_parts`], [`acme_local_endpoint_base_url`]), and
/// the Region's published enrollment host ([`acme_resource_url`]) otherwise.
///
/// Every URL this service publishes — `AcmeEndpoint$EndpointUrl`, the resource map inside
/// the directory, and an account's own URL — is minted through this one function, so the
/// address the client is told is the address the answer it fetched came from.
pub fn acme_resource_url_at(
    local_endpoint: Option<&str>,
    region: &str,
    endpoint_id: &str,
    resource: &str,
) -> String {
    match local_endpoint {
        Some(endpoint) => format!(
            "{}/{resource}",
            acme_local_endpoint_base_url(endpoint, region, endpoint_id)
        ),
        None => acme_resource_url(region, endpoint_id, resource),
    }
}

/// [`acme_endpoint_url`] at the address the caller reached ([`acme_resource_url_at`]).
pub fn acme_endpoint_url_at(
    local_endpoint: Option<&str>,
    region: &str,
    endpoint_id: &str,
) -> String {
    acme_resource_url_at(local_endpoint, region, endpoint_id, ACME_DIRECTORY_RESOURCE)
}

/// [`acme_account_url`] at the address the caller reached ([`acme_resource_url_at`]).
pub fn acme_account_url_at(
    local_endpoint: Option<&str>,
    region: &str,
    endpoint_id: &str,
    account_id: &str,
) -> String {
    acme_resource_url_at(
        local_endpoint,
        region,
        endpoint_id,
        &format!("{ACME_ACCOUNT_RESOURCE}/{account_id}"),
    )
}

/// The Region, endpoint and resource a path on the EMULATOR's own endpoint names, or
/// `None` when the path is not one [`acme_resource_url_at`] mints there.
///
/// The inverse of [`acme_local_endpoint_base_url`], so what is recognised is exactly what
/// is published.
pub fn parse_acme_local_path(path: &str) -> Option<(String, AcmeRequestPath)> {
    let rest = acme_local_plane_path(path)?;
    let (region, rest) = ::service_common::data_plane_addressing::first_segment(rest)?;
    if !is_acme_region(region) {
        return None;
    }
    Some((region.to_string(), parse_acme_request_path(rest)?))
}

/// Whether `path` addresses this service's ACME plane on the emulator's own endpoint at
/// all — the reserved namespace segment and the plane that names this surface, whatever
/// follows them.
///
/// It is what the service CLAIMS such a request on: a path on this plane is this
/// service's to answer however the rest of it reads, so a client holding a URL this
/// service published is never answered by the front door's "unknown service".
pub fn addresses_acme_local_plane(path: &str) -> bool {
    acme_local_plane_path(path).is_some()
}

/// The path BENEATH this service's ACME plane prefix, or `None` for a path that is not on
/// it: the one reading of that prefix, by exact segment equality.
fn acme_local_plane_path(path: &str) -> Option<&str> {
    let (namespace, rest) = ::service_common::data_plane_addressing::first_segment(path)?;
    if namespace != ::service_common::data_plane_addressing::EMULATOR_PATH_NAMESPACE {
        return None;
    }
    let (plane, rest) = ::service_common::data_plane_addressing::first_segment(rest)?;
    (plane == ACME_ENROLLMENT_HOST_LABEL).then_some(rest)
}

/// The `ResourceNotFoundException` message for an ACME endpoint ARN that no stored
/// record answers to, built from the ARN the request asked for.
///
/// `ResourceNotFoundException` carries one modelled member (`message: String`) and the
/// model documents no text for it, so the sentence follows the template every other
/// lookup in this service renders ([`certificate_not_found_message`]): one complete
/// sentence naming the requested ARN and terminated by a period.
pub fn acme_endpoint_not_found_message(acme_endpoint_arn: &str) -> String {
    format!("Could not find ACME endpoint {acme_endpoint_arn}.")
}

/// The resource part of an ACM ARN
/// (`arn:<partition>:acm:<region>:<account>:<type>/<id>`), or `None` when `arn` is not
/// an ACM ARN at all.
///
/// Every ACM resource is addressed by an ARN of that one form, so the framing is parsed
/// in one place and each resource type only names its own `<type>/` prefix.
fn acm_arn_resource(arn: &str) -> Option<&str> {
    acm_arn_fields(arn).map(|(_region, _account, resource)| resource)
}

/// The Region segment of an ACM ARN, or `None` when `arn` is not an ACM ARN at all.
///
/// Every ARN this service mints carries the Region the resource lives in
/// (`^arn:aws[a-z-]*:acm:[a-z0-9-]+:...`), which is the half of the framing that says
/// WHERE the resource is — and for an ACME endpoint the Region whose enrollment host its
/// ACME surface is published on ([`acme_endpoint_url`]).
pub fn acm_arn_region(arn: &str) -> Option<&str> {
    acm_arn_fields(arn).map(|(region, _account, _resource)| region)
}

/// The account segment of an ACM ARN, or `None` when `arn` is not an ACM ARN at all.
///
/// Every ARN a caller addresses an ACM resource by carries the account that holds it
/// (`^arn:aws[a-z-]*:acm:[a-z0-9-]+:[0-9]{12}:...`), so this is the half of the framing
/// that says WHOSE resource the ARN names.
pub fn acm_arn_account(arn: &str) -> Option<&str> {
    acm_arn_fields(arn).map(|(_region, account, _resource)| account)
}

/// Whether `arn` names a resource of the account this emulator serves.
///
/// An ARN carries the account that holds the resource, and a caller can address only
/// their own: an ARN whose account segment is another account's names nothing this
/// caller could reach, whatever is in the store. Judged against the one account every
/// ARN this emulator mints carries (`::engine::emulator_account_id`), never against a
/// literal, so this agrees with [`acme_endpoint_arn`] and [`certificate_arn`] by
/// construction.
pub fn arn_is_in_caller_account(arn: &str) -> bool {
    acm_arn_account(arn).is_some_and(|account| account == ::engine::emulator_account_id())
}

/// The Region, the account and the resource part of an ACM ARN
/// (`arn:<partition>:acm:<region>:<account>:<type>/<id>`), or `None` when `arn` is not
/// an ACM ARN at all.
fn acm_arn_fields(arn: &str) -> Option<(&str, &str, &str)> {
    let mut fields = arn.split(':');
    let (
        Some("arn"),
        Some(partition),
        Some(SIGNING_NAME),
        Some(region),
        Some(account),
        Some(resource),
        None,
    ) = (
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
    )
    else {
        return None;
    };
    if partition.is_empty() || !account.bytes().all(|b| b.is_ascii_digit()) || account.is_empty() {
        return None;
    }
    Some((region, account, resource))
}

/// The id in an ACM ARN whose resource type is `resource_type`, or `None` when `arn`
/// names something else.
fn acm_resource_id<'arn>(arn: &'arn str, resource_type: &str) -> Option<&'arn str> {
    let id = acm_arn_resource(arn)?.strip_prefix(resource_type)?;
    let id = id.strip_prefix('/')?;
    (!id.is_empty() && !id.contains('/')).then_some(id)
}

/// The resource TYPE in an ACM ARN (`certificate`, `acme-endpoint`,
/// `acme-domain-validation`, …), or `None` when `arn` is not an ACM ARN or its resource
/// section names no type.
///
/// The counterpart of [`acm_resource_id`] for a caller that has to report which kind of
/// resource an ARN addressed rather than look one type up — the ARN is the only place
/// that type comes from, so the reporting path reads it here rather than guessing from
/// which store answered.
///
/// An ACM resource section is a `<type>/<id>` path, and a sub-resource extends its
/// parent's with another such pair (`acme-endpoint/<id>/acme-domain-validation/<id>`),
/// so the type is the segment immediately before the trailing id — the type of the
/// resource the ARN NAMES, not of the parent it hangs under.
pub fn acm_arn_resource_type(arn: &str) -> Option<&str> {
    let (path, id) = acm_arn_resource(arn)?.rsplit_once('/')?;
    let resource_type = path.rsplit('/').next()?;
    (!resource_type.is_empty() && !id.is_empty()).then_some(resource_type)
}

/// The certificate id in an ACM certificate ARN, or `None` when `arn` names
/// something that is not one (the caller answers `InvalidArnException`).
///
/// The model's `@pattern` already refused anything that is not an `acm` ARN before a
/// handler runs; what is left to check here is that the resource is a `certificate`.
pub fn certificate_id_from_arn(arn: &str) -> Option<&str> {
    acm_resource_id(arn, CERTIFICATE_RESOURCE_TYPE)
}

/// The endpoint id in an ACME endpoint ARN, or `None` when `arn` names something that
/// is not one.
///
/// The counterpart of [`certificate_id_from_arn`] for the other resource type this
/// service persists, and what tells an opaque `ListAcmeEndpoints` pagination token this
/// service issued apart from one it did not.
pub fn acme_endpoint_id_from_arn(arn: &str) -> Option<&str> {
    acm_resource_id(arn, ACME_ENDPOINT_RESOURCE_TYPE)
}

/// The `InvalidArnException` message for an ARN that is not an ACM certificate ARN,
/// built from the ARN the request asked for.
///
/// `InvalidArnException` carries one modelled member (`message: String`) and the model
/// documents no text for it, so the sentence comes from ACM's recorded behaviour: the
/// requested ARN followed by `is not a valid ARN`. Rendered here so every
/// certificate-lookup path reports the same sentence.
pub fn invalid_arn_message(certificate_arn: &str) -> String {
    format!("{certificate_arn} is not a valid ARN")
}

/// The `ResourceInUseException` message for a call that would take a certificate away
/// from a resource still referencing it, built from the ARN the request asked for.
///
/// `ResourceInUseException` carries one modelled member (`message: String`) and the model
/// documents no text for it, so the sentence comes from ACM's recorded behaviour: one
/// complete sentence naming the requested certificate and terminated by a period. It
/// names no referencing resource — which resources those are is what
/// `DescribeCertificate`'s `InUseBy` reports — and every operation that refuses on an
/// association renders it here, so the template cannot drift between them.
pub fn certificate_in_use_message(certificate_arn: &str) -> String {
    format!("Certificate {certificate_arn} is in use.")
}

/// The `ResourceNotFoundException` message for an ACM certificate ARN that no stored
/// record answers to, built from the ARN the request asked for.
///
/// `ResourceNotFoundException` carries one modelled member (`message: String`) and the
/// model documents no text for it, so the sentence comes from ACM's recorded
/// behaviour: one complete sentence naming the requested ARN and terminated by a
/// period. Every operation that resolves a certificate ARN renders it here, so the
/// template cannot drift between the lookup paths.
pub fn certificate_not_found_message(certificate_arn: &str) -> String {
    format!("Could not find certificate {certificate_arn}.")
}

/// The `ResourceNotFoundException` message for a lookup that resolves a DOMAIN inside a
/// certificate the account holds — a certificate whose domain validations do not cover
/// the name the request asked for.
///
/// Built from the request, as the certificate-ARN form above is: what is missing is not
/// the certificate but the pair the request named, so the sentence names the
/// certificate, the domain it was asked for and the account holding it — read off the
/// requested ARN, which carries that account, so the sentence cannot name an account the
/// request did not address. Rendered here so every domain-scoped lookup reports one
/// template.
pub fn certificate_domain_not_found_message(certificate_arn: &str, domain_name: &str) -> String {
    let account =
        acm_arn_account(certificate_arn).unwrap_or_else(|| ::engine::emulator_account_id());
    format!(
        "Could not find certificate {certificate_arn} with domain {domain_name} in account \
         {account}"
    )
}

/// The `ResourceNotFoundException` message for an ACM resource ARN that names no
/// resource this account holds, built from the ARN the request asked for.
///
/// Rendered by the operations that address an ACM resource by type-agnostic ARN
/// (`TagResource`, `UntagResource`, `ListTagsForResource`), so the sentence names the
/// resource TYPE the ARN itself carries ([`acm_arn_resource_type`]) — which is what
/// says WHICH kind of ACM resource was missing when the member admits several.
/// `ResourceNotFoundException` carries one modelled member (`message: String`) and the
/// model documents no text for it, so the sentence follows the template every other
/// lookup in this service renders ([`certificate_not_found_message`]): one complete
/// sentence naming the type and the requested ARN, terminated by a period. An ARN whose
/// resource section carries no type at all is named by the generic noun, since there is
/// no type in the request to report.
pub fn resource_not_found_message(resource_arn: &str) -> String {
    let resource_type = acm_arn_resource_noun(resource_arn);
    format!("Could not find {resource_type} {resource_arn}.")
}

/// How a sentence about one ARN names the kind of resource it addressed: the resource
/// TYPE the ARN itself carries ([`acm_arn_resource_type`]), and the generic noun for an
/// ARN whose resource section carries no type at all, since there is then no type in the
/// request to report.
///
/// Read by every sentence that has to say WHICH kind of resource an ARN member named —
/// a lookup that found none ([`resource_not_found_message`]) and a verdict on the ARN
/// itself — so the two cannot name one resource type differently.
pub fn acm_arn_resource_noun(arn: &str) -> &str {
    acm_arn_resource_type(arn).unwrap_or(UNTYPED_RESOURCE_NOUN)
}

/// How a sentence names a resource whose ARN carries no resource type to report.
const UNTYPED_RESOURCE_NOUN: &str = "resource";

/// The `ValidationException` message for a CERTIFICATE ARN handed to one of the
/// generic resource-tagging operations, naming the certificate-specific operation to
/// use instead.
///
/// Those operations are documented for "all ACM resource types except the certificate
/// resource type", each pointing at its certificate counterpart
/// (`TagResource` -> `AddTagsToCertificate`, `UntagResource` ->
/// `RemoveTagsFromCertificate`, `ListTagsForResource` -> `ListTagsForCertificate`), so
/// a certificate ARN is input the operation's own constraints do not admit — which is
/// what `ValidationException` ("the supplied input failed to satisfy constraints")
/// names.
///
/// `ValidationException` carries one modelled member (`message: String`) and the model
/// documents no text for it, so the sentences come from ACM's recorded behaviour: three
/// of them, stating in turn which resource types the CALLED operation supports, that the
/// resource the request named is a certificate, and the certificate-specific operation
/// to call instead. The variable parts are the called operation, the requested ARN and
/// that counterpart operation; everything else is fixed. Rendered here so all three
/// refusals read the same.
pub fn certificate_resource_message(
    operation: &str,
    resource_arn: &str,
    certificate_operation: &str,
) -> String {
    format!(
        "{operation} only supports non-certificate resource types. Resource {resource_arn} is a \
         certificate resource. Use {certificate_operation} API instead."
    )
}

/// The `ValidationException` message `UpdateCertificateOptions` answers for a request
/// that directs issuance work ACM has yet to perform on a certificate that is not
/// [`STATUS_ISSUED`].
///
/// `Options.ValidationMethod` is "the domain validation method for the certificate" and
/// `Options.Export` the export "you cannot update … after the the certificate is
/// created" (`CertificateOptions` member docs): both direct how ACM carries out the
/// issuance of the certificate the request names, so naming either for a record ACM has
/// not issued is input this operation's constraints do not admit — which is what
/// `ValidationException` reports ("the supplied input failed to satisfy constraints of
/// an Amazon Web Services service"). A request that asks only about the issued
/// certificate's own record draws the certificate's processing state instead
/// ([`CERTIFICATE_NOT_ISSUED_OR_VALID_MESSAGE`]). `ValidationException` carries one
/// modelled member (`message: String`) and the model documents no text for it, so the
/// sentence comes from ACM's recorded behaviour for this refusal: ONE constant sentence
/// naming neither the certificate nor the status it is in, and terminated by no period.
pub const CERTIFICATE_NOT_ISSUED_MESSAGE: &str = "Certificate is not issued";

/// The `InvalidStateException` message `UpdateCertificateOptions` answers for a request
/// that asks only about an issued certificate's own record — the transparency-logging
/// preference — over a certificate that is not [`STATUS_ISSUED`].
///
/// `CertificateTransparencyLoggingPreference` describes the certificate ACM has already
/// logged ("all public certificates are recorded in a certificate transparency log",
/// `CertificateOptions` doc) rather than directing issuance work still to come, so the
/// request itself is admissible and what rules it out is that the certificate's
/// processing has produced no issued certificate to reconfigure — "processing has
/// reached an invalid state" (`InvalidStateException` doc). `InvalidStateException`
/// carries one modelled member (`message: String`) and the model documents no text for
/// it, so the sentence comes from ACM's recorded behaviour for this error class: ONE
/// constant sentence naming neither the certificate nor the status it is in, terminated
/// by a period.
pub const CERTIFICATE_NOT_ISSUED_OR_VALID_MESSAGE: &str =
    "The selected certificate is not Issued and/or Valid.";

/// The `RequestInProgressException` message for a request that asks for the
/// certificate material of a resource this service holds none for.
///
/// `RequestInProgressException` carries one modelled member (`message: String`), and
/// the model documents the whole error class in one sentence — "The certificate request
/// is in process and the certificate in your account has not yet been issued"
/// (`RequestInProgressException` doc) — so that sentence, terminated by a period, is
/// what the refusal reports. Rendered here so every path that reports a request ACM is
/// still working on, without naming a certificate it has no material for, reports the
/// same sentence.
pub const REQUEST_IN_PROGRESS_MESSAGE: &str =
    "The certificate request is in process and the certificate in your account has not yet \
     been issued.";

/// The `RequestInProgressException` message for a request that asks for the material of
/// a certificate ACM has not issued, built from the ARN the request asked for.
///
/// `RequestInProgressException` carries one modelled member (`message: String`) and the
/// model documents no text for it, so the sentence comes from ACM's recorded behaviour
/// for this condition: `Certificate `, the requested ARN, ` in account ` and the
/// account that ARN names, then ` not yet issued`, with no terminating period. The
/// account is read off the ARN the caller addressed — the field that says whose
/// certificate it is, as [`certificate_domain_not_found_message`] reads it — so the
/// sentence can never name an account the request did not address. Rendered here so
/// every path that holds no issued material for a certificate ARN reports one template.
pub fn certificate_not_yet_issued_message(certificate_arn: &str) -> String {
    let account =
        acm_arn_account(certificate_arn).unwrap_or_else(|| ::engine::emulator_account_id());
    format!("Certificate {certificate_arn} in account {account} not yet issued")
}

/// The refusal sentence for a pagination token the service did not issue, as the
/// certificate-scoped reads of this service word it.
///
/// A `NextToken` is opaque and belongs to the page that issued it, so a token this
/// service cannot read as one of its own names no position to resume from. The
/// sentence comes from ACM's recorded behaviour: one constant sentence for the whole
/// class, naming neither the token nor what was wrong with it — a caller cannot
/// repair an opaque value, it can only restart the walk. Rendered here so every
/// paginated read that answers this class reports the same sentence.
///
/// The ACME reads answer the SAME class in the pagination framework's own words instead
/// ([`pagination_token_message`]), which is what they are recorded doing.
pub const INVALID_NEXT_TOKEN_MESSAGE: &str = "Invalid next token.";

/// The message the ACME reads of this service refuse an unreadable `NextToken` with: the
/// AWS pagination framework's sentence, naming the operation the token was presented to
/// and the wrapping schema that token's own payload carries.
///
/// Fixed text around two variable parts, both taken from the request: the operation name
/// and [`payload_wrapping_schema`]. Nothing of the token itself travels in it, so an
/// opaque value a caller sent is not echoed back.
pub fn pagination_token_message(operation: &str, next_token: &str) -> String {
    format!(
        "Pagination token exception in operation '{operation}': invalid payload wrapping \
         schema {}",
        payload_wrapping_schema(next_token)
    )
}

/// The wrapping schema a pagination token's own payload carries: the first 16 bits of the
/// payload its leading base64url characters encode, read as a big-endian SIGNED integer.
///
/// That field is where a wrapped pagination payload states which schema it was written
/// under, and it is the value the decoder reports back when it does not know the schema it
/// read — so the number is a function of the token presented and of nothing else. Only the
/// token's first base64 group is read, which is what carries those bits: a token shorter
/// than one group is read as if padded with zero bits ([`ZERO_SEXTET`]), and one whose
/// leading characters are not base64 at all carries no payload to read a schema out of,
/// which is the zero that answer reports.
fn payload_wrapping_schema(next_token: &str) -> i16 {
    let mut leading: String = next_token.chars().take(BASE64_GROUP).collect();
    while leading.chars().count() < BASE64_GROUP {
        leading.push(ZERO_SEXTET);
    }
    let octets = ::service_common::jwt::decode_base64url(&leading).unwrap_or_default();
    i16::from_be_bytes([
        octets.first().copied().unwrap_or_default(),
        octets.get(1).copied().unwrap_or_default(),
    ])
}

/// The characters one base64 group spells three octets in (RFC 4648 §4).
const BASE64_GROUP: usize = 4;

/// The base64 character for six zero bits, which a token shorter than one group is read
/// as if padded with.
const ZERO_SEXTET: char = 'A';

/// The byte that separates the two halves of a parent-scoped pagination cursor
/// ([`encode_page_cursor`]). It is outside every ARN pattern this service mints, and
/// outside a domain name or an `https` URL, so it cannot occur inside either value it
/// joins.
pub const PAGE_CURSOR_SEPARATOR: char = '\u{1f}';

/// The opaque `NextToken` for the position a page of one parent resource's
/// sub-resources stopped at.
///
/// It carries the PARENT the page was cut from as well as the position in it, so a
/// token issued for one parent cannot resume another's list, where the same position
/// would name a different place. Hex-encoded, so the token is opaque to the caller and
/// satisfies the `^[\u0009\u000A\u000D\u0020-\u00FF]*$` pattern a `NextToken` member
/// carries whatever the two values contain.
///
/// Rendered here so every paginated read that walks a parent's sub-resources in one
/// key's order issues ONE token form, with [`decode_page_cursor`] as its only reader.
/// (The certificate listings page in an order built from several keys at once, so they
/// frame their own cursors.)
pub fn encode_page_cursor(parent: &str, position: &str) -> String {
    hex::encode(format!("{parent}{PAGE_CURSOR_SEPARATOR}{position}"))
}

/// The position inside a `NextToken` [`encode_page_cursor`] issued for `parent`, or
/// `None` when the token did not come from it for that parent — which every caller
/// refuses with [`INVALID_NEXT_TOKEN_MESSAGE`].
///
/// Every step is fallible on purpose: the token is caller-supplied, so no value read
/// out of it may panic a handler or silently resume somebody else's list.
pub fn decode_page_cursor(token: &str, parent: &str) -> Option<String> {
    let decoded = String::from_utf8(hex::decode(token).ok()?).ok()?;
    let (named_parent, position) = decoded.split_once(PAGE_CURSOR_SEPARATOR)?;
    (named_parent == parent && !position.is_empty()).then(|| position.to_string())
}

/// The `ValidationException` message for an operation asked of a certificate export
/// was never allowed for, built from the ARN the request asked for.
///
/// Export is opt-in at creation (`CertificateOptions$Export` doc), and an operation
/// that only applies to an exported certificate — `RevokeCertificate`: "You can only
/// revoke certificates that have been previously exported" — cannot apply to one that
/// may not be exported at all. `ValidationException` carries one modelled member
/// (`message: String`) and the model documents no text for it, so the sentence comes
/// from ACM's recorded behaviour: `Export not allowed for certificate` followed by the
/// requested ARN. Rendered here so every export-eligibility path reports the same
/// sentence.
pub fn export_not_allowed_message(certificate_arn: &str) -> String {
    format!("Export not allowed for certificate {certificate_arn}")
}

/// The `ValidationException` message `ExportCertificate` answers for a certificate
/// export was never allowed for, built from the ARN the request asked for.
///
/// `ValidationException` carries one modelled member (`message: String`) and the model
/// documents no text for it, so the sentence comes from ACM's recorded behaviour for
/// this error class: `Certificate ARN: ` and the requested ARN, followed by ` is not an
/// export enabled certificate`. It is the export surface's own sentence, distinct from
/// the one [`export_not_allowed_message`] renders for an operation that requires a
/// certificate to have BEEN exported.
pub fn not_export_enabled_message(certificate_arn: &str) -> String {
    format!("Certificate ARN: {certificate_arn} is not an export enabled certificate")
}

/// The `ResourceNotFoundException` message `ExportCertificate` answers for a
/// certificate ARN that resolves to a record which has not been ISSUED, built from the
/// ARN the request asked for.
///
/// The member names "an Amazon Resource Name (ARN) of the issued certificate"
/// (`ExportCertificateRequest$CertificateArn` doc), so what is not found under the ARN
/// of a record in any status other than [`STATUS_ISSUED`] is an issued certificate —
/// the resource this argument addresses — even though the ARN itself
/// resolves, exactly as [`not_an_imported_certificate_message`] reports for the other
/// operation whose argument addresses one kind of certificate.
/// `ResourceNotFoundException` carries one modelled member (`message: String`) and the
/// model documents no text for it, so the sentence comes from ACM's recorded behaviour
/// for this error class: `Could not find an issued certificate ` followed by the
/// requested ARN.
pub fn issued_certificate_not_found_message(certificate_arn: &str) -> String {
    format!("Could not find an issued certificate {certificate_arn}")
}

/// The `ValidationException` message `RevokeCertificate` answers for a certificate ACM
/// did not issue, built from the ARN the request asked for.
///
/// "Revokes a public ACM certificate" (`RevokeCertificate` doc): a certificate whose
/// material came from outside ACM is no certificate ACM issued, so it is not one of
/// the public certificates this operation revokes. `ValidationException` carries one
/// modelled member (`message: String`) and the model documents no text for it, so the
/// sentence comes from ACM's recorded behaviour for this error class: `Certificate `,
/// the requested ARN, and ` is not a public certificate.`
pub fn not_a_public_certificate_message(certificate_arn: &str) -> String {
    format!("Certificate {certificate_arn} is not a public certificate.")
}

/// The `ResourceNotFoundException` message `ImportCertificate` answers for a
/// `CertificateArn` that resolves to a certificate ACM issued, built from the ARN the
/// request asked for.
///
/// The member names "an imported certificate to replace"
/// (`ImportCertificateRequest$CertificateArn` doc), so what is not found at an
/// ACM-issued certificate's ARN is an IMPORTED certificate — the resource this
/// argument addresses — even though the ARN itself resolves.
/// `ResourceNotFoundException` carries one modelled member (`message: String`) and the
/// model documents no text for it, so the sentence comes from ACM's recorded behaviour
/// for this error class: `The certificate ARN `, the requested ARN, and ` is not an
/// imported certificate ARN.`
pub fn not_an_imported_certificate_message(certificate_arn: &str) -> String {
    format!("The certificate ARN {certificate_arn} is not an imported certificate ARN.")
}

/// The `InvalidStateException` message `ResendValidationEmail` answers for a
/// certificate that is not awaiting domain validation, built from the ARN the request
/// asked for.
///
/// There is validation mail to resend only while ACM is still waiting for proof of
/// domain control, which is the [`STATUS_PENDING_VALIDATION`] status.
/// `InvalidStateException` carries one modelled member (`message: String`) and the
/// model documents no text for it, so the sentence comes from ACM's recorded behaviour
/// for this error class: `Certificate `, the requested ARN, and ` is not in pending
/// validation state`.
pub fn not_pending_validation_message(certificate_arn: &str) -> String {
    format!("Certificate {certificate_arn} is not in pending validation state")
}

/// The `InvalidStateException` message `UpdateCertificateOptions` answers for a
/// certificate ACM did not issue.
///
/// `CertificateOptions` configures ACM's own issuance of a certificate — the
/// transparency logging of a public certificate it issues, the export it allows, the
/// method it validates by — and a certificate whose material came from outside ACM has
/// no such issuance to configure. `InvalidStateException` carries one modelled member
/// (`message: String`) and the model documents no text for it, so the sentence comes
/// from ACM's recorded behaviour for this error class: ONE constant sentence naming the
/// operation and the kind of certificate it does not apply to, and neither the
/// certificate nor its status.
pub const IMPORTED_CERTIFICATE_OPTIONS_MESSAGE: &str =
    "The UpdateCertificateOptions command is not permitted for imported certificates.";

/// The `ValidationException` message `UpdateCertificateOptions` answers for a request
/// that asks for an `Export` value other than the one the certificate carries.
///
/// "You cannot update the value of Export after the the certificate is created"
/// (`CertificateOptions$Export` doc), so the member is fixed at creation.
/// `ValidationException` carries one modelled member (`message: String`) and the model
/// documents no text for it, so the sentence comes from ACM's recorded behaviour for
/// this refusal: ONE constant sentence naming the member that cannot be updated, and
/// neither the certificate, nor the value in force, nor the one asked for, and
/// terminated by no period.
pub const EXPORT_OPTION_NOT_UPDATABLE_MESSAGE: &str =
    "Export option for certificates cannot be updated";

/// The `ValidationException` message `RenewCertificate` answers for a certificate ACM
/// does not renew, built from the ARN the request asked for.
///
/// "Renews an ELIGIBLE ACM certificate" (`RenewCertificate` doc), and "only exported
/// private certificates can be renewed with the RenewCertificate command"
/// (`CertificateDetail$RenewalEligibility` doc), so a certificate whose
/// `RenewalEligibility` is [`RENEWAL_INELIGIBLE`] is one this operation does not apply
/// to. `ValidationException` carries one modelled member (`message: String`) and the
/// model documents no text for it, so the sentence comes from ACM's recorded behaviour
/// for this error class: `Certificate ARN `, the requested ARN, and ` is for a public
/// or imported certificate and not eligible for the Renew Certificate API` — the two
/// kinds of certificate that are not an exported private one.
pub fn not_renewal_eligible_message(certificate_arn: &str) -> String {
    format!(
        "Certificate ARN {certificate_arn} is for a public or imported certificate and not \
         eligible for the Renew Certificate API"
    )
}

/// A certificate's serial number as ACM renders it: the bytes of the X.509 serial the
/// signing authority assigned, each as exactly two lowercase hex digits — so a byte
/// below `0x10` keeps its leading zero — joined by colons.
///
/// "The serial number of the certificate" (`CertificateDetail$Serial` doc) is modelled
/// as a plain string, and this is the rendering ACM publishes it in: the byte-wise form
/// a serial is compared in everywhere else it appears (a revocation list, an
/// OpenSSL-printed certificate, the authority's own record). Rendered here so every
/// path that stores a serial stores the one form every reader reports.
pub fn serial_number(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<Vec<_>>()
        .join(":")
}

/// A wall-clock instant as the modelled `TStamp`.
pub fn to_date_time(at: SystemTime) -> ::aws_smithy_types::DateTime {
    let since_epoch = at
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    ::aws_smithy_types::DateTime::from_secs_and_nanos(
        i64::try_from(since_epoch.as_secs()).unwrap_or(i64::MAX),
        since_epoch.subsec_nanos(),
    )
}

/// A fingerprint of the parameters one request builds a record from, for an operation
/// that honours an `@idempotencyToken`: re-sending a token must answer with what it
/// already created, and the same token on a DIFFERENT request must be told apart from
/// it rather than answering 200 while the change is discarded.
///
/// The parts are hashed as GROUPS: each group behind its own part COUNT, and every part
/// inside it behind its own length. That is what makes the framing unambiguous — no
/// value can move between groups, or run into its neighbour, and leave the fingerprint
/// unchanged. A group marker pushed in-band as a part could not do that, since every
/// part is caller-controlled text.
///
/// Both operations that key an index on a token (`RequestCertificate`,
/// `CreateAcmeEndpoint`) render it here, so the framing lives in one place.
pub fn fingerprint_groups(groups: &[&[&str]]) -> String {
    use sha2::Digest;

    let mut hasher = sha2::Sha256::new();
    for group in groups {
        hasher.update(group.len().to_le_bytes());
        for part in *group {
            hasher.update(part.len().to_le_bytes());
            hasher.update(part.as_bytes());
        }
    }
    hex::encode(hasher.finalize())
}

/// The validation label ACM publishes for one `(certificate, domain, purpose)`:
/// 32 lowercase hex characters, the shape of the labels in the `ResourceRecord` doc
/// (`_a79865eb4cd1a6ab990a45779b4e0b96`).
///
/// Derived from the certificate's own id, so repeated describes of one certificate
/// report the same record while two certificates for the same domain differ.
pub fn validation_label(certificate_id: &str, domain_name: &str, purpose: &str) -> String {
    use sha2::Digest;
    let mut hasher = sha2::Sha256::new();
    hasher.update(certificate_id.as_bytes());
    hasher.update([0u8]);
    hasher.update(domain_name.as_bytes());
    hasher.update([0u8]);
    hasher.update(purpose.as_bytes());
    let digest = hasher.finalize();
    hex::encode(&digest[..16])
}

/// A domain name with a leading wildcard label removed: ACM validates
/// `*.example.com` by proving control of `example.com`.
pub fn validation_target(domain_name: &str) -> &str {
    domain_name.strip_prefix("*.").unwrap_or(domain_name)
}

/// The mailbox local-parts ACM sends a domain-validation email to: "ACM sends email
/// to the the following five addresses: admin@<suffix> administrator@<suffix>
/// hostmaster@<suffix> postmaster@<suffix> webmaster@<suffix>"
/// (`ResendValidationEmailRequest$ValidationDomain` doc).
///
/// A certificate's `ValidationEmails` lists the addresses ACM HAS written to, so these
/// are rendered onto a record when mail goes out — the initial dispatch the clock owes
/// a request ([`Certificate::dispatch_validation_email`]) or a
/// `ResendValidationEmail` — never by the describe that reports it.
pub const VALIDATION_EMAIL_LOCAL_PARTS: &[&str] = &[
    "admin",
    "administrator",
    "hostmaster",
    "postmaster",
    "webmaster",
];

/// The mailboxes ACM sends a domain's validation email to, at `mail_suffix`.
///
/// "ACM sends email to the the following five addresses:
/// admin@<suffix> administrator@<suffix> hostmaster@<suffix> postmaster@<suffix>
/// webmaster@<suffix>" (`ResendValidationEmailRequest$ValidationDomain` doc), i.e.
/// [`VALIDATION_EMAIL_LOCAL_PARTS`] at the validation domain. Rendered here so the
/// address set a `RequestCertificate` records and the one a `ResendValidationEmail`
/// records cannot drift apart.
pub fn validation_mailboxes(mail_suffix: &str) -> Vec<String> {
    VALIDATION_EMAIL_LOCAL_PARTS
        .iter()
        .map(|local_part| format!("{local_part}@{mail_suffix}"))
        .collect()
}

/// Whether two domain names name the same domain. DNS labels are case-insensitive,
/// so `Example.COM` and `example.com` are one name.
pub fn same_domain(left: &str, right: &str) -> bool {
    left.eq_ignore_ascii_case(right)
}

/// Whether `validation_domain` may validate `domain_name`: the superdomain rule ACM
/// states for every `ValidationDomain` member it models
/// (`DomainValidationOption$ValidationDomain`: "This must be the same as the
/// `DomainName` value or a superdomain of the `DomainName` value";
/// `ResendValidationEmailRequest$ValidationDomain` says the same of `Domain`).
///
/// A superdomain is a DOT-BOUNDARY suffix: `example.com` validates
/// `testing.example.com`, while `ample.com` and `other.example.com` do not. The
/// comparison is case-insensitive (see [`same_domain`]), and a wildcard name is
/// validated through the domain it covers, which is the name ACM proves control of
/// ([`validation_target`]).
pub fn is_validation_domain_for(validation_domain: &str, domain_name: &str) -> bool {
    if same_domain(validation_domain, domain_name) {
        return true;
    }
    let candidate = validation_domain.to_ascii_lowercase();
    let target = validation_target(domain_name).to_ascii_lowercase();
    candidate == target
        || target
            .strip_suffix(&candidate)
            .is_some_and(|higher_labels| higher_labels.ends_with('.'))
}

/// The validation ACM is waiting on for one domain name of one certificate.
///
/// DNS validation publishes a CNAME, email validation names the mailboxes ACM wrote
/// to, and HTTP validation names the URL the CA fetches; `validation_method` decides
/// which. Every value is derived from the certificate's own id, so the same
/// `(certificate, domain, method)` always yields the same record — a later
/// `DescribeCertificate` reports exactly what was stored, and a certificate migrated
/// back to a method it already used reports the record it had.
///
/// Shared, because the method a certificate validates its domains by is set when the
/// certificate is requested and can be changed afterwards
/// (`CertificateOptions$ValidationMethod`), so both paths must mint the same record.
pub fn domain_validation_record(
    certificate_id: &str,
    domain_name: &str,
    validation_method: &str,
    validation_domain: Option<&str>,
) -> DomainValidationRecord {
    let target = validation_target(domain_name);
    let mut record = DomainValidationRecord {
        domain_name: domain_name.to_string(),
        validation_status: DOMAIN_STATUS_PENDING_VALIDATION.to_string(),
        validation_method: validation_method.to_string(),
        // The domain whose control is being proven, whatever the method proves it
        // with: the one the request named for this name, else the name itself. It is
        // also the SUFFIX of the mailboxes ACM writes to, and a mail suffix carries no
        // wildcard label — a certificate that validates `*.example.com` through itself
        // is mailed at `example.com`, the domain ACM proves control of.
        validation_domain: Some(
            validation_domain
                .map(validation_target)
                .unwrap_or(target)
                .to_string(),
        ),
        requested_validation_domain: validation_domain.map(str::to_string),
        // Minting a validation sends no mail: the mailbox list names the messages ACM
        // HAS sent, which is what [`Certificate::dispatch_validation_email`] and
        // `ResendValidationEmail` record.
        validation_emails: Vec::new(),
        resource_record: None,
        http_redirect: None,
    };
    match validation_method {
        "DNS" => {
            let name = validation_label(certificate_id, domain_name, "dns-name");
            record.resource_record = Some(ResourceRecord {
                name: format!("_{name}.{target}."),
                r#type: RECORD_TYPE_CNAME.to_string(),
                value: acm_validations_cname_target(certificate_id, domain_name),
            });
        }
        "HTTP" => {
            let token = validation_label(certificate_id, domain_name, "http-token");
            let path = HTTP_VALIDATION_PATH;
            record.http_redirect = Some(HttpRedirect {
                redirect_from: Some(format!("http://{target}{path}{token}.txt")),
                redirect_to: Some(format!(
                    "http://{token}.{ACM_VALIDATIONS_ZONE}{path}{token}.txt"
                )),
            });
        }
        // EMAIL, and the default when no method was named: the validation domain ACM
        // mails is already on the record, and the mailboxes it wrote to are recorded
        // by the call that writes them.
        _ => {}
    }
    record
}

/// The DNS record the caller publishes to prove ownership of `domain_name` for one
/// ACME domain validation: "the DNS resource record to create for domain validation"
/// (`DnsPrevalidationDetails$ResourceRecord` doc).
///
/// SERVER-MANAGED, like the CNAME a certificate's DNS validation publishes
/// ([`domain_validation_record`]) and in the same form: a `_<label>` prefixed name
/// under the domain being validated, pointing at the challenge target
/// [`acm_validations_cname_target`] mints. Every label is derived from the validation's
/// OWN id through [`validation_label`], so repeated reads of one validation report the
/// same record, two validations of the same domain differ, and no part of the record
/// comes from anything the caller sent.
pub fn acme_prevalidation_resource_record(
    validation_id: &str,
    domain_name: &str,
) -> ResourceRecord {
    let target = validation_target(domain_name);
    let name = validation_label(validation_id, domain_name, "dns-name");
    ResourceRecord {
        name: format!("_{name}.{target}."),
        r#type: RECORD_TYPE_CNAME.to_string(),
        value: acm_validations_cname_target(validation_id, domain_name),
    }
}

/// The number of label characters a validation's own subdomain under
/// [`ACM_VALIDATIONS_ZONE`] carries, as the recorded targets show that label: a short
/// run of lower-case alphanumerics, far shorter than the 32-character challenge token
/// beside it.
const VALIDATION_ZONE_LABEL_LENGTH: usize = 10;

/// The CNAME target a DNS validation's challenge record points at:
/// `_<token>.<per-validation label>.acm-validations.<zone>.`
///
/// Three parts, and every one of them is required: the challenge TOKEN, then the
/// validation's OWN label, then [`ACM_VALIDATIONS_ZONE`]. The middle label is what
/// makes the target a name under the validation's own subdomain of the ACM validation
/// zone rather than a name directly in it — the shape a client that publishes the
/// record and then checks the delegation structure reads it as.
///
/// Both labels are derived from the id of whatever the validation belongs to (a
/// certificate, an ACME domain validation) through [`validation_label`], so repeated
/// reads report the same target, two validations of the same domain differ, and no part
/// of it comes from anything the caller sent. Rendered here so the certificate path
/// ([`domain_validation_record`]) and the ACME prevalidation path
/// ([`acme_prevalidation_resource_record`]) publish one shape.
pub fn acm_validations_cname_target(owner_id: &str, domain_name: &str) -> String {
    let token = validation_label(owner_id, domain_name, "dns-value");
    let label: String = validation_label(owner_id, domain_name, "dns-zone-label")
        .chars()
        .take(VALIDATION_ZONE_LABEL_LENGTH)
        .collect();
    format!("_{token}.{label}.{ACM_VALIDATIONS_ZONE}.")
}

/// The tag-key prefix Amazon Web Services reserves for its own tags: neither half of
/// a pair a caller names may begin with it (`InvalidTagException`: "you cannot specify
/// a tag value that begins with aws:").
pub const RESERVED_TAG_PREFIX: &str = "aws:";

/// The number of tags one ACM resource may carry, from the `TagList` length bound the
/// model declares (`len 1..50`): it is both the most pairs one request may name and
/// the most a resource holds, since `ListTagsForResource` and
/// `ListTagsForCertificate` report the stored map through that same list.
pub const TAG_LIMIT: usize = 50;

/// The number of ACM certificates one account may hold in a Region, from the ACM
/// service quota "ACM certificates" (default 2,500 per Region). It bounds the store
/// against a caller that repeats a certificate-creating call, and past it ACM answers
/// the quota error those operations model (`LimitExceededException`: "An ACM quota has
/// been exceeded").
pub const CERTIFICATE_LIMIT: usize = 2_500;

/// The most certificate records \u2014 and the most idempotency entries \u2014 this emulator
/// stores, across every Region.
///
/// Not an AWS quota: [`CERTIFICATE_LIMIT`] is ACM's own per-Region one, and the Region
/// of a request is read off the caller's SigV4 credential scope ([`request_region`]),
/// so a per-Region bound is a bound the CALLER can partition by signing each request
/// for another Region string. This cap is keyed on nothing in the request, so it bounds
/// [`ComAmazonawsAcmCertificatemanagerState::certificates`] and
/// [`ComAmazonawsAcmCertificatemanagerState::idempotency`] \u2014 and with them the
/// per-request [`ComAmazonawsAcmCertificatemanagerState::materialize_due`] pass over
/// those records \u2014 however the request varies. `RequestCertificate` and
/// `ImportCertificate` check it before inserting and answer their modelled quota error
/// past it, exactly as [`ACME_ENDPOINT_STORE_LIMIT`] is checked for ACME endpoints.
pub const CERTIFICATE_STORE_LIMIT: usize = 5_000;

/// The request's tags as the key -> value map the certificate record stores, or the
/// message explaining why a tag was refused.
///
/// A tag key or value beginning with `aws:` is reserved, which is what
/// `InvalidTagException` names ("you cannot specify a tag value that begins with
/// aws:"). Later values win for a repeated key, so one key maps to one value.
pub fn tag_map(tags: &[Tag]) -> Result<BTreeMap<String, String>, String> {
    let mut mapped = BTreeMap::new();
    for tag in tags {
        check_tag_not_reserved(tag)?;
        mapped.insert(tag.key.clone(), tag.value.clone().unwrap_or_default());
    }
    Ok(mapped)
}

/// `Ok(())` for a tag any caller may name, or the `InvalidTagException` message for
/// one it may not.
///
/// The `aws:` prefix is reserved on both halves of the pair, which is what
/// `InvalidTagException` names ("you cannot specify a tag value that begins with
/// aws:"). Shared by every operation that takes tags from a request, so the tagging
/// and the untagging paths refuse the same pairs with the same sentence.
pub fn check_tag_not_reserved(tag: &Tag) -> Result<(), String> {
    check_tag_key_not_reserved(&tag.key)?;
    if let Some(value) = tag
        .value
        .as_deref()
        .filter(|value| value.starts_with(RESERVED_TAG_PREFIX))
    {
        return Err(reserved_tag_message(TAG_VALUE_HALF, value));
    }
    Ok(())
}

/// `Ok(())` for a tag key any caller may name, or the reserved-tag message for one it
/// may not.
///
/// The operation that removes tags BY KEY (`UntagResource`) carries no value to check,
/// so the key half of the rule is reachable on its own; it is the same rule and the
/// same sentence either way.
pub fn check_tag_key_not_reserved(key: &str) -> Result<(), String> {
    if key.starts_with(RESERVED_TAG_PREFIX) {
        return Err(reserved_tag_message(TAG_KEY_HALF, key));
    }
    Ok(())
}

/// How the reserved-tag refusal names the KEY half of a pair.
const TAG_KEY_HALF: &str = "Key";

/// How it names the VALUE half.
const TAG_VALUE_HALF: &str = "Value";

/// The message a refused reserved tag reports: which half of the pair carried the
/// reserved prefix, that half's text in double quotes, and why it may not be named.
///
/// A fixed template with the offending text interpolated — the pair is not named any
/// other way, so a client can read the rejected key or value straight back out of the
/// sentence.
fn reserved_tag_message(half: &str, offending: &str) -> String {
    format!(
        "Invalid Tag {half}: \"{offending}\". AWS internal tags cannot be changed with this API."
    )
}

/// The message a refusal of the per-resource tag quota reports: the quota, and nothing
/// of the request that reached it.
///
/// A constant sentence for the rule rather than a count computed from the request, so
/// every path that enforces [`TAG_LIMIT`] — whichever modelled error carries it — states
/// the same quota the same way.
pub fn tag_quota_message() -> String {
    format!("The resultant tag set must not have more than {TAG_LIMIT} user tags.")
}

/// Drop the requested tags from a stored tag map, as ACM's untagging operations
/// define it: a requested pair that carries no value removes its key whatever value
/// the key holds, and one that carries a value removes the key only when the stored
/// value is that value (`RemoveTagsFromCertificate` doc: "If you do not specify the
/// value portion of the tag when calling this function, the tag will be removed
/// regardless of value. If you specify a value, the tag is removed only if it is
/// associated with the specified value").
///
/// Entries are applied in request order and a key the map does not hold is simply
/// absent afterwards, so removing is idempotent.
pub fn remove_tags(stored: &mut BTreeMap<String, String>, requested: &[Tag]) {
    for tag in requested {
        let matches = match tag.value.as_deref() {
            None => stored.contains_key(&tag.key),
            Some(value) => stored
                .get(&tag.key)
                .is_some_and(|stored_value| stored_value == value),
        };
        if matches {
            stored.remove(&tag.key);
        }
    }
}

/// Drop the requested KEYS from a stored tag map, as ACM's key-addressed untagging
/// operation (`UntagResource`, whose `TagKeys` carries keys alone) defines it: a named
/// key is removed whatever value it holds, and a key the map does not hold leaves the
/// map as it was — so removing is idempotent.
pub fn remove_tag_keys(stored: &mut BTreeMap<String, String>, keys: &[String]) {
    for key in keys {
        stored.remove(key);
    }
}

/// The stored tags as the modelled `Tag` list, in key order.
pub fn tag_list(tags: &BTreeMap<String, String>) -> Vec<Tag> {
    tags.iter()
        .map(|(key, value)| Tag {
            key: key.clone(),
            value: Some(value.clone()),
        })
        .collect()
}

/// `CertificateExport` for a certificate: `ENABLED` only when its options opted in.
pub fn export_option(cert: &Certificate) -> String {
    cert.options
        .as_ref()
        .and_then(|options| options.export.clone())
        .unwrap_or_else(|| EXPORT_DISABLED.to_string())
}

/// A certificate's `CertificateOptions` as a response reports them: every preference
/// the certificate carries, defaulted where the request named none, so a client that
/// set one can read it back and one that set nothing sees what ACM applied.
///
/// `ValidationMethod` is reported from the certificate's own options: it is the
/// preference a request or an `UpdateCertificateOptions` named there, and a
/// certificate whose options never named one reports none — the method each name is
/// being validated by is reported per domain, in `DomainValidation$ValidationMethod`.
pub fn certificate_options(cert: &Certificate) -> CertificateOptions {
    CertificateOptions {
        certificate_transparency_logging_preference: Some(
            cert.options
                .as_ref()
                .and_then(|options| options.certificate_transparency_logging_preference.clone())
                .unwrap_or_else(|| TRANSPARENCY_LOGGING_ENABLED.to_string()),
        ),
        export: Some(export_option(cert)),
        validation_method: cert
            .options
            .as_ref()
            .and_then(|options| options.validation_method.clone()),
    }
}

/// `CertificateDetail$Subject`: the subject distinguished name of a certificate whose
/// common name is `common_name`, in the `CN=<name>` form an X.509 subject is written
/// in.
pub fn subject_distinguished_name(common_name: &str) -> String {
    format!("CN={common_name}")
}

/// `CertificateDetail$Issuer`: the name of the CA that signs the certificate.
///
/// A public certificate ACM issues is signed by Amazon's own CA ([`ISSUER_AMAZON`]).
/// One ACM issues from a private CA is signed by that CA, whose subject name lives in
/// Private CA's own store and not in this service's, so no issuer is reported for it.
pub fn issuer_name(certificate_authority_arn: Option<&str>) -> Option<String> {
    certificate_authority_arn
        .is_none()
        .then(|| ISSUER_AMAZON.to_string())
}

/// `CertificateDetail$SignatureAlgorithm`: "the algorithm that was used to sign the
/// certificate", as the `<hash>WITH<key family>` spelling AWS reports it in, or `None`
/// for a key algorithm of no family this service knows how to sign with — an absent
/// member rather than a wrong one.
///
/// ACM signs with SHA-256, so the stored `KeyAlgorithm`'s family picks the rest:
/// `SHA256WITHRSA` for an RSA key pair and `SHA256WITHECDSA` for an elliptic-curve
/// one. Both spellings are the sibling Private CA model's `SigningAlgorithm` values
/// (`smithy_query(query="shape", shape="com.amazonaws.acmpca#SigningAlgorithm")`).
///
/// The family is the part of the `KeyAlgorithm` BEFORE its separator (`EC` of
/// `EC_prime256v1`), matched in full, so neither a name that merely begins with those
/// letters nor a family the model adds later is reported as one of these two.
pub fn signature_algorithm(key_algorithm: &str) -> Option<String> {
    // The family is the part before the first separator (`EC` of `EC_prime256v1`),
    // and a name with no separator is its own family.
    let family = match key_algorithm
        .split_once('_')
        .map(|(family, _)| family)
        .unwrap_or(key_algorithm)
    {
        "RSA" => "RSA",
        "EC" => "ECDSA",
        _ => return None,
    };
    Some(format!("SHA256WITH{family}"))
}

/// `KeyAlgorithm` as every projection that reports a certificate's algorithm states
/// it: the X.509 algorithm descriptor, whose family and parameter are joined by a
/// HYPHEN (`RSA-2048`, `EC-prime256v1`), rather than the model enum's underscored name
/// (`RSA_2048`) that a request and the `Includes.keyTypes` filter are written in.
///
/// `SearchCertificates` is the one reader that states the underscored name instead, and
/// deliberately: every member of its result is the value the filter on that member
/// selects on (see [`certificate_search_result`]), and no filter is written in the
/// descriptor spelling.
pub fn reported_key_algorithm(key_algorithm: &str) -> String {
    key_algorithm.replacen('_', "-", 1)
}

/// The account's expiry-events configuration as the modelled
/// `ExpiryEventsConfiguration`: what `PutAccountConfiguration` stored, or the
/// documented default for an account that has written none.
pub fn expiry_events_configuration(
    stored: Option<&AccountConfiguration>,
) -> ExpiryEventsConfiguration {
    ExpiryEventsConfiguration {
        days_before_expiry: match stored {
            // A write that named an `ExpiryEvents` with no `DaysBeforeExpiry` left
            // the option unset, so the account is back on the default.
            Some(config) => config
                .days_before_expiry
                .or(Some(DEFAULT_DAYS_BEFORE_EXPIRY)),
            None => Some(DEFAULT_DAYS_BEFORE_EXPIRY),
        },
    }
}

/// One stored validation as the modelled `DomainValidation`.
///
/// `ValidationEmails` is "a list of email addresses that ACM used to send domain
/// validation emails" (`DomainValidation$ValidationEmails` doc), so it belongs to a
/// domain validated BY EMAIL: it is reported for such a domain — as the empty list
/// once no mailbox was used, which is a validation that never ran — and is absent for
/// a domain validated by DNS or HTTP, where ACM sends no mail at all.
fn domain_validation(record: &DomainValidationRecord) -> DomainValidation {
    DomainValidation {
        domain_name: record.domain_name.clone(),
        validation_emails: (!record.validation_emails.is_empty()
            || record.validation_method == VALIDATION_METHOD_EMAIL)
            .then(|| record.validation_emails.clone()),
        validation_domain: record.validation_domain.clone(),
        validation_status: Some(record.validation_status.clone()),
        resource_record: record.resource_record.clone(),
        http_redirect: record.http_redirect.clone(),
        validation_method: Some(record.validation_method.clone()),
    }
}

/// A stored certificate as `DescribeCertificate`'s `CertificateDetail`.
///
/// Every member the certificate's own state decides is reported: the X.509 identity
/// ACM fixed when it accepted the request (`Subject`, `Issuer`,
/// `SignatureAlgorithm`), the key-usage extension lists (empty, as lists, while the
/// certificate is still pending) and the full `Options`. Members that come into
/// existence only with the certificate itself (`Serial`, `IssuedAt`, `NotBefore` /
/// `NotAfter`) stay absent until the record carries them.
pub fn certificate_detail(cert: &Certificate, in_use_by: Vec<String>) -> CertificateDetail {
    CertificateDetail {
        certificate_arn: Some(cert.arn.clone()),
        domain_name: Some(cert.domain_name.clone()),
        subject_alternative_names: Some(cert.subject_alternative_names.clone()),
        managed_by: cert.managed_by.clone(),
        domain_validation_options: (!cert.domain_validations.is_empty())
            .then(|| cert.domain_validations.iter().map(domain_validation).collect()),
        // "The time at which the certificate was requested"
        // (`CertificateDetail$CreatedAt` doc): an imported certificate was never
        // requested, so it reports the instant it was imported instead.
        created_at: (!cert.is_imported()).then(|| to_date_time(cert.created_at)),
        issued_at: cert.issued_at.map(to_date_time),
        imported_at: cert.imported_at.map(to_date_time),
        // The serial the signing authority assigned, which only a record holding a
        // signed certificate carries.
        serial: cert.serial.clone(),
        status: Some(cert.status.clone()),
        not_before: cert.not_before.map(to_date_time),
        not_after: cert.not_after.map(to_date_time),
        key_algorithm: Some(reported_key_algorithm(&cert.key_algorithm)),
        subject: cert.subject.clone(),
        issuer: cert.issuer.clone(),
        signature_algorithm: cert.signature_algorithm.clone(),
        // "A list of ARNs for the Amazon Web Services resources that are using the
        // certificate" (`CertificateDetail$InUseBy` doc): those resources live in the
        // stores of the services that own them, so the list is the shared association
        // directory's answer for this certificate
        // ([`certificate_association_directory`]) and never a fact this service holds a
        // second copy of.
        in_use_by: Some(in_use_by),
        // The X.509 key-usage extensions the certificate carries. A pending
        // certificate has none yet, which is the EMPTY list, not a missing member.
        key_usages: Some(
            cert.key_usages
                .iter()
                .map(|name| KeyUsage {
                    name: Some(name.clone()),
                })
                .collect(),
        ),
        // Each purpose the extension asserts, as the {name, identifier} pair ACM
        // publishes it as: "each object specifies a purpose for which the certificate
        // public key can be used and consists of a name and an object identifier (OID)"
        // (`CertificateDetail$ExtendedKeyUsages` doc).
        extended_key_usages: Some(
            cert.extended_key_usages
                .iter()
                .map(|usage| ExtendedKeyUsage {
                    name: Some(usage.name.clone()),
                    oid: Some(usage.oid.clone()),
                })
                .collect(),
        ),
        r#type: Some(cert.cert_type.clone()),
        certificate_authority_arn: cert.certificate_authority_arn.clone(),
        renewal_summary: renewal_summary(cert),
        renewal_eligibility: Some(cert.renewal_eligibility().to_string()),
        // "This value exists only when the certificate status is FAILED"
        // (`CertificateDetail$FailureReason` doc), which is what the record stores.
        failure_reason: cert.failure_reason.clone(),
        // Both "exist only when the certificate status is REVOKED"
        // (`CertificateDetail$RevokedAt` / `$RevocationReason` docs), and the record
        // only ever carries them together with that status.
        revoked_at: cert.revoked_at.map(to_date_time),
        revocation_reason: cert.revocation_reason.clone(),
        options: Some(certificate_options(cert)),
        certificate_key_pair_origin: Some(cert.key_pair_origin.clone()),
        ..CertificateDetail::default()
    }
}

/// A stored certificate as `ListCertificates`' `CertificateSummary`.
///
/// `SubjectAlternativeNameSummaries` carries at most [`SUMMARY_SAN_LIMIT`] names and
/// `HasAdditionalSubjectAlternativeNames` says whether the certificate has more, as
/// that member's doc defines.
pub fn certificate_summary(cert: &Certificate, in_use: bool) -> CertificateSummary {
    let total_sans = cert.subject_alternative_names.len();
    let shown = total_sans.min(SUMMARY_SAN_LIMIT);
    CertificateSummary {
        certificate_arn: Some(cert.arn.clone()),
        domain_name: Some(cert.domain_name.clone()),
        subject_alternative_name_summaries: Some(
            cert.subject_alternative_names[..shown].to_vec(),
        ),
        has_additional_subject_alternative_names: Some(total_sans > shown),
        status: Some(cert.status.clone()),
        r#type: Some(cert.cert_type.clone()),
        // The summary states the algorithm exactly as the certificate's DETAIL does
        // ([`reported_key_algorithm`]): both report the same member of the same
        // certificate, so one certificate cannot carry two key algorithms depending on
        // which reader is asked. The enum's underscored spelling is what the
        // `Includes.keyTypes` filter is written in, not what a report states.
        key_algorithm: Some(reported_key_algorithm(&cert.key_algorithm)),
        // The X.509 key-usage extension names of an issued certificate; absent while
        // the record carries none.
        key_usages: (!cert.key_usages.is_empty()).then(|| cert.key_usages.clone()),
        extended_key_usages: (!cert.extended_key_usages.is_empty())
            .then(|| extended_key_usage_names(&cert.extended_key_usages)),
        export_option: Some(export_option(cert)),
        // Whether any resource reported by `CertificateDetail$InUseBy` references this
        // certificate, which the shared association directory is what knows.
        in_use: Some(in_use),
        exported: Some(cert.exported),
        renewal_eligibility: Some(cert.renewal_eligibility().to_string()),
        not_before: cert.not_before.map(to_date_time),
        not_after: cert.not_after.map(to_date_time),
        // As on `CertificateDetail`: `CreatedAt` is when the certificate was
        // REQUESTED, and an imported one carries its import instant instead.
        created_at: (!cert.is_imported()).then(|| to_date_time(cert.created_at)),
        issued_at: cert.issued_at.map(to_date_time),
        imported_at: cert.imported_at.map(to_date_time),
        // Present only for a revoked certificate, as on `CertificateDetail`.
        revoked_at: cert.revoked_at.map(to_date_time),
        managed_by: cert.managed_by.clone(),
        certificate_key_pair_origin: Some(cert.key_pair_origin.clone()),
    }
}

/// The `ExtendedKeyUsageName` of each stored key purpose, for the two projections that
/// report the names alone (`CertificateSummary$ExtendedKeyUsages`,
/// `X509Attributes$ExtendedKeyUsages`, both lists of `ExtendedKeyUsageName`).
fn extended_key_usage_names(usages: &[ExtendedKeyUsageRecord]) -> Vec<String> {
    usages.iter().map(|usage| usage.name.clone()).collect()
}

/// The managed renewal a certificate carries as the modelled `RenewalSummary`, or
/// `None` for one no renewal was ever started for — the member "exists only when the
/// certificate type is AMAZON_ISSUED" (`CertificateDetail$RenewalSummary` doc) and a
/// renewal is what brings it into existence.
///
/// `DomainValidationOptions` is the validation of each name "as it pertains to ACM's
/// managed renewal" (`RenewalSummary$DomainValidationOptions` doc), which is the
/// certificate's own validation records: a renewal re-proves the names the certificate
/// covers, and those are the records that hold their evidence.
pub fn renewal_summary(cert: &Certificate) -> Option<RenewalSummary> {
    cert.renewal.as_ref().map(|renewal| RenewalSummary {
        renewal_status: renewal.status.clone(),
        domain_validation_options: cert.domain_validations.iter().map(domain_validation).collect(),
        // "The reason that a renewal request was unsuccessful"
        // (`RenewalSummary$RenewalStatusReason` doc): no renewal this service starts
        // fails, so no record carries one.
        renewal_status_reason: None,
        updated_at: to_date_time(renewal.updated_at),
    })
}

/// A stored ACME endpoint as `DescribeAcmeEndpoint`'s `AcmeEndpoint`.
///
/// `FailureReason` exists only for an endpoint that failed
/// (`AcmeEndpoint$FailureReason` doc), which is what the record stores, and
/// `CertificateTags` is reported only when the endpoint carries tags to apply to the
/// certificates it issues — a create that named none leaves the member absent rather
/// than reporting an empty list (`TagList` has a minimum length of 1, so an empty list
/// is not a value the member can take).
///
/// `EndpointUrl` is the address the ACME surface of this endpoint is published at for the
/// CALLER being answered ([`acme_published_endpoint_url`]): an ACME client is configured
/// with that exact URL and dials it, so it names the endpoint this caller reached rather
/// than an address only a client that resolves AWS's own hostnames could reach.
pub fn acme_endpoint_detail(
    endpoint: &AcmeEndpointRecord,
    local_endpoint: Option<&str>,
) -> AcmeEndpoint {
    AcmeEndpoint {
        acme_endpoint_arn: Some(endpoint.arn.clone()),
        endpoint_url: Some(acme_published_endpoint_url(endpoint, local_endpoint)),
        status: Some(endpoint.status.clone()),
        failure_reason: endpoint.failure_reason.clone(),
        authorization_behavior: Some(endpoint.authorization_behavior.clone()),
        contact: Some(endpoint.contact.clone()),
        certificate_authority: Some(endpoint.certificate_authority.clone()),
        certificate_tags: (!endpoint.certificate_tags.is_empty())
            .then(|| tag_list(&endpoint.certificate_tags)),
        created_at: Some(to_date_time(endpoint.created_at)),
        updated_at: Some(to_date_time(endpoint.updated_at)),
    }
}

/// The URL this endpoint's ACME directory is published at for a caller that reached
/// `local_endpoint`: the stored published URL ([`AcmeEndpointRecord::endpoint_url`]) for
/// a caller that resolves AWS's own hostnames, and the same resource of the same endpoint
/// on the endpoint THIS caller reached for one that reached this emulator directly
/// ([`acme_endpoint_url_at`]).
///
/// Both are derived from the endpoint's own ARN — the Region and the id it carries — so
/// the two addresses name one endpoint and neither carries anything the caller sent.
pub fn acme_published_endpoint_url(
    endpoint: &AcmeEndpointRecord,
    local_endpoint: Option<&str>,
) -> String {
    let (Some(local), Some(region), Some(endpoint_id)) = (
        local_endpoint,
        acm_arn_region(&endpoint.arn),
        acme_endpoint_id_from_arn(&endpoint.arn),
    ) else {
        return endpoint.endpoint_url.clone();
    };
    acme_endpoint_url_at(Some(local), region, endpoint_id)
}

/// A stored ACME endpoint as `ListAcmeEndpoints`' `AcmeEndpointSummary`.
///
/// `AcmeEndpointSummary` declares exactly the members `AcmeEndpoint` does, so one
/// endpoint is reported identically whether it is read one at a time or listed: the
/// summary is projected from the same record, member for member, through
/// [`acme_endpoint_detail`] — including the two members that are present only when the
/// record carries them (`FailureReason`, `CertificateTags`) and the `EndpointUrl` minted
/// at the address the caller reached.
pub fn acme_endpoint_summary(
    endpoint: &AcmeEndpointRecord,
    local_endpoint: Option<&str>,
) -> AcmeEndpointSummary {
    let detail = acme_endpoint_detail(endpoint, local_endpoint);
    AcmeEndpointSummary {
        acme_endpoint_arn: detail.acme_endpoint_arn,
        endpoint_url: detail.endpoint_url,
        status: detail.status,
        failure_reason: detail.failure_reason,
        authorization_behavior: detail.authorization_behavior,
        contact: detail.contact,
        certificate_authority: detail.certificate_authority,
        certificate_tags: detail.certificate_tags,
        created_at: detail.created_at,
        updated_at: detail.updated_at,
    }
}

/// A stored ACME domain validation as `DescribeAcmeDomainValidation`'s
/// `AcmeDomainValidation`.
///
/// Every member is read off the record: `PrevalidationDetails` reports the scope and
/// hosted zone the create configured BESIDE the resource record the service minted
/// ([`acme_prevalidation_resource_record`]), which is the one member of the three the
/// caller cannot have sent. `FailureDetails` exists only for a validation that failed
/// ("details about the failure, if the validation failed",
/// `AcmeDomainValidation$FailureDetails` doc), which is what the record stores.
pub fn acme_domain_validation_detail(
    validation: &AcmeDomainValidationRecord,
) -> AcmeDomainValidation {
    AcmeDomainValidation {
        acme_domain_validation_arn: Some(validation.arn.clone()),
        acme_endpoint_arn: Some(validation.acme_endpoint_arn.clone()),
        domain_name: Some(validation.domain_name.clone()),
        prevalidation_type: Some(validation.prevalidation_type.clone()),
        prevalidation_details: Some(PrevalidationDetails::DnsPrevalidation(
            DnsPrevalidationDetails {
                domain_scope: validation.domain_scope.clone(),
                hosted_zone_id: validation.hosted_zone_id.clone(),
                resource_record: validation.resource_record.clone(),
            },
        )),
        status: Some(validation.status.clone()),
        failure_details: validation.failure_details.clone(),
        created_at: Some(to_date_time(validation.created_at)),
        updated_at: Some(to_date_time(validation.updated_at)),
    }
}

/// A stored ACME domain validation as `ListAcmeDomainValidations`'
/// `AcmeDomainValidationSummary`.
///
/// `AcmeDomainValidationSummary` declares exactly the members `AcmeDomainValidation`
/// does, so one validation is reported identically whether it is read one at a time or
/// listed: the summary is projected from the same record, member for member, through
/// [`acme_domain_validation_detail`] — including `FailureDetails`, which is present only
/// for a validation that failed.
pub fn acme_domain_validation_summary(
    validation: &AcmeDomainValidationRecord,
) -> AcmeDomainValidationSummary {
    let detail = acme_domain_validation_detail(validation);
    AcmeDomainValidationSummary {
        acme_domain_validation_arn: detail.acme_domain_validation_arn,
        acme_endpoint_arn: detail.acme_endpoint_arn,
        domain_name: detail.domain_name,
        prevalidation_type: detail.prevalidation_type,
        prevalidation_details: detail.prevalidation_details,
        status: detail.status,
        failure_details: detail.failure_details,
        created_at: detail.created_at,
        updated_at: detail.updated_at,
    }
}

/// A stored ACME external account binding as
/// `DescribeAcmeExternalAccountBinding`'s `AcmeExternalAccountBinding`.
///
/// Every member is read off the record. The three instant members exist only for a
/// binding that has reached them: `ExpiresAt` for one the create gave an `Expiration`,
/// `RevokedAt` only "the time at which the external account binding was revoked"
/// (`AcmeExternalAccountBinding$RevokedAt` doc) and `LastUsedAt` only once an ACME client
/// has registered with the binding's credentials ("the time at which the external
/// account binding was last used", `$LastUsedAt` doc). The credentials themselves are
/// NOT members of this shape: `GetAcmeExternalAccountBindingCredentials` is the operation
/// that reports them.
pub fn acme_external_account_binding_detail(
    binding: &AcmeExternalAccountBindingRecord,
) -> AcmeExternalAccountBinding {
    AcmeExternalAccountBinding {
        acme_external_account_binding_arn: Some(binding.arn.clone()),
        acme_endpoint_arn: Some(binding.acme_endpoint_arn.clone()),
        role_arn: Some(binding.role_arn.clone()),
        expires_at: binding.expires_at.map(to_date_time),
        revoked_at: binding.revoked_at.map(to_date_time),
        last_used_at: binding.last_used_at.map(to_date_time),
        created_at: Some(to_date_time(binding.created_at)),
        updated_at: Some(to_date_time(binding.updated_at)),
    }
}

/// A stored ACME external account binding as `ListAcmeExternalAccountBindings`'
/// `AcmeExternalAccountBindingSummary`.
///
/// `AcmeExternalAccountBindingSummary` declares exactly the members
/// `AcmeExternalAccountBinding` does, so one binding is reported identically whether it
/// is read one at a time or listed: the summary is projected from the same record, member
/// for member, through [`acme_external_account_binding_detail`] — including the three
/// instant members that exist only for a binding that has reached them. The credentials
/// are no members of this shape either, so listing reports no secret.
pub fn acme_external_account_binding_summary(
    binding: &AcmeExternalAccountBindingRecord,
) -> AcmeExternalAccountBindingSummary {
    let detail = acme_external_account_binding_detail(binding);
    AcmeExternalAccountBindingSummary {
        acme_external_account_binding_arn: detail.acme_external_account_binding_arn,
        acme_endpoint_arn: detail.acme_endpoint_arn,
        role_arn: detail.role_arn,
        expires_at: detail.expires_at,
        revoked_at: detail.revoked_at,
        last_used_at: detail.last_used_at,
        created_at: detail.created_at,
        updated_at: detail.updated_at,
    }
}

/// A stored ACME account as `ListAcmeAccounts`' `AcmeAccountSummary`.
///
/// Every member is read off the record: the summary and `DescribeAcmeAccount`'s
/// `AcmeAccount` carry the same members, so both project this one record and cannot
/// disagree. `AcmeExternalAccountBindingArn` is present only for an account that
/// registered against a binding, and `Contacts` only for one that registered contact
/// information — "the contact information for the ACME account"
/// (`AcmeAccountSummary$Contacts` doc), which an endpoint whose `Contact` is
/// `NOT_REQUIRED` need not have.
pub fn acme_account_summary(account: &AcmeAccountRecord) -> AcmeAccountSummary {
    AcmeAccountSummary {
        account_url: Some(account.account_url.clone()),
        public_key_thumbprint: Some(account.public_key_thumbprint.clone()),
        status: Some(account.status.clone()),
        created_at: Some(to_date_time(account.created_at)),
        acme_external_account_binding_arn: account.acme_external_account_binding_arn.clone(),
        contacts: (!account.contacts.is_empty()).then(|| account.contacts.clone()),
    }
}

/// A stored ACME account as `DescribeAcmeAccount`'s `AcmeAccount`.
///
/// `AcmeAccount` declares exactly the members `AcmeAccountSummary` does, so one account is
/// reported identically whether it is described one at a time or listed: the detail is
/// projected from the same record, member for member, through [`acme_account_summary`].
pub fn acme_account_detail(account: &AcmeAccountRecord) -> AcmeAccount {
    let summary = acme_account_summary(account);
    AcmeAccount {
        account_url: summary.account_url,
        public_key_thumbprint: summary.public_key_thumbprint,
        status: summary.status,
        created_at: summary.created_at,
        acme_external_account_binding_arn: summary.acme_external_account_binding_arn,
        contacts: summary.contacts,
    }
}

/// The `ResourceNotFoundException` message for an ACME account URL no stored record
/// answers to on the endpoint the request named.
///
/// Worded like every other lookup miss in this service
/// ([`acme_endpoint_not_found_message`]): one complete sentence naming what the request
/// asked for, terminated by a period.
pub fn acme_account_not_found_message(account_url: &str) -> String {
    format!("Could not find ACME account {account_url}.")
}

/// A stored certificate as `SearchCertificates`' `CertificateSearchResult`: the same
/// record `certificate_detail` and `certificate_summary` project, split into the
/// X.509 attributes and the ACM metadata that operation's filters select on.
///
/// Every member here is the value the FILTER on that member compares, so a
/// certificate a filter kept reports the value that kept it — a client that
/// re-reads the attribute it filtered on sees what it asked for, and this
/// projection and the filter cannot drift into two vocabularies for one attribute.
///
/// Members that belong to something this service never does stay absent, and are the
/// members the search filters correspondingly keep nothing on:
///
/// * `X509Attributes.Issuer` is the issuing CA's distinguished NAME, which no stored
///   record holds — the detail's own `Issuer` member is a plain CA name, which
///   [`issuer_name`] does report. `SerialNumber` is the serial the signing authority
///   assigned, reported for a record that carries one (an imported certificate's own
///   X.509 serial) and absent for one this emulator never signed.
/// * `AcmCertificateMetadata.RenewalStatus` belongs to a renewal ACM has started,
///   which `RenewCertificate` is what starts, and the ACME members to a certificate
///   obtained through an ACME endpoint, which no stored record carries. `ImportedAt` is
///   reported for an imported certificate,
///   `RevokedAt` for a revoked one, and each is absent exactly when the record carries
///   nothing for it, which "exists only when the certificate status is REVOKED"
///   (`CertificateDetail$RevokedAt` doc) and "only when the certificate type is
///   IMPORTED" (`$ImportedAt` doc) demand.
pub fn certificate_search_result(cert: &Certificate, in_use: bool) -> CertificateSearchResult {
    CertificateSearchResult {
        certificate_arn: Some(cert.arn.clone()),
        x509_attributes: Some(X509Attributes {
            // The subject common name of an ACM certificate is the domain name the
            // certificate was requested for.
            subject: Some(DistinguishedName {
                common_name: Some(cert.domain_name.clone()),
                ..DistinguishedName::default()
            }),
            // The name set the certificate covers, each as the DNS name it is.
            subject_alternative_names: Some(
                cert.subject_alternative_names
                    .iter()
                    .map(|name| GeneralName::DnsName(name.clone()))
                    .collect(),
            ),
            // A search result reports each attribute as the value the FILTER on that
            // attribute selects on, so a certificate a filter kept reports the value
            // that kept it: `X509AttributeFilter.KeyAlgorithm` compares the stored
            // `KeyAlgorithm` in the model enum's spelling (`EC_prime256v1`), which is
            // therefore the spelling reported here. The certificate's own REPORTS of
            // the algorithm — its detail and its listing summary — state the X.509
            // descriptor spelling instead (see [`reported_key_algorithm`]), which no
            // filter is written in.
            key_algorithm: Some(cert.key_algorithm.clone()),
            // The X.509 key-usage extensions of an issued certificate; absent while
            // the record carries none.
            key_usages: (!cert.key_usages.is_empty()).then(|| cert.key_usages.clone()),
            extended_key_usages: (!cert.extended_key_usages.is_empty())
                .then(|| extended_key_usage_names(&cert.extended_key_usages)),
            not_before: cert.not_before.map(to_date_time),
            not_after: cert.not_after.map(to_date_time),
            serial_number: cert.serial.clone(),
            ..X509Attributes::default()
        }),
        certificate_metadata: Some(CertificateMetadata::AcmCertificateMetadata(
            AcmCertificateMetadata {
                created_at: (!cert.is_imported()).then(|| to_date_time(cert.created_at)),
                exported: Some(cert.exported),
                in_use: Some(in_use),
                issued_at: cert.issued_at.map(to_date_time),
                imported_at: cert.imported_at.map(to_date_time),
                // The revocation instant, which exists only beside the `REVOKED`
                // status the same record carries.
                revoked_at: cert.revoked_at.map(to_date_time),
                renewal_eligibility: Some(cert.renewal_eligibility().to_string()),
                // The status of the managed renewal this certificate carries, for one a
                // renewal was started for; absent for every other record, which is what
                // the matching filter then keeps nothing on.
                renewal_status: cert.renewal.as_ref().map(|renewal| renewal.status.clone()),
                status: Some(cert.status.clone()),
                r#type: Some(cert.cert_type.clone()),
                export_option: Some(export_option(cert)),
                managed_by: cert.managed_by.clone(),
                validation_method: cert.validation_method.clone(),
                certificate_key_pair_origin: Some(cert.key_pair_origin.clone()),
                ..AcmCertificateMetadata::default()
            },
        )),
    }
}
