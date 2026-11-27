// SPDX-License-Identifier: CC0-1.0

//! ONE email identity of the account, and the states it carries: whether it names an
//! address or a domain, where its verification and its DKIM setup stand, the custom MAIL
//! FROM configuration it may have, and the S/MIME certificate associations it holds.
//!
//! Split out of the state module root so this one concern lives in its own file; every item
//! is re-exported from [`crate::state`], so a caller still reaches it as `crate::state::…`.
//! What the account's identity REGISTRY does with such a record — publishing it so the other
//! SES surfaces over the same identities answer for it — stays in the module root beside the
//! registry, and whether the DKIM records a domain identity waits on are published is
//! [`super::dkim_publication`]'s.

use super::*;

/// Whether an `EmailIdentity` names an address or a domain.
///
/// The model's `IdentityType` also carries `MANAGED_DOMAIN`, which its own
/// documentation says is "not supported for email identity types", so nothing here
/// produces it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentityKind {
    /// `IdentityType` = `EMAIL_ADDRESS`.
    EmailAddress,
    /// `IdentityType` = `DOMAIN`.
    Domain,
}

impl IdentityKind {
    /// An `EmailIdentity` that carries a local part (`@`) is an address; anything
    /// else is a domain.
    pub fn of(identity: &str) -> Self {
        if identity.contains('@') {
            Self::EmailAddress
        } else {
            Self::Domain
        }
    }

    /// The modelled `IdentityType` value.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::EmailAddress => "EMAIL_ADDRESS",
            Self::Domain => "DOMAIN",
        }
    }
}

/// The value set shared by the model's `VerificationStatus`, `DkimStatus` and
/// `MailFromDomainStatus` enums. Only the states this emulator can reach are
/// represented, and `NOT_STARTED` is not one of `MailFromDomainStatus`'s values, so
/// no MAIL FROM record carries it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SesStatus {
    /// The process has not been initiated for this identity.
    NotStarted,
    /// The process was initiated and has not completed.
    Pending,
    /// The process completed successfully.
    Success,
    /// Verification failed.
    ///
    /// Nothing in this service moves an identity here — its own verification either
    /// completes or stays `PENDING` — but the account's identity registry can carry
    /// the state for an identity the other SES surface holds, and an identity this
    /// service adopts from the registry is reported in the state the account holds it
    /// in rather than in a state it is not in.
    Failed,
    /// Verification failed for a reason that may clear on its own; carried for the
    /// same reason as [`Self::Failed`].
    TemporaryFailure,
}

impl SesStatus {
    /// The modelled enum value.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NotStarted => "NOT_STARTED",
            Self::Pending => "PENDING",
            Self::Success => "SUCCESS",
            Self::Failed => "FAILED",
            Self::TemporaryFailure => "TEMPORARY_FAILURE",
        }
    }
}

/// How DKIM was configured for an identity (`DkimSigningAttributesOrigin`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DkimOrigin {
    /// Easy DKIM: SES generated the key pair and published the tokens.
    AwsSes,
    /// Bring Your Own DKIM: the caller supplied the selector and private key.
    External,
}

impl DkimOrigin {
    /// The modelled enum value.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AwsSes => "AWS_SES",
            Self::External => "EXTERNAL",
        }
    }
}

/// The DKIM configuration stored for one identity.
#[derive(Debug, Clone)]
pub struct DkimState {
    /// Whether messages from the identity are DKIM-signed.
    pub signing_enabled: bool,
    /// The DKIM authentication status.
    pub status: SesStatus,
    /// Easy DKIM CNAME tokens, or the BYODKIM selector.
    pub tokens: Vec<String>,
    /// How DKIM was configured.
    pub origin: DkimOrigin,
    /// `[Easy DKIM]` the key length of the key pair in use, when SES owns the key.
    pub signing_key_length: Option<String>,
}

impl DkimState {
    /// Easy DKIM for a domain: SES owns the key pair, publishes three CNAME tokens
    /// and waits for them to appear in the domain's DNS.
    pub fn easy_dkim(domain: &str, key_length: Option<String>) -> Self {
        Self {
            signing_enabled: true,
            status: SesStatus::Pending,
            tokens: easy_dkim_tokens(domain),
            origin: DkimOrigin::AwsSes,
            // `DkimSigningKeyLength` has two values; SES generates 2048-bit keys
            // unless the request asks for the other one.
            signing_key_length: Some(key_length.unwrap_or_else(|| "RSA_2048_BIT".to_string())),
        }
    }

