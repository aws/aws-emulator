// SPDX-License-Identifier: CC0-1.0

//! The cluster certificate authority slice of this service's state: the signing
//! and distribution statuses a CA moves through, the spans the clock owes those
//! moves, the record the store keeps for one, and the activation an update holds
//! until it is due.

use super::*;

/// The `CertificateAuthoritySigningStatus` members this slice uses. The model's
/// enum (`smithy_query query="shape" shape="CertificateAuthoritySigningStatus"`)
/// is `NOT_USED | ACTIVATING | IN_USE`; `CertificateAuthority$signingStatus` is a
/// plain string member, so the status is stored as the wire token. `ACTIVATING`
/// is the status a CA reports while the activation update that promotes it to
/// the cluster's signer is in flight.
pub const CA_SIGNING_NOT_USED: &str = "NOT_USED";
/// See [`CA_SIGNING_NOT_USED`].
pub const CA_SIGNING_IN_USE: &str = "IN_USE";
/// See [`CA_SIGNING_NOT_USED`].
pub const CA_SIGNING_ACTIVATING: &str = "ACTIVATING";

/// The `CertificateAuthorityDistributionStatus` members this slice uses, from
/// `smithy_query query="shape" shape="CertificateAuthorityDistributionStatus"`
/// (`IN_PROGRESS | COMPLETE | FAILED | DELETING`). `FAILED` is written by no
/// operation here: this emulator's trust distribution never fails. `DELETING` is
/// what a certificate authority reports while the update that removes it from the
/// cluster's trust bundle is in flight.
pub const CA_DISTRIBUTION_IN_PROGRESS: &str = "IN_PROGRESS";
/// See [`CA_DISTRIBUTION_IN_PROGRESS`].
pub const CA_DISTRIBUTION_COMPLETE: &str = "COMPLETE";
/// See [`CA_DISTRIBUTION_IN_PROGRESS`].
pub const CA_DISTRIBUTION_DELETING: &str = "DELETING";

/// The `CertificateAuthorityCreatedBy`/`CertificateAuthorityActivatedBy` members
/// (both `EKS | CUSTOMER`): "Certificate authorities that you create are
/// `CUSTOMER`; those that Amazon EKS provisions on your behalf, such as a
/// cluster's initial certificate authority, are `EKS`"
/// (`CertificateAuthority$createdBy`).
pub const CA_BY_EKS: &str = "EKS";
/// See [`CA_BY_EKS`].
pub const CA_BY_CUSTOMER: &str = "CUSTOMER";

/// How many certificate authorities one cluster holds at a time: "Each cluster
/// can have at most two certificate authorities at a time: the outgoing CA
/// that's currently signing (its signingStatus is IN_USE) and one successor CA
/// (signingStatus of NOT_USED)" (`CreateCertificateAuthority`). A cluster
/// already holding this many is refused a further append as a request that is
/// invalid given the state the cluster is in, and the ceiling is the one value
/// that refusal interpolates. It also bounds
/// [`ClusterRecord::certificate_authorities`].
pub const MAX_CERTIFICATE_AUTHORITIES_PER_CLUSTER: usize = 2;

/// The sentence `DeleteCertificateAuthority` refuses the certificate authority
/// that signs for the cluster with.
///
/// "You can't delete the certificate authority that's currently signing
/// certificates for the cluster" — the way out is to promote the successor
/// first, so the refusal carries that remediation. It is entirely fixed text:
/// the request named the cluster and the certificate authority in its own two
/// `@httpLabel`s, so there is nothing about them the caller could not know.
pub const ACTIVE_CERTIFICATE_AUTHORITY_UNDELETABLE: &str =
    "Cannot delete the active certificate authority. Activate a different CA first.";

/// How long an appended successor CA reports `IN_PROGRESS` distribution before
/// the clock owes it `COMPLETE`.
///
/// "Amazon EKS then distributes the successor CA to the Amazon Web Services
/// managed components in your cluster; you can track this through the CA's
/// distributionStatus" — that distribution IS the asynchronous update the append
/// returns, so it is owed at the same instant the update is owed its `Successful`
/// status ([`UPDATE_COMPLETION_DELAY`], itself CHOSEN rather than cited). The due
/// instant is stored on the record; nothing sleeps for it.
pub const CA_DISTRIBUTION_DELAY: Duration = UPDATE_COMPLETION_DELAY;

