// SPDX-License-Identifier: CC0-1.0

//! The account's registered CA CERTIFICATES: the record one holds, and what the account's
//! own state refuses a registration for.
//!
//! A CA certificate is a certificate the account REGISTERS rather than one this service
//! issues: `RegisterCACertificate` is documented "Registers a CA certificate with Amazon
//! Web Services IoT Core", so the certificate material is the caller's and everything
//! this service reports about it is READ OUT of that material — the subject it certifies
//! and the validity period it carries — rather than chosen here. That is why the
//! registration parses the certificate it is handed with `x509-cert` and refuses one it
//! cannot read with the modelled `CertificateValidationException` ("The certificate is
//! invalid"): a record whose `validity` or subject was invented would describe a
//! certificate the caller never registered.
//!
//! A CA certificate's identity is its `certificateId`, which is why the store is keyed by
//! it: the id is the `@httpLabel` of `/cacertificate/{certificateId}` on every
//! single-CA-certificate operation, and it is modelled `length 64..64, pattern
//! ^(0x)?[a-fA-F0-9]+$` (`smithy_query(query="operation",operation="DescribeCACertificate")`)
//! — the SHA-256 digest of the certificate, hex encoded, which is what
//! [`super::certificate_id`] computes from the certificate's own DER. The id is therefore
//! the certificate's own fingerprint: registering the same material twice names the same
//! record, which is what makes a repeat registration a collision rather than a second
//! record of one certificate.
//!
//! The store is the account's CA registry and separate from the DEVICE certificate store
//! ([`super::certificates`]): the two families are reached through different URIs, carry
//! different shapes (`CACertificateDescription` against `CertificateDescription`) and have
//! different statuses (`CACertificateStatus` is `ACTIVE | INACTIVE`, with no transfer or
//! revocation state), so a CA certificate is never reported by a reader of a device
//! certificate and the other way round.

use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use x509_cert::der::oid::AssociatedOid;
use x509_cert::der::{Decode, Encode};
use x509_cert::ext::pkix::BasicConstraints;
use x509_cert::Certificate;

use super::{
    certificate_id, fresh_uuid, read_certificate_material,
    unreadable_certificate_material_message, CERTIFICATE_MODE_DEFAULT,
};
use crate::ops::types::{CACertificateDescription, CertificateValidity, RegistrationConfig};

/// `CACertificateStatus$ACTIVE` — the status of a CA certificate a registration asked to
/// have set as active (`smithy_query(query="shape",shape="CACertificateStatus")`).
pub const CA_CERTIFICATE_STATUS_ACTIVE: &str = "ACTIVE";

/// `CACertificateStatus$INACTIVE` — the status of a CA certificate registered without
/// `setAsActive`, the member that "specifies if the CA certificate is set to active" and
/// is modelled `default false`: the certificate is registered but nothing may be signed
/// by it yet.
pub const CA_CERTIFICATE_STATUS_INACTIVE: &str = "INACTIVE";

/// `AutoRegistrationStatus$ENABLE` — what a CA certificate registered with
/// `allowAutoRegistration` carries: the member "Allows this CA certificate to be used for
/// auto registration of device certificates", and `CACertificateDescription$autoRegistrationStatus`
/// reports "Whether the CA certificate configured for auto registration of device
/// certificates. Valid values are \"ENABLE\" and \"DISABLE\"".
pub const AUTO_REGISTRATION_ENABLE: &str = "ENABLE";

/// `AutoRegistrationStatus$DISABLE` — what a CA certificate registered without
/// `allowAutoRegistration` carries, that member being modelled `default false`.
pub const AUTO_REGISTRATION_DISABLE: &str = "DISABLE";

/// `CertificateMode$SNI_ONLY`: the mode a CA is registered in when the registration
/// proves nothing about the CA's private key, which is why
/// `RegisterCACertificate$verificationCertificate` documents "If certificateMode is
/// SNI_ONLY, the verificationCertificate field must be empty"
/// (`smithy_query(query="shape",shape="CertificateMode")`).
pub const CERTIFICATE_MODE_SNI_ONLY: &str = "SNI_ONLY";