    /// BYODKIM for a domain: the caller owns the key pair, so the stored token set
    /// is the selector that names its public key and SES reports no key length.
    pub fn byo_dkim(selector: &str) -> Self {
        Self {
            signing_enabled: true,
            status: SesStatus::Pending,
            tokens: vec![selector.to_string()],
            origin: DkimOrigin::External,
            signing_key_length: None,
        }
    }

    /// An address identity: DKIM signing applies to domains, so nothing is
    /// configured and the authentication process was never initiated.
    pub fn not_started() -> Self {
        Self {
            signing_enabled: false,
            status: SesStatus::NotStarted,
            tokens: Vec::new(),
            origin: DkimOrigin::AwsSes,
            signing_key_length: None,
        }
    }

    /// The Easy DKIM setup the ACCOUNT holds for the identity `name`, as this surface
    /// records it.
    ///
    /// The setup belongs to the account's identity rather than to the API that asked
    /// for it, so a record of this surface reports the tokens its owner was told to
    /// publish, whether messages from it are signed and where the search for the
    /// records stands exactly as the account's identity registry carries them. Easy
    /// DKIM by definition: the registry carries a setup only where SES generated the
    /// key pair and the token set.
    pub fn shared(name: &str, shared: &::service_common::ses_identities::SesDkimSetup) -> Self {
        let mut dkim = Self::easy_dkim(name, None);
        dkim.signing_enabled = shared.signing_enabled;
        dkim.status = adopted_status(shared.status);
        dkim.tokens = shared.tokens.clone();
        dkim
    }
}

/// The key length SES reports as the next Easy-DKIM key pair's for an identity it
/// holds no Easy-DKIM key configuration for: an address identity, or a domain whose
/// key pair the caller owns. SES has generated no pair for such an identity and no
/// request has named a length for the next one, so the member reports the shortest
/// length `DkimSigningKeyLength` publishes rather than a length nothing chose.
pub const DKIM_BASE_SIGNING_KEY_LENGTH: &str = "RSA_1024_BIT";

/// The `BehaviorOnMxFailure` a custom MAIL FROM configuration takes when the
/// request that set it named none: SES then sends the mail using its own domain
/// rather than refusing it.
pub const BEHAVIOR_ON_MX_FAILURE_DEFAULT: &str = "USE_DEFAULT_VALUE";

/// The custom MAIL FROM configuration stored for one identity.
#[derive(Debug, Clone)]
pub struct MailFromState {
    /// `MailFromDomain`, as the request that configured it spelled the domain.
    pub domain: String,
    /// `MailFromDomainStatus`: where the search for the domain's MX record stands.
    pub status: SesStatus,
    /// `BehaviorOnMxFailure`: what sending does while the configuration is not in
    /// the `SUCCESS` state.
    pub behavior_on_mx_failure: String,
    /// When the `PENDING` -> `SUCCESS` transition falls due.
    pub mx_lookup_due_at: SystemTime,
}

impl MailFromState {
    /// A freshly configured custom MAIL FROM domain: "PENDING - Amazon SES hasn't
    /// started searching for the MX record yet", so the configuration starts there
    /// and the search's outcome falls due [`MAIL_FROM_MX_SETTLE`] later.
    pub fn new(domain: &str, behavior_on_mx_failure: String, now: SystemTime) -> Self {
        Self {
            domain: domain.to_string(),
            status: SesStatus::Pending,
            behavior_on_mx_failure,
            mx_lookup_due_at: now.checked_add(MAIL_FROM_MX_SETTLE).unwrap_or(now),
        }
    }

    /// Apply the transition the clock owes this configuration at `now`.
    ///
    /// SES reports `SUCCESS` once it "detected the required MX record for the MAIL
    /// FROM domain". Nothing here resolves DNS, so the emulator takes the caller at
    /// its word that the record it was told to publish is published — the same
    /// assumption the identity's own verification and its Easy DKIM tokens are
    /// settled under.
    pub fn settle(&mut self, now: SystemTime) {
        if self.status == SesStatus::Pending && now >= self.mx_lookup_due_at {
            self.status = SesStatus::Success;
        }
    }

    /// The modelled `MailFromAttributes` view of this record.
    pub fn view(&self) -> crate::ops::types::MailFromAttributes {
        crate::ops::types::MailFromAttributes {
            mail_from_domain: Some(self.domain.clone()),
            mail_from_domain_status: Some(self.status.as_str().to_string()),
            behavior_on_mx_failure: self.behavior_on_mx_failure.clone(),
        }
    }
}