/// How long a certificate authority's certificate is valid for.
///
/// `CertificateAuthorityValidity` only says the structure carries the period's
/// `notBefore` and `notAfter`, and no lifetime is published for an EKS cluster
/// CA, so this span is CHOSEN (ten 365-day years) rather than cited. It is
/// recorded on the record at creation, so a read reports the same two instants
/// every time.
pub const CA_VALIDITY: Duration = Duration::from_secs(10 * 365 * 24 * 60 * 60);

/// How long before a CA's `notAfter` Amazon EKS may first auto-activate it
/// ("The earliest Unix epoch timestamp in seconds at which Amazon EKS may
/// automatically activate this certificate authority",
/// `CertificateAuthorityScheduledEvents$firstAutoActivation`). The model states
/// only that the events are "computed from its validity period"; the lead is
/// CHOSEN, and the computation is `notAfter - lead`.
pub const CA_FIRST_AUTO_ACTIVATION_LEAD: Duration = Duration::from_secs(180 * 24 * 60 * 60);
/// The lead for `finalAutoActivation` — "the Unix epoch timestamp in seconds by
/// which Amazon EKS will automatically activate this certificate authority if
/// you haven't already activated it". Shorter than
/// [`CA_FIRST_AUTO_ACTIVATION_LEAD`], and CHOSEN for the same reason.
pub const CA_FINAL_AUTO_ACTIVATION_LEAD: Duration = Duration::from_secs(30 * 24 * 60 * 60);

/// The `UpdateType` member for a certificate-authority change, from
/// `smithy_query query="shape" shape="UpdateType"`.
pub const UPDATE_TYPE_CERTIFICATE_AUTHORITY: &str = "CertificateAuthorityUpdate";
/// The `UpdateParamType` member that names the certificate authority an update
/// is about, from `smithy_query query="shape" shape="UpdateParamType"`.
pub const UPDATE_PARAM_CERTIFICATE_AUTHORITY_ID: &str = "CertificateAuthorityId";
/// The `UpdateParamType` member that names the signing status an update moves a
/// certificate authority to, from the same enum. It is what distinguishes an
/// activation update from the append update that put the CA on the cluster:
/// both carry the one certificate-authority `UpdateType`.
pub const UPDATE_PARAM_SIGNING_STATUS: &str = "SigningStatus";

/// How long after an activation reverting to the outgoing certificate authority
/// is still available.
///
/// "For a limited period after activation, CA rollback is available to revert to
/// the outgoing CA if needed" (`ActivateCertificateAuthority`) — no length is
/// published for that period, so this span is CHOSEN rather than cited: non-zero,
/// so a caller reading the retired CA straight after the activation still sees
/// `rollbackAvailable` true, and in the same band as the other chosen spans here.
/// The closing instant is stored on the record; nothing sleeps for it.
pub const CA_ROLLBACK_WINDOW: Duration = Duration::from_millis(750);

/// The internal service that answers for a cluster's CERTIFICATE AUTHORITIES.
///
/// `DescribeCertificateAuthority` is served by that plane, so a certificate
/// authority it cannot resolve is a refusal Amazon EKS carries through from it —
/// reported WRAPPED, naming the service, the status it answered, the id of that
/// call and how many attempts were made ([`wrapped_peer_tail`]).
pub const CERTIFICATE_AUTHORITY_MANAGER: &str = "EksCertificateAuthority";

/// The sentence behind that wrapped refusal: what the certificate-authority
/// plane says about an id it does not hold.
///
/// Entirely fixed text, and it names NEITHER the certificate authority nor the
/// cluster: both are the request's own `@httpLabel`s, so there is nothing about
/// them the caller could not know.
pub const CERTIFICATE_AUTHORITY_NOT_FOUND_DETAIL: &str = "certificate authority not found";

/// The HTTP status that plane answers an unresolvable certificate authority
/// with, which the wrapped tail reports.
pub(super) const CERTIFICATE_AUTHORITY_NOT_FOUND_STATUS: u16 = 404;