/// How many CA certificates with the SAME CA subject field one account may register:
/// "You can register up to 10 CA certificates with the same CA subject field per Amazon
/// Web Services account" (`RegisterCACertificate`). The documented figure, which is why
/// the registration counts the subject the certificate itself carries rather than
/// anything the request names.
pub const MAX_CA_CERTIFICATES_PER_SUBJECT: usize = 10;

/// How many CA certificates this emulator registers for the account.
///
/// `RegisterCACertificate` documents "There is no limit to the number of CA certificates
/// you can register in your Amazon Web Services account", so this is not that limit: it
/// is the emulator's own ceiling on a store a CALLER grows, one registration at a time,
/// in memory — the same reason [`super::MAX_CERTIFICATES`] exists — and a registration
/// past it is refused with the modelled `LimitExceededException` rather than growing
/// without bound. It is comfortably above the documented per-subject limit, so the rule a
/// caller can actually reach is that one.
pub const MAX_CA_CERTIFICATES: usize = 1_000;

/// One CA certificate the account holds, as every reader of one reports it.
///
/// Each member is either taken from the certificate material (`certificate_pem`,
/// `subject`, `not_before`, `not_after`), resolved from the registration request
/// (`status`, `auto_registration_status`, `registration_config`, `tags`) or derived from
/// the material itself (`certificate_id`, and the ARN minted over it), so no reader has
/// to recompute anything and two readers cannot describe one record differently.
#[derive(Debug, Clone)]
pub struct RegisteredCaCertificate {
    /// The certificate's identity, and its key in the store: the SHA-256 digest of its
    /// DER, hex encoded ([`super::certificate_id`]).
    pub certificate_id: String,
    /// `arn:aws:iot:<region>:<account>:cacert/<certificateId>`, minted on the
    /// registration ([`ca_certificate_arn`]).
    pub certificate_arn: String,
    /// "The CA certificate data, in PEM format" — the material the registration carried,
    /// kept verbatim so a reader hands back what the caller registered.
    pub certificate_pem: String,
    /// "The status of a CA certificate": [`CA_CERTIFICATE_STATUS_ACTIVE`] or
    /// [`CA_CERTIFICATE_STATUS_INACTIVE`], resolved on the write from `setAsActive`.
    pub status: String,
    /// "Whether the CA certificate configured for auto registration of device
    /// certificates": [`AUTO_REGISTRATION_ENABLE`] or [`AUTO_REGISTRATION_DISABLE`],
    /// resolved on the write from `allowAutoRegistration`.
    pub auto_registration_status: String,
    /// "The owner of the CA certificate" — the account this emulator serves.
    pub owned_by: String,
    /// "The date the CA certificate was created", meaning the instant it was registered.
    pub creation_date: SystemTime,
    /// "The date the CA certificate was last modified" — equal to
    /// [`RegisteredCaCertificate::creation_date`] until a later write moves it.
    pub last_modified_date: SystemTime,
    /// "The customer version of the CA certificate", which is
    /// [`super::FIRST_CERTIFICATE_CUSTOMER_VERSION`] on the registration: the member is
    /// modelled `range min 1`.
    pub customer_version: i32,
    /// "The generation ID of the CA certificate".
    pub generation_id: String,
    /// When the registered certificate starts being valid, read out of its own
    /// `tbsCertificate.validity`.
    pub not_before: SystemTime,
    /// When the registered certificate stops being valid, read out of the same place.
    pub not_after: SystemTime,
    /// "The mode of the CA": [`super::CERTIFICATE_MODE_DEFAULT`] or
    /// [`CERTIFICATE_MODE_SNI_ONLY`], the mode the registration resolved.
    pub certificate_mode: String,
    /// The CA SUBJECT FIELD the certificate certifies under, as the RFC 4514 string of
    /// its own `tbsCertificate.subject`. It is not a modelled member of any shape: it is
    /// the dimension the documented per-account limit counts over
    /// ([`MAX_CA_CERTIFICATES_PER_SUBJECT`]), so it is read out of the material once, on
    /// the write, rather than re-parsed by every later registration.
    pub subject: String,
    /// "Information about the registration configuration", as the registration named it —
    /// absent when it named none, which is what `DescribeCACertificate` then reports.
    pub registration_config: Option<RegistrationConfig>,
    /// "Metadata which can be used to manage the CA certificate", keyed by `Tag$Key`
    /// because that member is `@required` and keys a resource's tag set.
    pub tags: BTreeMap<String, Option<String>>,
}