/// The modelled `MailFromAttributes` of an identity with NO custom MAIL FROM domain.
///
/// SES reports the object for every identity, because `BehaviorOnMxFailure` states what
/// sending does when the required MX record cannot be found whether or not a custom
/// domain was ever configured: with none configured the documented default applies
/// ([`BEHAVIOR_ON_MX_FAILURE_DEFAULT`], under which "the mail is sent using
/// amazonses.com as the MAIL FROM domain"). There is no domain and no MX search to
/// report for such an identity, so those two members carry no value.
pub fn unconfigured_mail_from_view() -> crate::ops::types::MailFromAttributes {
    crate::ops::types::MailFromAttributes {
        mail_from_domain: None,
        mail_from_domain_status: None,
        behavior_on_mx_failure: BEHAVIOR_ON_MX_FAILURE_DEFAULT.to_string(),
    }
}

/// The status of one certificate association (`IdentityCertificateStatus`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CertificateStatus {
    /// "The certificate association was created and the certificate is being
    /// prepared for use."
    Provisioning,
    /// "The certificate is ready to use for signing."
    Active,
    /// "The certificate is no longer used for signing" — what an association that has
    /// been disassociated carries, until SES cleans it up.
    Inactive,
    /// "The certificate association is being cleaned up."
    Deprovisioning,
    /// "The certificate couldn't be prepared for use, or the certificate has
    /// expired" — where the preparation of an association whose certificate cannot be
    /// signed with ends.
    Failed,
}

impl CertificateStatus {
    /// The modelled enum value.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Provisioning => "PROVISIONING",
            Self::Active => "ACTIVE",
            Self::Inactive => "INACTIVE",
            Self::Deprovisioning => "DEPROVISIONING",
            Self::Failed => "FAILED",
        }
    }
}

/// One S/MIME certificate association: the address its certificate signs for, the
/// ACM certificate it names, and where the association is in its lifecycle.
#[derive(Debug, Clone)]
pub struct IdentityCertificateRecord {
    /// `FromAddress`: the address the certificate applies to, as the request spelled
    /// it.
    pub from_address: String,
    /// `CertificateArn`: the ACM certificate the association names.
    pub certificate_arn: String,
    /// Whether the certificate the association names can be prepared for use at all, as
    /// the service that HOLDS that certificate answered when the association was created
    /// (see [`Self::new`]). It is what the status the association is created in says:
    /// `PROVISIONING` for a certificate that can be signed with, `FAILED` for one that
    /// cannot.
    pub certificate_is_usable: bool,
    /// Where the association is in its lifecycle.
    pub status: CertificateStatus,
    /// When this status's clock-owed transition falls due: `PROVISIONING` -> `ACTIVE`,
    /// `INACTIVE` -> `DEPROVISIONING`, and the clean-up that removes a `DEPROVISIONING`
    /// association. A status the clock owes nothing further carries the instant the
    /// association reached it.
    pub status_due_at: SystemTime,
}

impl IdentityCertificateRecord {
    /// A fresh association, as `AssociateEmailIdentityCertificate` stores it.
    ///
    /// `certificate_is_usable` is the answer of the service that HOLDS the certificate
    /// the ARN names, taken here because creating the association is when SES reads that
    /// certificate, and it is what the status the association starts in says:
    ///
    /// * a certificate that can be signed with is one there is something to prepare out
    ///   of, so "when the association is created, the certificate begins provisioning and
    ///   its status is PROVISIONING", and "the status changes to ACTIVE when the
    ///   certificate is ready to use for signing" — the span [`CERTIFICATE_SETTLE`] later
    ///   ([`Self::settle`]);
    /// * a certificate that cannot be signed with — one the holder has none of under that
    ///   ARN, has not issued, whose validity has run out, that may not protect email or
    ///   that no authority issued — is one no preparation can be made out of, so the
    ///   association is created in the terminal `FAILED` state ("the certificate couldn't
    ///   be prepared for use, or the certificate has expired"): nothing about it is left
    ///   for the clock to decide, and the first read reports it.
    pub fn new(
        from_address: &str,
        certificate_arn: &str,
        certificate_is_usable: bool,
        now: SystemTime,
    ) -> Self {
        let (status, status_due_at) = if certificate_is_usable {
            (
                CertificateStatus::Provisioning,
                now.checked_add(CERTIFICATE_SETTLE).unwrap_or(now),
            )
        } else {
            (CertificateStatus::Failed, now)
        };
        Self {
            from_address: from_address.to_string(),
            certificate_arn: certificate_arn.to_string(),
            certificate_is_usable,
            status,
            status_due_at,
        }
    }