/// The `ResourceNotFoundException` a READ of a certificate authority the named
/// cluster does not hold — one never appended, or one a deletion has taken out
/// of the trust bundle — is refused with.
///
/// The request-id in the tail is drawn afresh per call, because that is what a
/// request id IS: it identifies the call that was made, and nothing in the
/// request decides it.
///
/// It is this operation's own refusal rather than the service's generic one:
/// `ActivateCertificateAuthority` and `DeleteCertificateAuthority` each word the
/// same absence as their own recordings show — the activation as the CA not
/// being in the cluster's trusted list — so the three are not routed through one
/// sentence.
pub fn certificate_authority_not_found() -> ResourceNotFoundException {
    child_resource_not_found(format!(
        "{CERTIFICATE_AUTHORITY_NOT_FOUND_DETAIL}{}",
        wrapped_peer_tail(
            CERTIFICATE_AUTHORITY_MANAGER,
            CERTIFICATE_AUTHORITY_NOT_FOUND_STATUS
        )
    ))
}

/// The id of the `seq`th certificate authority of the cluster `cluster_arn`
/// identifies.
///
/// The model documents the member only as "the unique identifier of the
/// certificate authority", so the form is this emulator's: the UUID shape EKS
/// uses for the ids it mints (`Update$id` is documented as "a UUID"), rendered by
/// the one [`hyphenate_uuid`] every id of that shape goes through. It is derived
/// from the cluster ARN — which already carries the account, region and cluster
/// name — and the cluster's CA counter, so it is distinct per CA and stable for
/// the record's whole life. It is computed once, when the CA is created, and
/// stored on the record; no read recomputes it.
pub fn derive_certificate_authority_id(cluster_arn: &str, seq: u64) -> String {
    let digest = Sha256::digest(format!("{cluster_arn}|certificate-authority|{seq}").as_bytes());
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    // RFC 4122 §4.4: the version nibble is 4, and the variant's two top bits are `10`.
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    hyphenate_uuid(&bytes)
}

/// One certificate authority of one cluster, as this service stores it.
///
/// Everything a `CertificateAuthority` reports is held here — including the
/// server-generated parts (`id`, `createdAt`, the validity period and the
/// public-certificate `data`) — so `DescribeCertificateAuthority` returns exactly
/// what the write operation recorded rather than recomputing anything per
/// request.
#[derive(Debug, Clone)]
pub struct CertificateAuthorityRecord {
    pub id: String,
    pub created_at: SystemTime,
    pub created_by: String,
    pub activated_at: Option<SystemTime>,
    pub activated_by: Option<String>,
    pub signing_status: String,
    pub distribution_status: String,
    /// When set, the instant at which the clock owes this CA the move from
    /// `IN_PROGRESS` to `COMPLETE` distribution. Cleared once the move is
    /// materialised.
    pub distribution_complete_at: Option<SystemTime>,
    pub not_before: SystemTime,
    pub not_after: SystemTime,
    /// Until when reverting to this CA is still possible: "After you activate a
    /// successor CA, rollback lets you revert to the outgoing CA for a limited"
    /// period (`CertificateAuthority$rollbackAvailable`). Only an activation
    /// retires an outgoing CA, so this is set on the CA an activation RETIRED,
    /// [`CA_ROLLBACK_WINDOW`] past the instant the activation was owed. A CA that
    /// nothing has superseded reports `rollbackAvailable` false, which is what
    /// this `None` means.
    pub rollback_available_until: Option<SystemTime>,
    /// The Base64-encoded public certificate of this CA
    /// (`CertificateAuthority$data`), written once and read back unchanged.
    pub data: String,
    /// The `@idempotencyToken` of the request that appended this CA, so a retry
    /// of that request is absorbed and answered with this CA.
    pub client_request_token: Option<String>,
    /// The id of the [`UpdateRecord`] that appended this CA to the cluster's trust
    /// bundle ("This is an asynchronous operation that returns an update object",
    /// `CreateCertificateAuthority`), so the append and a retry of it both answer
    /// with the update the cluster actually holds. `None` for the CA Amazon EKS
    /// provisions with the cluster, which no update appended.
    pub update_id: Option<String>,
}