impl RegisteredCaCertificate {
    /// This record as the `CACertificateDescription` every reader of one CA certificate
    /// reports: one function, so two readers cannot describe the same record differently.
    pub fn description(&self) -> CACertificateDescription {
        CACertificateDescription {
            certificate_arn: Some(self.certificate_arn.clone()),
            certificate_id: Some(self.certificate_id.clone()),
            status: Some(self.status.clone()),
            certificate_pem: Some(self.certificate_pem.clone()),
            owned_by: Some(self.owned_by.clone()),
            creation_date: Some(::aws_smithy_types::DateTime::from(self.creation_date)),
            auto_registration_status: Some(self.auto_registration_status.clone()),
            last_modified_date: Some(::aws_smithy_types::DateTime::from(self.last_modified_date)),
            customer_version: Some(self.customer_version),
            generation_id: Some(self.generation_id.clone()),
            validity: Some(CertificateValidity {
                not_before: Some(::aws_smithy_types::DateTime::from(self.not_before)),
                not_after: Some(::aws_smithy_types::DateTime::from(self.not_after)),
            }),
            certificate_mode: Some(self.certificate_mode.clone()),
        }
    }
}

/// The ARN of a CA certificate: `arn:aws:iot:<region>:<account>:cacert/<certificateId>`.
///
/// `cacert/` is the resource type AWS IoT CA certificate ARNs use — the resource type
/// that keeps a registered CA apart from the `cert/` ARNs of the device certificates it
/// signs ([`super::certificate_arn`]); the namespace and account segments are the ones
/// [`super::thing_arn`] documents.
pub fn ca_certificate_arn(region: &str, certificate_id: &str) -> String {
    ::emulator_server::emulator_arn("iot", region, &format!("cacert/{certificate_id}"))
}

/// What one `RegisterCACertificate` asks for, as the registration reads it.
///
/// A struct rather than a parameter list because the request is what the one write takes:
/// the certificate material, the mode it is registered in, and the three things the
/// caller chooses about the record (its status, whether device certificates may
/// auto-register under it, and the metadata it carries).
#[derive(Debug, Clone, Default)]
pub struct CaCertificateRegistration {
    /// `caCertificate` — "The CA certificate", PEM as the caller sent it.
    pub ca_certificate: String,
    /// `verificationCertificate` — "The private key verification certificate", absent
    /// when the registration sent none.
    pub verification_certificate: Option<String>,
    /// `certificateMode` — absent when the request named none, which "defaults to
    /// DEFAULT".
    pub certificate_mode: Option<String>,
    /// `setAsActive` — "A boolean value that specifies if the CA certificate is set to
    /// active".
    pub set_as_active: bool,
    /// `allowAutoRegistration` — "Allows this CA certificate to be used for auto
    /// registration of device certificates".
    pub allow_auto_registration: bool,
    /// `registrationConfig` — "Information about the registration configuration".
    pub registration_config: Option<RegistrationConfig>,
    /// `tags` — "Metadata which can be used to manage the CA certificate", keyed by tag
    /// key.
    pub tags: BTreeMap<String, Option<String>>,
}