    /// Stop signing with this association, as `DisassociateEmailIdentityCertificate`
    /// does: "after the association is removed, Amazon SES API v2 stops adding an
    /// S/MIME signature to messages sent from that address", so the certificate "is
    /// no longer used for signing" at once, and SES then cleans the association up.
    pub fn deactivate(&mut self, now: SystemTime) {
        self.status = CertificateStatus::Inactive;
        self.status_due_at = now.checked_add(CERTIFICATE_SETTLE).unwrap_or(now);
    }

    /// Whether this association is one the identity signs with: a `PROVISIONING` one
    /// is being prepared for signing and an `ACTIVE` one is ready for it, while a
    /// `FAILED` one never became usable and a disassociated one is on its way out, so
    /// neither signs anything.
    pub fn signs(&self) -> bool {
        matches!(
            self.status,
            CertificateStatus::Provisioning | CertificateStatus::Active
        )
    }

    /// Whether this association is still in force — one the account holds rather than
    /// one already on its way out.
    ///
    /// A `FAILED` association is in force in this sense even though it signs nothing:
    /// the account still holds the record, so removing it is a change rather than the
    /// no-change case a disassociation of an association already leaving is.
    pub fn in_force(&self) -> bool {
        !matches!(
            self.status,
            CertificateStatus::Inactive | CertificateStatus::Deprovisioning
        )
    }

    /// Apply every transition the clock owes this association at `now`, and report
    /// whether the association still exists afterwards: a disassociated one is cleaned
    /// up, and once that clean-up falls due it is gone.
    pub fn settle(&mut self, now: SystemTime) -> bool {
        loop {
            if now < self.status_due_at {
                return true;
            }
            match self.status {
                // "The status changes to ACTIVE when the certificate is ready to use for
                // signing": only an association whose certificate can be signed with is
                // ever being prepared — one whose certificate cannot was created `FAILED`
                // ([`Self::new`]) — so this is where that preparation ends.
                CertificateStatus::Provisioning => {
                    self.status = if self.certificate_is_usable {
                        CertificateStatus::Active
                    } else {
                        CertificateStatus::Failed
                    };
                    return true;
                }
                // Each is a state the association stays in until a request moves it, so
                // neither owes the clock anything further.
                CertificateStatus::Active | CertificateStatus::Failed => return true,
                CertificateStatus::Inactive => {
                    self.status = CertificateStatus::Deprovisioning;
                    // Measured from the instant the clean-up was owed, not from this
                    // call, so a store nobody read for a while settles all the way
                    // through rather than one step per read.
                    self.status_due_at = self
                        .status_due_at
                        .checked_add(CERTIFICATE_SETTLE)
                        .unwrap_or(now);
                }
                CertificateStatus::Deprovisioning => return false,
            }
        }
    }

    /// The modelled `IdentityCertificate` view of this record.
    pub fn view(&self) -> crate::ops::types::IdentityCertificate {
        crate::ops::types::IdentityCertificate {
            from_address: Some(self.from_address.clone()),
            status: Some(self.status.as_str().to_string()),
            certificate_arn: Some(self.certificate_arn.clone()),
            // "The timestamp after which the certificate is no longer valid" is the
            // validity of the ACM certificate this association names, which belongs
            // to that service's state and not to this one: SES reads it from the
            // certificate rather than deciding it, so no expiry is reported here
            // rather than one being made up.
            certificate_expiry_time: None,
        }
    }
}