impl CertificateAuthorityRecord {
    /// The cluster's initial certificate authority — the one Amazon EKS
    /// provisions on the caller's behalf, so `createdBy` is `EKS` — already
    /// signing (`IN_USE`) and already distributed (`COMPLETE`) when the cluster
    /// is created. Its `data` is the cluster's own `certificateAuthority.data`,
    /// so the two are one value rather than two copies.
    ///
    /// `data` is the authority the create provisioned
    /// ([`provision_cluster_authority`], whose validity window is the one
    /// computed here because both are taken from the same `now`), rather than
    /// material minted inside: the SAME authority has to sign the certificate the
    /// cluster's Kubernetes API server presents at the endpoint published beside
    /// this member, and the key that signs both lives only inside that one call.
    pub fn initial_for_cluster(cluster_arn: &str, now: SystemTime, data: String) -> Self {
        let not_after = now + CA_VALIDITY;
        Self {
            id: derive_certificate_authority_id(cluster_arn, 0),
            created_at: now,
            created_by: CA_BY_EKS.to_string(),
            activated_at: Some(now),
            activated_by: Some(CA_BY_EKS.to_string()),
            signing_status: CA_SIGNING_IN_USE.to_string(),
            distribution_status: CA_DISTRIBUTION_COMPLETE.to_string(),
            distribution_complete_at: None,
            not_before: now,
            not_after,
            rollback_available_until: None,
            data,
            client_request_token: None,
            update_id: None,
        }
    }

    /// A successor CA appended to a cluster's trust bundle by the caller, so
    /// `createdBy` is `CUSTOMER`. "Appending a successor CA adds its public
    /// certificate to the cluster's trust bundle ... but it doesn't begin signing
    /// certificates", hence `NOT_USED` signing, and its distribution starts
    /// `IN_PROGRESS` with the clock owing `COMPLETE` [`CA_DISTRIBUTION_DELAY`]
    /// later.
    ///
    /// `seq` is how many CAs the cluster has ever had, so the id is distinct from
    /// every id that cluster has already used, and `update_id` names the update
    /// that appends it. `None` for material that does not issue, the same answer
    /// [`Self::initial_for_cluster`] gives.
    pub fn successor(
        cluster_arn: &str,
        seq: u64,
        now: SystemTime,
        client_request_token: Option<String>,
        update_id: String,
    ) -> Option<Self> {
        let not_after = now + CA_VALIDITY;
        Some(Self {
            data: mint_certificate_authority_data(now, not_after)?,
            id: derive_certificate_authority_id(cluster_arn, seq),
            created_at: now,
            created_by: CA_BY_CUSTOMER.to_string(),
            activated_at: None,
            activated_by: None,
            signing_status: CA_SIGNING_NOT_USED.to_string(),
            distribution_status: CA_DISTRIBUTION_IN_PROGRESS.to_string(),
            distribution_complete_at: Some(now + CA_DISTRIBUTION_DELAY),
            not_before: now,
            not_after,
            rollback_available_until: None,
            client_request_token,
            update_id: Some(update_id),
        })
    }

    /// Materialise the distribution move the clock already owes at `now`.
    ///
    /// Like a cluster's `CREATING` -> `ACTIVE` move, this was due at
    /// `distribution_complete_at` whether or not anybody called, so applying it
    /// on a read does not change what a later caller sees. The
    /// certificate-authority operations apply it to the record they report,
    /// which is the only way a CA is ever observed.
    pub fn materialize_due_transitions(&mut self, now: SystemTime) {
        if self.distribution_complete_at.is_some_and(|due| now >= due) {
            self.distribution_status = CA_DISTRIBUTION_COMPLETE.to_string();
            self.distribution_complete_at = None;
        }
    }