/// Why a CA certificate was NOT registered.
///
/// Each variant names the modelled error class the refusal answers with, so the one rule
/// `RegisterCACertificate` follows is stated once, beside the store it writes, rather
/// than in the handler.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CaCertificateRegistrationRefusal {
    /// Certificate material this service cannot read as an X.509 certificate authority:
    /// `CertificateValidationException` (HTTP 400).
    InvalidCertificate(String),
    /// A request whose members contradict each other — a mode and a verification
    /// certificate that cannot go together: `InvalidRequestException` (HTTP 400). It is
    /// the REQUEST that is refused and not the account's state, so it is read before the
    /// store is consulted.
    InvalidRequest(String),
    /// Material the account has already registered: `ResourceAlreadyExistsException`
    /// (HTTP 409), which also reports the record that already exists.
    AlreadyRegistered {
        /// The sentence the refusal states.
        message: String,
        /// `resourceId` — the id the existing record is held under.
        certificate_id: String,
        /// `resourceArn` — the ARN that record carries.
        certificate_arn: String,
    },
    /// One subject field registered [`MAX_CA_CERTIFICATES_PER_SUBJECT`] times already:
    /// `LimitExceededException` (HTTP 410).
    SubjectLimitExceeded(String),
    /// An account already holding [`MAX_CA_CERTIFICATES`] CA certificates: the same
    /// modelled `LimitExceededException` (HTTP 410), since what is exceeded is a limit
    /// either way, with its own sentence naming which.
    RegistryFull(String),
}

/// The member a `RegisterCACertificate` carries the CA material at, which is what a
/// refusal of unreadable material names ([`unreadable_certificate_material_message`]).
const CA_CERTIFICATE_MEMBER: &str = "caCertificate";

/// The message a registration refuses material it cannot read as a certificate with: the
/// one rendering every operation that takes certificate material refuses unreadable
/// material with ([`unreadable_certificate_material_message`]), naming this operation's
/// member, so a caller can tell it from the refusal below.
pub fn unreadable_ca_certificate_message() -> String {
    unreadable_certificate_material_message(CA_CERTIFICATE_MEMBER)
}

/// The message a registration refuses a certificate that is not a certificate AUTHORITY
/// with: material that is a readable certificate but certifies nothing, which is what the
/// operation's whole purpose — registering "a CA certificate" whose device certificates
/// this account will then trust — requires of it.
///
/// Worded from the certificate's own `basicConstraints` extension (RFC 5280 section
/// 4.2.1.9), which is where that property of a certificate is written, for the same
/// reason [`unreadable_ca_certificate_message`] is worded from the material.
pub fn not_a_certificate_authority_message() -> String {
    "Invalid certificate: the caCertificate field is not a certificate authority (its \
     basicConstraints extension does not assert cA TRUE)"
        .to_string()
}

/// The message a registration in `DEFAULT` mode refuses a request carrying no
/// verification certificate with, worded from the rule
/// `RegisterCACertificate$verificationCertificate` states: "If certificateMode is DEFAULT
/// or not provided, the verificationCertificate field must not be empty".
pub fn verification_certificate_required_message() -> String {
    format!(
        "The verificationCertificate field must not be empty when certificateMode is \
         {CERTIFICATE_MODE_DEFAULT}"
    )
}

/// The message a registration in `SNI_ONLY` mode refuses a request that carries a
/// verification certificate with, worded from the other half of that same rule: "If
/// certificateMode is SNI_ONLY, the verificationCertificate field must be empty".
pub fn verification_certificate_not_allowed_message() -> String {
    format!(
        "The verificationCertificate field must be empty when certificateMode is \
         {CERTIFICATE_MODE_SNI_ONLY}"
    )
}

/// The message a registration refuses the subject field's
/// [`MAX_CA_CERTIFICATES_PER_SUBJECT`]th registration with, worded from the rule
/// `RegisterCACertificate` states: "You can register up to 10 CA certificates with the
/// same CA subject field per Amazon Web Services account", with the limit taken from the
/// constant that enforces it.
pub fn ca_subject_limit_message() -> String {
    format!(
        "You can register up to {MAX_CA_CERTIFICATES_PER_SUBJECT} CA certificates with the same \
         CA subject field per Amazon Web Services account"
    )
}