/// One email identity.
#[derive(Debug, Clone)]
pub struct EmailIdentityRecord {
    /// The `EmailIdentity` this record is keyed by.
    pub name: String,
    /// The Region the account holds this identity in — the Region the request that
    /// recorded it was signed for.
    ///
    /// An email identity is a REGIONAL resource (SES publishes its identity quota "per
    /// account per Region" and mints every identity ARN with a Region in it), so an
    /// operation of this surface answers only for the identities of the Region the
    /// caller signed for ([`Self::in_region`]).
    ///
    /// `None` states NO Region, and such an identity is answered for in every one. That
    /// is what the account's cross-surface identity registry carries: the classic SES
    /// surface over these same identities records no Region with them, so an identity
    /// ADOPTED from the registry (or one a fixture writes straight into the table) is
    /// not an identity of any one Region here, and scoping it to one would make an
    /// identity the account does hold unreachable.
    pub region: Option<String>,
    /// Address or domain.
    pub identity_kind: IdentityKind,
    /// `VerificationStatus`.
    pub verification_status: SesStatus,
    /// `VerifiedForSendingStatus` / `SendingEnabled`.
    pub verified_for_sending: bool,
    /// `FeedbackForwardingStatus`.
    pub feedback_forwarding: bool,
    /// The identity's DKIM configuration.
    pub dkim: DkimState,
    /// The configuration set used by default when sending from this identity.
    pub configuration_set_name: Option<String>,
    /// The custom MAIL FROM configuration, when one is enabled for this identity.
    pub mail_from: Option<MailFromState>,
    /// Tags, in the order they were supplied.
    pub tags: Vec<(String, String)>,
    /// Sending-authorization policies, keyed by policy name.
    ///
    /// The set is the ACCOUNT's identity's, so it is the one the account's identity
    /// registry carries
    /// ([`::service_common::ses_identities::SesIdentityRecord::policies`]): a policy
    /// attached through another API over these identities is reported here, and one
    /// attached here is reported there. Every write publishes it
    /// ([`publish_identity`]).
    pub policies: BTreeMap<String, String>,
    /// When the identity was created.
    pub created_at: SystemTime,
    /// When the `PENDING` -> `SUCCESS` verification transition falls due.
    pub verification_due_at: SystemTime,
    /// When the last search of this identity's DNS records was made, which
    /// `VerificationInfo$LastCheckedTimestamp` reports. `None` until the first one
    /// has been materialised.
    pub last_checked_at: Option<SystemTime>,
    /// When the next search falls due, [`VERIFICATION_PROBE_INTERVAL`] after the last
    /// one; the first is owed at creation, so it is the instant the identity was
    /// created.
    pub next_check_due_at: SystemTime,
    /// When this identity's verification last succeeded, which
    /// `VerificationInfo$LastSuccessTimestamp` reports. `None` while it never has.
    pub last_success_at: Option<SystemTime>,
    /// When the search of the account's own DNS first FOUND the Easy DKIM records a
    /// domain identity's owner was told to publish, and `None` while it has not found
    /// them — which is where a domain identity starts and stays until they are there.
    ///
    /// What the search finds is read on every pass rather than remembered
    /// ([`resolve_dkim_publication`]); this records only WHEN it first succeeded, which
    /// is the instant the verification move was brought forward from. A verification
    /// nothing is waiting on DNS for — an address identity's, one the account completed
    /// another way — carries `None` and is untouched by that pass.
    pub dkim_records_found_at: Option<SystemTime>,
    /// The S/MIME certificate associations this identity holds, keyed by
    /// [`identity_key`] of the address each applies to — "each email address can have
    /// only one certificate association". Ordered, so
    /// `ListEmailIdentityCertificates` enumerates and pages deterministically.
    pub certificates: BTreeMap<String, IdentityCertificateRecord>,
    /// Whether the account's identity registry
    /// ([`::service_common::ses_identities::SesIdentityDirectory`]) already carries
    /// this identity.
    ///
    /// The registry is the account's SET of identities, which the classic SES surface
    /// publishes to and reads as well, so this flag is what tells an identity DELETED
    /// through that surface — registered here, gone from the registry — from one this
    /// service has created but not published yet ([`reconcile_identities`]).
    pub registered: bool,
    /// Whether a verification move is OWED to this identity at all.
    ///
    /// True for an identity this service started verifying: the move to `SUCCESS` is
    /// owed at [`Self::verification_due_at`]. False only for an identity
    /// ADOPTED from the account's identity registry in a state the registry owes no
    /// move to — a verification that failed, was never started, or cannot complete —
    /// so a read of it here does not complete a verification nothing was waiting on.
    pub verification_move_owed: bool,
}