    /// The key this certificate authority sorts and pages under in
    /// `ListCertificateAuthorities`, which reports a cluster's certificate
    /// authorities most recently created FIRST.
    ///
    /// One definition serves both the order the listing walks
    /// ([`ClusterRecord::certificate_authorities_newest_first`]) and the key a page
    /// token names ([`encode_scoped_page_token`]), so a token always resumes the
    /// order the listing walks. The key is a string whose BYTE order is the listing
    /// order: the creation instant goes in as the complement of its nanoseconds
    /// since the epoch, zero-padded to the 39 digits of `u128::MAX`, so a later
    /// instant sorts earlier; the id follows it, so two certificate authorities
    /// created within one clock tick still have a total, stable order. An instant
    /// before the epoch has no nanoseconds since it and takes the largest
    /// complement, which sorts it last — where the oldest creation belongs.
    pub fn page_key(&self) -> String {
        let nanos = self
            .created_at
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|since_epoch| since_epoch.as_nanos())
            .unwrap_or(0);
        format!("{:039}/{}", u128::MAX - nanos, self.id)
    }

    /// This record as the modelled `CertificateAuthoritySummary` shape, which the
    /// certificate-authority write operations and `ListCertificateAuthorities`
    /// answer with.
    pub fn to_summary(&self) -> CertificateAuthoritySummary {
        CertificateAuthoritySummary {
            id: Some(self.id.clone()),
            created_at: Some(::aws_smithy_types::DateTime::from(self.created_at)),
            created_by: Some(self.created_by.clone()),
            activated_at: self.activated_at.map(::aws_smithy_types::DateTime::from),
            activated_by: self.activated_by.clone(),
            signing_status: Some(self.signing_status.clone()),
            distribution_status: Some(self.distribution_status.clone()),
        }
    }

    /// This record as the modelled `CertificateAuthority` shape — the summary
    /// plus the validity period, the scheduled auto-activation events computed
    /// from it, whether a rollback to this CA is still available at `now`, and
    /// the public certificate.
    pub fn to_certificate_authority(&self, now: SystemTime) -> CertificateAuthority {
        let summary = self.to_summary();
        CertificateAuthority {
            id: summary.id,
            created_at: summary.created_at,
            created_by: summary.created_by,
            activated_at: summary.activated_at,
            activated_by: summary.activated_by,
            signing_status: summary.signing_status,
            distribution_status: summary.distribution_status,
            validity: Some(CertificateAuthorityValidity {
                not_before: Some(::aws_smithy_types::DateTime::from(self.not_before)),
                not_after: Some(::aws_smithy_types::DateTime::from(self.not_after)),
            }),
            scheduled_events: Some(CertificateAuthorityScheduledEvents {
                first_auto_activation: Some(::aws_smithy_types::DateTime::from(
                    self.not_after - CA_FIRST_AUTO_ACTIVATION_LEAD,
                )),
                final_auto_activation: Some(::aws_smithy_types::DateTime::from(
                    self.not_after - CA_FINAL_AUTO_ACTIVATION_LEAD,
                )),
            }),
            rollback_available: Some(
                self.rollback_available_until
                    .is_some_and(|until| now < until),
            ),
            data: Some(self.data.clone()),
        }
    }

    /// The `UpdateRecord` for the asynchronous update that appends this CA to the
    /// cluster's trust bundle: "This is an asynchronous operation that returns an
    /// update object" (`CreateCertificateAuthority`), and that object is the one
    /// `DescribeUpdate` reads back from the cluster.
    ///
    /// The update's work IS the trust distribution, so it is owed its `Successful`
    /// status at the same instant the CA is owed `COMPLETE` distribution.
    pub fn append_update_record(&self, update_id: String) -> UpdateRecord {
        UpdateRecord {
            id: update_id,
            r#type: UPDATE_TYPE_CERTIFICATE_AUTHORITY.to_string(),
            status: UPDATE_IN_PROGRESS.to_string(),
            created_at: self.created_at,
            // "A key-value map that contains the parameters associated with the
            // update": one entry, keyed by the `UpdateParamType` that names a
            // certificate authority, whose value is the CA this update appends.
            params: vec![UpdateParam {
                r#type: Some(UPDATE_PARAM_CERTIFICATE_AUTHORITY_ID.to_string()),
                value: Some(self.id.clone()),
            }],
            client_request_token: self.client_request_token.clone(),
            succeeds_at: self.distribution_complete_at,
            // This update's work is the trust distribution onto the CA record,
            // not a change to the cluster's own configuration.
            pending_config: None,
            // Nothing has attempted to cancel this update yet; `CancelUpdate`
            // writes both of these.
            cancellation: None,
            cancellation_request_token: None,
            // The trust bundle is the cluster's own, so this is an update of the
            // cluster rather than of a resource inside it.
            scope: UpdateScope::Cluster,
        }
    }
}