/// The message a registration past the emulator's own capacity is refused with, naming
/// the ceiling that refused it ([`MAX_CA_CERTIFICATES`]).
pub fn ca_registry_full_message() -> String {
    format!("this emulator registers at most {MAX_CA_CERTIFICATES} CA certificates")
}

/// The sentence every operation that addresses ONE registered CA certificate refuses an id
/// the account's CA registry holds none under with.
///
/// The CA family words its miss after the RESOURCE the request addressed — the CA
/// certificate the account was asked for — rather than after the `certificateId` member the
/// DEVICE certificate family names its own miss by ([`super::missing_certificate_message`]):
/// the two registries are reached through different URIs and answer for different
/// resources, so the id is not part of this text.
pub const MISSING_CA_CERTIFICATE_MESSAGE: &str = "Requested CA Certificate does not exist.";

/// The message a delete refuses a CA certificate whose status is
/// [`CA_CERTIFICATE_STATUS_ACTIVE`] with.
///
/// An ACTIVE CA is one the account still auto-registers and validates device certificates
/// against, so it is deactivated before it is removed — the same precondition the DEVICE
/// certificate family states ([`super::active_certificate_deletion_message`]) — and the
/// sentence names the CA by the id the request carried, because the caller's next step,
/// `UpdateCACertificate`, takes that same id.
///
/// Like every other refusal of this plane ([`MISSING_CA_CERTIFICATE_MESSAGE`]), the
/// sentence names the resource the request addressed — a CA certificate, reached through
/// `/cacertificate/{certificateId}` — so a caller can tell this refusal from the DEVICE
/// certificate family's identically-shaped one, whose remediation is `UpdateCertificate`
/// rather than `UpdateCACertificate`.
pub fn active_ca_certificate_deletion_message(certificate_id: &str) -> String {
    format!("CA Certificate must be deactivated (not ACTIVE) before deletion. Id: {certificate_id}")
}

/// What one `UpdateCACertificate` asks to change about a registered CA certificate.
///
/// A struct rather than a parameter list for the same reason
/// [`CaCertificateRegistration`] is one: the request is what the one write takes. Every
/// member is absent when the request named none, which is what leaves the record's own
/// value standing — `removeAutoRegistration` is modelled `default false`, so its absence
/// is `false`.
#[derive(Debug, Clone, Default)]
pub struct CaCertificateUpdate {
    /// `newStatus` — "The updated status of the CA certificate":
    /// [`CA_CERTIFICATE_STATUS_ACTIVE`] or [`CA_CERTIFICATE_STATUS_INACTIVE`].
    pub new_status: Option<String>,
    /// `newAutoRegistrationStatus` — "The new value for the auto registration status":
    /// [`AUTO_REGISTRATION_ENABLE`] or [`AUTO_REGISTRATION_DISABLE`].
    pub new_auto_registration_status: Option<String>,
    /// `registrationConfig` — "Information about the registration configuration".
    pub registration_config: Option<RegistrationConfig>,
    /// `removeAutoRegistration` — "If true, removes auto registration".
    pub remove_auto_registration: bool,
}

/// The members of a `registrationConfig`, as a value two states of one record are compared
/// by: the modelled shape derives no equality, and what a write has to decide is whether
/// it changed the record at all.
fn registration_config_members(
    registration_config: Option<&RegistrationConfig>,
) -> Option<(Option<String>, Option<String>, Option<String>)> {
    registration_config.map(|configuration| {
        (
            configuration.template_body.clone(),
            configuration.role_arn.clone(),
            configuration.template_name.clone(),
        )
    })
}

/// Why a CA certificate was NOT deleted.
///
/// Each variant names the modelled error class the refusal answers with
/// (`smithy_query(query="operation",operation="DeleteCACertificate")`), so the handler
/// chooses the status from the state rather than restating the rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CaCertificateDeletionRefusal {
    /// An id the account's CA registry holds none under: `ResourceNotFoundException`
    /// (HTTP 404).
    NotFound(String),
    /// A CA certificate whose status is [`CA_CERTIFICATE_STATUS_ACTIVE`]:
    /// `CertificateStateException` (HTTP 406), the one state this operation models.
    Active(String),
}