impl EmailIdentityRecord {
    /// Start verifying `name`, as `CreateEmailIdentity` does. `kind` is the
    /// classification the caller already computed from [`addr_spec`], so the record
    /// and its creator cannot disagree about it.
    ///
    /// The record states NO Region ([`Self::region`]): the operation that records the
    /// identity sets the Region it was signed for, and a caller that states none holds
    /// the identity in every Region.
    pub fn new(name: &str, kind: IdentityKind, dkim: DkimState, now: SystemTime) -> Self {
        Self {
            name: name.to_string(),
            region: None,
            identity_kind: kind,
            // "Starts the process of verifying an email identity": the identity is
            // not verified yet, so it cannot send yet either.
            verification_status: SesStatus::Pending,
            verified_for_sending: false,
            // SES forwards bounces and complaints to the identity's owner unless
            // the caller turns it off with PutEmailIdentityFeedbackAttributes.
            feedback_forwarding: true,
            dkim,
            configuration_set_name: None,
            // A new identity sends with SES's own MAIL FROM domain until
            // `PutEmailIdentityMailFromAttributes` configures a custom one.
            mail_from: None,
            tags: Vec::new(),
            policies: BTreeMap::new(),
            created_at: now,
            // WHEN the move to `SUCCESS` is owed, which is what the identity is waiting
            // ON: a DOMAIN is verified by the DKIM records its owner publishes in that
            // domain's DNS, which nobody has published yet, so the move stays owed at
            // the far end of the window SES searches for them in and is brought forward
            // by the pass that finds them ([`resolve_dkim_publication`]). An
            // email-address identity is verified by the recipient of the message SES
            // sends to that address, a step this emulator owes the clock rather than the
            // DNS.
            verification_due_at: match kind {
                IdentityKind::Domain => now
                    .checked_add(DKIM_RECORD_SEARCH_WINDOW)
                    .unwrap_or(now),
                IdentityKind::EmailAddress => {
                    now.checked_add(VERIFICATION_SETTLE).unwrap_or(now)
                }
            },
            // Nothing has been found in the domain's DNS yet; the first search is owed
            // below.
            dkim_records_found_at: None,
            // SES starts looking for the records it told the caller to publish as
            // soon as verification starts, so the first search is owed at once.
            last_checked_at: None,
            next_check_due_at: now,
            last_success_at: None,
            certificates: BTreeMap::new(),
            // Published to the account registry by the operation that created the
            // identity, so the classic SES surface answers for it from the moment
            // that call returns.
            registered: false,
            // "Starts the process of verifying an email identity": the move to
            // `SUCCESS` is owed from here.
            verification_move_owed: true,
        }
    }

    /// An identity of this account that the registry holds and this service does not,
    /// as this service's own record of it.
    ///
    /// The account holds the identity in the verification state the registry carries,
    /// so the record is adopted in THAT state rather than started afresh in `PENDING`:
    /// the identity was recorded through another SES surface, and both surfaces report
    /// one state for it. The same goes for the facts the account holds about it rather
    /// than this API — its Easy DKIM setup and its sending-authorization policies.
    /// Everything this service's own API adds — tags, a default configuration set, a
    /// custom MAIL FROM configuration, certificate associations — starts where
    /// `CreateEmailIdentity` starts it, since none of it has been asked for.
    pub fn adopted(
        record: &::service_common::ses_identities::SesIdentityRecord,
        now: SystemTime,
    ) -> Self {
        use ::service_common::ses_identities::SesIdentityKind;

        let kind = match record.kind {
            SesIdentityKind::EmailAddress => IdentityKind::EmailAddress,
            SesIdentityKind::Domain => IdentityKind::Domain,
        };
        // The DKIM configuration this record is adopted with. Easy DKIM is set up for
        // the ACCOUNT's identity, so an identity the account already holds a setup for
        // — recorded through another API over its identities — is adopted with THAT
        // setup, whose tokens are the records its owner publishes. An identity the
        // account holds NO setup for has had DKIM set up for it by nobody, which is
        // what `NOT_STARTED` reports: a read here must not fabricate a key pair, a
        // token set and a signing state the identity's owner never asked for and no
        // other API over these identities reports. Only a create of this surface's own
        // (`CreateEmailIdentity`) starts Easy DKIM for a domain.
        let dkim = match record.dkim.as_ref() {
            Some(shared) => DkimState::shared(&identity_key(&record.name), shared),
            None => DkimState::not_started(),
        };
        let mut adopted = Self::new(&record.name, kind, dkim, now);
        adopted.registered = true;
        // The policies that authorize other principals to send through the identity are
        // the ACCOUNT's identity's, so they are adopted with it; everything else this
        // service's own API adds starts where `CreateEmailIdentity` starts it.
        adopted.policies = record.policies.clone().unwrap_or_default();
        adopted.verification_status = adopted_status(record.verification);
        // The registry carries the instant the account owes this identity its move,
        // and carries NONE when it owes it none at all: such an identity keeps the
        // state the account holds it in rather than being completed by the next read
        // ([`Self::verification_move_owed`]).
        match record.verification_due {
            Some(due) => adopted.verification_due_at = due,
            None => adopted.verification_move_owed = false,
        }
        if adopted.verification_status == SesStatus::Success {
            // The same three facts this service's own settle writes when a
            // verification completes, so an adopted identity reads like one that
            // completed here.
            adopted.verified_for_sending = true;
            adopted.last_success_at = Some(now);
            if adopted.dkim.status == SesStatus::Pending {
                adopted.dkim.status = SesStatus::Success;
            }
        }
        adopted
    }