/// The promotion an in-flight activation update makes when it completes: which
/// certificate authority becomes the cluster's signer, when, and at whose
/// request.
///
/// It is held on the update ([`PendingClusterConfig::certificate_authority_activation`])
/// rather than written straight away because "this is an asynchronous operation
/// that returns an update object you can track with DescribeUpdate"
/// (`ActivateCertificateAuthority`): the promotion is owed at the instant the
/// update is owed its `Successful` status. `activated_at` is that DUE instant
/// rather than the instant of the call that noticed it, so every caller reading
/// the promoted CA sees the same activation time.
#[derive(Debug, Clone)]
pub struct CertificateAuthorityActivation {
    /// The certificate authority being promoted to signer.
    pub certificate_authority_id: String,
    /// The instant the promotion is owed, which is what the promoted CA reports
    /// as `activatedAt`.
    pub activated_at: SystemTime,
    /// A `CertificateAuthorityActivatedBy` wire token: who asked for it.
    pub activated_by: String,
}

impl CertificateAuthorityActivation {
    /// Promote the named certificate authority to the cluster's signer and retire
    /// whichever certificate authority was signing, which is what completing an
    /// activation update does: "Amazon EKS promotes it to be the cluster's signer
    /// (its signingStatus becomes IN_USE) and the outgoing CA is retired
    /// (NOT_USED). The outgoing CA remains in the cluster's trust bundle but no
    /// longer signs certificates" — so the retired record stays on the cluster,
    /// with the rollback window the activation opens on it.
    pub fn apply(self, record: &mut ClusterRecord) {
        for (id, authority) in record.certificate_authorities.iter_mut() {
            if *id == self.certificate_authority_id {
                authority.signing_status = CA_SIGNING_IN_USE.to_string();
                authority.activated_at = Some(self.activated_at);
                authority.activated_by = Some(self.activated_by.clone());
                // The CA that signs now is not one to revert TO: a rollback
                // window belongs to the CA this promotion retires.
                authority.rollback_available_until = None;
            } else if authority.signing_status != CA_SIGNING_NOT_USED {
                authority.signing_status = CA_SIGNING_NOT_USED.to_string();
                authority.rollback_available_until =
                    self.activated_at.checked_add(CA_ROLLBACK_WINDOW);
            }
        }
    }
}

impl ClusterRecord {
    /// This cluster's certificate authority with `id`, if it holds one.
    pub fn certificate_authority(&self, id: &str) -> Option<&CertificateAuthorityRecord> {
        self.certificate_authorities.get(id)
    }

    /// This cluster's certificate authorities in the order
    /// `ListCertificateAuthorities` reports them: most recently created first, ties
    /// broken by id — [`CertificateAuthorityRecord::page_key`]'s order.
    ///
    /// The records are STORED in a map keyed by certificate-authority id, which is
    /// a lookup index ([`Self::certificate_authority`]) and not a reporting order:
    /// an id is derived from the cluster and a counter, so its byte order says
    /// nothing about when the certificate authority was created.
    pub fn certificate_authorities_newest_first(&self) -> Vec<&CertificateAuthorityRecord> {
        let mut ordered: Vec<&CertificateAuthorityRecord> =
            self.certificate_authorities.values().collect();
        ordered.sort_by_key(|authority| authority.page_key());
        ordered
    }

    /// "An object identifying the certificate authority that is currently signing
    /// certificates for the cluster" (`Certificate$active`).
    ///
    /// DERIVED from the certificate-authority records rather than stored beside
    /// them, so the cluster and `DescribeCertificateAuthority` can never disagree
    /// about which CA signs: an activation moves the signing status on the records
    /// and this follows. `None` while no record signs, which is what a cluster
    /// with no Amazon EKS control plane behind it reports.
    pub fn active_certificate_authority(&self) -> Option<ActiveCertificateAuthority> {
        self.certificate_authorities
            .values()
            .find(|authority| authority.signing_status == CA_SIGNING_IN_USE)
            .map(|authority| ActiveCertificateAuthority {
                id: Some(authority.id.clone()),
                activated_by: authority.activated_by.clone(),
            })
    }

    /// The same record, mutably, so the operation that reports a CA can first
    /// materialise the distribution move the clock already owes it.
    pub fn certificate_authority_mut(
        &mut self,
        id: &str,
    ) -> Option<&mut CertificateAuthorityRecord> {
        self.certificate_authorities.get_mut(id)
    }

    /// The certificate authority this cluster recorded for `client_request_token`,
    /// if any — what makes a retry of the call that appended it idempotent.
    pub fn certificate_authority_for_token(
        &self,
        token: &str,
    ) -> Option<&CertificateAuthorityRecord> {
        self.certificate_authorities
            .values()
            .find(|authority| authority.client_request_token.as_deref() == Some(token))
    }
}