impl CaCertificateDeletionRefusal {
    /// The message the refusal states, whichever class it names.
    pub fn message(&self) -> &str {
        match self {
            Self::NotFound(message) | Self::Active(message) => message,
        }
    }
}

impl super::ComAmazonawsIotAwsiotserviceState {
    /// Delete the CA certificate recorded under `certificate_id`, answering the record that
    /// was removed, or the reason the account's state held it back.
    ///
    /// "Deletes a registered CA certificate" (`DeleteCACertificate`): the id is the
    /// record's identity and its key in the registry, so removing that one record is the
    /// whole write — every later reader of the registry stops reporting it, and the
    /// `registrationConfig` it carried stops holding the provisioning template that
    /// configuration named back ([`super::provisioning_templates`]).
    ///
    /// What holds it back is the account's own state: an id the registry holds nothing
    /// under, and a record still in [`CA_CERTIFICATE_STATUS_ACTIVE`] — the status this
    /// operation models `CertificateStateException` ("The certificate operation is not
    /// allowed", HTTP 406) for, the same precondition the DEVICE certificate family states
    /// ([`super::ComAmazonawsIotAwsiotserviceState::delete_certificate`]).
    pub fn delete_ca_certificate(
        &mut self,
        certificate_id: &str,
    ) -> Result<RegisteredCaCertificate, CaCertificateDeletionRefusal> {
        let Some(certificate) = self.ca_certificates.get(certificate_id) else {
            return Err(CaCertificateDeletionRefusal::NotFound(
                MISSING_CA_CERTIFICATE_MESSAGE.to_string(),
            ));
        };
        if certificate.status == CA_CERTIFICATE_STATUS_ACTIVE {
            return Err(CaCertificateDeletionRefusal::Active(
                active_ca_certificate_deletion_message(certificate_id),
            ));
        }
        self.ca_certificates.remove(certificate_id).ok_or_else(|| {
            CaCertificateDeletionRefusal::NotFound(MISSING_CA_CERTIFICATE_MESSAGE.to_string())
        })
    }

    /// Apply `update` to the CA certificate recorded under `certificate_id`, answering the
    /// record as it stands afterwards, or `None` when the registry holds none under that id.
    ///
    /// "Updates a registered CA certificate" (`UpdateCACertificate`): every member the
    /// request carries is one the record holds, so the update writes the ones the request
    /// named and leaves the rest as they stand — `newStatus` is the status
    /// ([`CA_CERTIFICATE_STATUS_ACTIVE`] or [`CA_CERTIFICATE_STATUS_INACTIVE`]), which is
    /// what decides whether device certificates may be signed by this CA and therefore what
    /// `DeleteCACertificate`'s precondition reads ([`active_ca_certificate_deletion_message`]);
    /// `newAutoRegistrationStatus` is "The new value for the auto registration status"; and
    /// `registrationConfig` is "Information about the registration configuration".
    /// `removeAutoRegistration` — "If true, removes auto registration" — is applied last and
    /// takes the record out of auto registration altogether: the status goes to
    /// [`AUTO_REGISTRATION_DISABLE`] and the configuration auto registration provisioned
    /// through is dropped.
    ///
    /// A call that leaves every one of those members as it found them is a write that
    /// changed nothing, so it moves neither `lastModifiedDate` nor `customerVersion`; one
    /// that does change the record stamps `now` as the moment it was last modified and takes
    /// the record to its next `customerVersion` — "The customer version of the CA
    /// certificate", the member counting the customer-initiated writes to it, exactly as the
    /// DEVICE certificate family counts its own
    /// ([`super::ComAmazonawsIotAwsiotserviceState::update_certificate_status`]).
    ///
    /// `newStatus` and `newAutoRegistrationStatus` are checked against the modelled
    /// `CACertificateStatus` and `AutoRegistrationStatus` enums before a handler is reached,
    /// so no value is refused here: what the account holds is the only thing that can refuse
    /// this call, and the only such refusal is an id it holds nothing under.
    pub fn update_ca_certificate(
        &mut self,
        certificate_id: &str,
        update: CaCertificateUpdate,
        now: SystemTime,
    ) -> Option<RegisteredCaCertificate> {
        let certificate = self.ca_certificates.get_mut(certificate_id)?;
        let before = (
            certificate.status.clone(),
            certificate.auto_registration_status.clone(),
            registration_config_members(certificate.registration_config.as_ref()),
        );

        if let Some(new_status) = update.new_status {
            certificate.status = new_status;
        }
        if let Some(new_auto_registration_status) = update.new_auto_registration_status {
            certificate.auto_registration_status = new_auto_registration_status;
        }
        if let Some(registration_config) = update.registration_config {
            certificate.registration_config = Some(registration_config);
        }
        if update.remove_auto_registration {
            certificate.auto_registration_status = AUTO_REGISTRATION_DISABLE.to_string();
            certificate.registration_config = None;
        }

        let after = (
            certificate.status.clone(),
            certificate.auto_registration_status.clone(),
            registration_config_members(certificate.registration_config.as_ref()),
        );
        if after != before {
            certificate.last_modified_date = now;
            certificate.customer_version = certificate.customer_version.saturating_add(1);
        }
        Some(certificate.clone())
    }