    /// The modelled `DkimAttributes` view of this record.
    ///
    /// One projection shared by every operation that reports DKIM state, so a
    /// create and the read after it cannot disagree.
    pub fn dkim_attributes(&self) -> crate::ops::types::DkimAttributes {
        crate::ops::types::DkimAttributes {
            signing_enabled: self.dkim.signing_enabled,
            status: Some(self.dkim.status.as_str().to_string()),
            // An identity with no DKIM configuration has no token set at all,
            // rather than an empty one.
            tokens: (!self.dkim.tokens.is_empty()).then(|| self.dkim.tokens.clone()),
            // SES reports the publishing hosted zone only for identities whose
            // records it publishes itself; nothing here configures one.
            signing_hosted_zone: None,
            signing_attributes_origin: Some(self.dkim.origin.as_str().to_string()),
            // "[Easy DKIM] The key length of the future DKIM key pair to be
            // generated": the length SES would generate this identity's next pair
            // at, which is the length its Easy-DKIM key is configured at until a
            // request names another. SES reports the member for every identity, so
            // one it holds no Easy-DKIM key configuration for reports the service's
            // base length ([`DKIM_BASE_SIGNING_KEY_LENGTH`]).
            next_signing_key_length: Some(
                self.dkim
                    .signing_key_length
                    .clone()
                    .unwrap_or_else(|| DKIM_BASE_SIGNING_KEY_LENGTH.to_string()),
            ),
            current_signing_key_length: self.dkim.signing_key_length.clone(),
            // SES generated the Easy DKIM key pair when it configured DKIM for the
            // identity, which is exactly when it published a token set of its own;
            // with BYODKIM the caller owns the key, so SES generated none, and an
            // identity with no DKIM configuration has no key pair either. Whether
            // signing is currently switched on does not come into it: turning
            // signing off generates and destroys no key.
            last_key_generation_timestamp: (self.dkim.origin == DkimOrigin::AwsSes
                && !self.dkim.tokens.is_empty())
                .then(|| ::aws_smithy_types::DateTime::from(self.created_at)),
        }
    }

    /// The modelled `TagList` view of this record's tags.
    pub fn tag_list(&self) -> Vec<crate::ops::types::Tag> {
        self.tags
            .iter()
            .map(|(key, value)| crate::ops::types::Tag {
                key: key.clone(),
                value: value.clone(),
            })
            .collect()
    }

    /// Whether a request signed for `region` is answered from this identity.
    ///
    /// An identity the account holds in one Region answers in THAT Region alone; one
    /// that states no Region at all ([`Self::region`]) answers in every Region. Every
    /// identity lookup of this service runs through here, so no two operations can
    /// disagree about which Region holds an identity.
    pub fn in_region(&self, region: &str) -> bool {
        self.region.as_deref().is_none_or(|held| held == region)
    }

    /// Whether this identity answers a lookup scoped to `region`, where `None` scopes
    /// the lookup to no Region in particular and every identity answers.
    pub fn answers_for(&self, region: Option<&str>) -> bool {
        region.is_none_or(|asked| self.in_region(asked))
    }

    /// Apply the transitions the clock owes this identity at `now`: the next search
    /// of its DNS records, its verification transition, its custom MAIL FROM domain's
    /// MX search, and each certificate association's own.
    pub fn settle(&mut self, now: SystemTime) {
        // The search for the records SES told the caller to publish: SES re-runs it
        // on its own schedule, so one owed at `now` has been made whether or not
        // anybody called, and the instant it was made is what a read reports.
        if now >= self.next_check_due_at {
            self.last_checked_at = Some(now);
            self.next_check_due_at = now
                .checked_add(VERIFICATION_PROBE_INTERVAL)
                .unwrap_or(now);
        }
        if self.verification_status == SesStatus::Pending
            && self.verification_move_owed
            && now >= self.verification_due_at
            && self.verification_can_complete()
        {
            self.verification_status = SesStatus::Success;
            self.verified_for_sending = true;
            self.last_success_at = Some(now);
            if self.dkim.status == SesStatus::Pending {
                self.dkim.status = SesStatus::Success;
            }
        }
        if let Some(mail_from) = self.mail_from.as_mut() {
            mail_from.settle(now);
        }
        self.certificates
            .retain(|_, certificate| certificate.settle(now));
    }

    /// Whether this identity's verification can ever complete.
    ///
    /// A domain identity is verified by records published in that domain's own DNS
    /// zone, so one whose name no DNS zone can cover ([`dns_zone_can_exist`]) can
    /// never be verified however long the caller waits: the search keeps answering
    /// "no such name", and the identity stays in the state it started in with
    /// `VerificationInfo` saying why (SES gives up on it after days, a span this
    /// emulator does not model). An email-address identity is verified by the
    /// recipient following the link in the verification message, which no DNS name
    /// decides.
    pub(super) fn verification_can_complete(&self) -> bool {
        self.identity_kind != IdentityKind::Domain || dns_zone_can_exist(identity_domain(&self.name))
    }