    /// Register the CA certificate `request` carries, answering the record that was
    /// written, or the reason the request or the account's state refused it.
    ///
    /// The one write every registration of a CA certificate makes, so the rules are
    /// stated once rather than per handler:
    ///
    /// * the mode and the verification certificate have to agree, which is what
    ///   `RegisterCACertificate$verificationCertificate` documents — a `DEFAULT`
    ///   registration carries one, an `SNI_ONLY` registration carries none — and the
    ///   request is read before the store, because neither value depends on what the
    ///   account holds;
    /// * the material has to be a readable X.509 certificate AUTHORITY, since every
    ///   member the record reports about the certificate is read out of it;
    /// * the material's own fingerprint is the record's id, so material already
    ///   registered is a collision rather than a second record;
    /// * one CA subject field is registered at most
    ///   [`MAX_CA_CERTIFICATES_PER_SUBJECT`] times for the account, and the account holds
    ///   at most [`MAX_CA_CERTIFICATES`] of them altogether.
    ///
    /// AWS additionally proves that the registering caller holds the CA's PRIVATE key, by
    /// requiring the verification certificate to be signed by it over the account's
    /// registration code; that code is the resource `GetRegistrationCode` publishes (see
    /// [`super::registration_code`]), and this registration does not check that the
    /// verification certificate was signed over it, so the verification certificate is read
    /// and required as the model documents and nothing about that proof is claimed.
    pub fn register_ca_certificate(
        &mut self,
        region: &str,
        owned_by: &str,
        request: CaCertificateRegistration,
        now: SystemTime,
    ) -> Result<RegisteredCaCertificate, CaCertificateRegistrationRefusal> {
        let certificate_mode = request
            .certificate_mode
            .unwrap_or_else(|| CERTIFICATE_MODE_DEFAULT.to_string());
        let verification_certificate = request
            .verification_certificate
            .as_deref()
            .map(str::trim)
            .filter(|material| !material.is_empty());

        if certificate_mode == CERTIFICATE_MODE_SNI_ONLY {
            if verification_certificate.is_some() {
                return Err(CaCertificateRegistrationRefusal::InvalidRequest(
                    verification_certificate_not_allowed_message(),
                ));
            }
        } else {
            let Some(material) = verification_certificate else {
                return Err(CaCertificateRegistrationRefusal::InvalidRequest(
                    verification_certificate_required_message(),
                ));
            };
            // The verification certificate is certificate material too, so it is read the
            // same way and refused the same way when it cannot be read.
            if read_certificate_material(material).is_none() {
                return Err(CaCertificateRegistrationRefusal::InvalidCertificate(
                    unreadable_ca_certificate_message(),
                ));
            }
        }

        let Some(certificate) = read_certificate_material(&request.ca_certificate) else {
            return Err(CaCertificateRegistrationRefusal::InvalidCertificate(
                unreadable_ca_certificate_message(),
            ));
        };
        if !is_certificate_authority(&certificate) {
            return Err(CaCertificateRegistrationRefusal::InvalidCertificate(
                not_a_certificate_authority_message(),
            ));
        }
        let Ok(certificate_der) = certificate.to_der() else {
            return Err(CaCertificateRegistrationRefusal::InvalidCertificate(
                unreadable_ca_certificate_message(),
            ));
        };
        let certificate_id = certificate_id(&certificate_der);

        if let Some(existing) = self.ca_certificates.get(&certificate_id) {
            return Err(CaCertificateRegistrationRefusal::AlreadyRegistered {
                message: super::existing_certificate_message(&existing.certificate_id),
                certificate_id: existing.certificate_id.clone(),
                certificate_arn: existing.certificate_arn.clone(),
            });
        }

        let subject = certificate.tbs_certificate.subject.to_string();
        let registered_under_subject = self
            .ca_certificates
            .values()
            .filter(|held| held.subject == subject)
            .count();
        if registered_under_subject >= MAX_CA_CERTIFICATES_PER_SUBJECT {
            return Err(CaCertificateRegistrationRefusal::SubjectLimitExceeded(
                ca_subject_limit_message(),
            ));
        }

        // The store is one a caller grows, so the account's whole registry is bounded
        // too. The ceiling is read AFTER the collision check, because material already
        // registered adds no record.
        if self.ca_certificates.len() >= MAX_CA_CERTIFICATES {
            return Err(CaCertificateRegistrationRefusal::RegistryFull(
                ca_registry_full_message(),
            ));
        }

        let validity = &certificate.tbs_certificate.validity;
        let status = if request.set_as_active {
            CA_CERTIFICATE_STATUS_ACTIVE
        } else {
            CA_CERTIFICATE_STATUS_INACTIVE
        };
        let auto_registration_status = if request.allow_auto_registration {
            AUTO_REGISTRATION_ENABLE
        } else {
            AUTO_REGISTRATION_DISABLE
        };
        let registered = RegisteredCaCertificate {
            certificate_arn: ca_certificate_arn(region, &certificate_id),
            certificate_id: certificate_id.clone(),
            certificate_pem: request.ca_certificate,
            status: status.to_string(),
            auto_registration_status: auto_registration_status.to_string(),
            owned_by: owned_by.to_string(),
            creation_date: now,
            last_modified_date: now,
            customer_version: super::FIRST_CERTIFICATE_CUSTOMER_VERSION,
            generation_id: fresh_uuid(),
            not_before: UNIX_EPOCH + validity.not_before.to_unix_duration(),
            not_after: UNIX_EPOCH + validity.not_after.to_unix_duration(),
            certificate_mode,
            subject,
            registration_config: request.registration_config,
            tags: request.tags,
        };
        self.ca_certificates
            .insert(certificate_id, registered.clone());
        Ok(registered)
    }
}

/// Whether `certificate` is a certificate AUTHORITY: whether its `basicConstraints`
/// extension asserts `cA TRUE`, which is where RFC 5280 section 4.2.1.9 says that
/// property of a certificate is written ("The basic constraints extension identifies
/// whether the subject of the certificate is a CA").
fn is_certificate_authority(certificate: &Certificate) -> bool {
    certificate
        .tbs_certificate
        .extensions
        .iter()
        .flatten()
        .filter(|extension| extension.extn_id == BasicConstraints::OID)
        .any(|extension| {
            BasicConstraints::from_der(extension.extn_value.as_bytes())
                .is_ok_and(|constraints| constraints.ca)
        })
}