    /// The modelled `VerificationInfo` view of the search for this identity's DNS
    /// records: when it last ran, when it last succeeded, and the reason the last one
    /// failed while it is failing.
    ///
    /// `ErrorType` is reported only while the records have not been found —
    /// "the reason for the failure describing why Amazon SES was not able to
    /// successfully verify the identity" — and is which of the model's two readings
    /// of a failed lookup applies to this name ([`dkim_lookup_error`]). No
    /// `SOARecord`: SES reports the start-of-authority record it read out of the
    /// zone, and nothing here reads a zone.
    pub fn verification_info(&self) -> crate::ops::types::VerificationInfo {
        crate::ops::types::VerificationInfo {
            last_checked_timestamp: self.last_checked_at.map(::aws_smithy_types::DateTime::from),
            last_success_timestamp: self.last_success_at.map(::aws_smithy_types::DateTime::from),
            error_type: (self.verification_status != SesStatus::Success)
                .then(|| dkim_lookup_error(identity_domain(&self.name)).to_string()),
            soa_record: None,
        }
    }

    /// The recommendations this identity's record yields, with their ARNs minted in
    /// `region`.
    ///
    /// Two of the model's `RecommendationType` values are decided by what this record
    /// carries: `DKIM` by whether SES has found the DKIM records it told the caller to
    /// publish for the domain, and `SPF` by whether a custom MAIL FROM domain is
    /// configured and its MX record found — the two mechanisms this service's own
    /// records say whether a domain has set up. Each is `OPEN` while the mechanism is
    /// not in place and `FIXED` once it is, raised when the identity was created (the
    /// first instant SES had this domain's setup to look at) and last updated when the
    /// search that settled it fell due.
    ///
    /// The model's other six types are NOT derived: `DMARC` and `BIMI` are published
    /// DNS policies for the domain, `COMPLAINT`, `BOUNCE` and `FEEDBACK_3P` are
    /// recipient feedback on delivered mail, and `IP_LISTING` is a third party's
    /// blocklist — none of which anything in this service observes or records, so there
    /// is no finding of those kinds to report rather than one being made up.
    ///
    /// Both mechanisms authenticate a DOMAIN, so an email-address identity yields no
    /// finding: it is the domain identity covering its address that carries the DKIM
    /// and MAIL FROM configuration SES looks at.
    pub fn recommendations(&self, region: &str) -> Vec<crate::ops::types::Recommendation> {
        if self.identity_kind != IdentityKind::Domain {
            return Vec::new();
        }
        let arn = identity_arn(region, &self.name);
        // When the DKIM records were found, the identity's verification succeeded with
        // them, so the instant that search last succeeded is when the finding was
        // resolved; an identity whose DKIM was already in place when it was created (one
        // inheriting a verified parent domain's) has no such instant, and its finding was
        // resolved as it was raised.
        let dkim_fixed_at = (self.dkim.status == SesStatus::Success)
            .then(|| self.last_success_at.unwrap_or(self.created_at));
        let spf_fixed_at = self
            .mail_from
            .as_ref()
            .filter(|mail_from| mail_from.status == SesStatus::Success)
            .map(|mail_from| mail_from.mx_lookup_due_at);
        vec![
            recommendation(
                &arn,
                RECOMMENDATION_TYPE_DKIM,
                DKIM_RECOMMENDATION_DESCRIPTION,
                self.created_at,
                dkim_fixed_at,
            ),
            recommendation(
                &arn,
                RECOMMENDATION_TYPE_SPF,
                SPF_RECOMMENDATION_DESCRIPTION,
                self.created_at,
                spf_fixed_at,
            ),
        ]
    }

    /// Configure `domain` as this identity's custom MAIL FROM domain.
    ///
    /// Setting a domain starts the search for that domain's MX record, so the
    /// configuration enters `PENDING`; re-writing the domain already configured
    /// starts no new search, which is what makes the same request sent twice change
    /// nothing beyond the behaviour it carries.
    pub fn set_mail_from(&mut self, domain: &str, behavior_on_mx_failure: String, now: SystemTime) {
        match self.mail_from.as_mut() {
            Some(mail_from) if domain_key(&mail_from.domain) == domain_key(domain) => {
                // The latest request's spelling is what every read reports, as it is
                // for every other value a request carries.
                mail_from.domain = domain.to_string();
                mail_from.behavior_on_mx_failure = behavior_on_mx_failure;
            }
            _ => {
                self.mail_from = Some(MailFromState::new(domain, behavior_on_mx_failure, now));
            }
        }
    }
}
