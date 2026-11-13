// SPDX-License-Identifier: CC0-1.0

//! # Enclave certificate IAM role associations
//!
//! [`ComAmazonawsEc2Amazonec2State::enclave_certificate_iam_roles`] holds, per ACM
//! certificate ARN, the IAM roles that certificate is associated with, together with the two
//! facts the association surface reports beside a role and that no request carries: the
//! Amazon S3 bucket the certificate bundle was uploaded to, and the KMS key its private key
//! is encrypted with ([`EnclaveCertificateRecord`]).
//!
//! The roles are kept as a [`BTreeSet`] of role ARNs rather than as stored
//! [`AssociatedRole`] shapes, because every member of that shape but the role ARN is the
//! same for every role of ONE certificate or is DERIVED from the pair: "the object key is
//! formatted as follows: `role_arn`/`certificate_arn`" (`AssociatedRole$CertificateS3ObjectKey`).
//! [`associated_enclave_certificate_iam_role`] is that one derivation, so the association
//! call and the report of it cannot describe one association differently.
//!
//! An association has no status member and no lifecycle
//! (`AssociateEnclaveCertificateIamRoleResult` answers the bucket, the key and the KMS key id
//! and nothing else), so nothing here is owed a clock-driven move: the association is in
//! force when the call returns, and it stays in force until it is disassociated.
//!
//! A child module of [`crate::state`] rather than more lines in it: everything here belongs
//! to one subject and reaches the rest of the store through `use super::*`. Everything it
//! declares is re-exported by `crate::state`, so a handler reaches it as
//! `crate::state::require_enclave_certificate_arn` exactly as it reaches the helpers that
//! live in the parent module.

use crate::ops::types::AssociatedRole;

use super::*;

/// The ARN service segment of an ACM certificate, and the signing name of the service that
/// issues one.
const ACM_SERVICE: &str = "acm";

/// The ARN resource type of an ACM certificate: `…:certificate/<id>`, the form every
/// `CertificateArn` member of the ACM model documents
/// (`arn:aws:acm:region:123456789012:certificate/12345678-1234-1234-1234-123456789012`).
const CERTIFICATE_RESOURCE_TYPE: &str = "certificate";

/// The fewest and the most characters an ACM certificate ARN may carry: the `@length` ACM's
/// own `Arn` shape puts on every certificate ARN member it types
/// (`smithy_query(query="shape",shape="Arn",model_path="services/service-com-amazonaws-acm-certificatemanager/smithy-model/com-amazonaws-acm-certificatemanager.json")`),
/// so an ARN this surface accepts is one ACM could hold a certificate under.
const CERTIFICATE_ARN_CHARS: std::ops::RangeInclusive<usize> = 20..=2048;

/// `InvalidCertificateArn.Malformed` (HTTP 400): the request named something that is not an
/// ACM certificate ARN, so there is no certificate for the association to be about.
pub const INVALID_CERTIFICATE_ARN_MALFORMED: &str = "InvalidCertificateArn.Malformed";

/// `InvalidRoleArn.Malformed` (HTTP 400): the request named something that is not an IAM
/// role ARN, so there is no role for the association to give access to.
pub const INVALID_ROLE_ARN_MALFORMED: &str = "InvalidRoleArn.Malformed";

/// "You can associate up to 16 IAM roles with an ACM certificate"
/// (`AssociateEnclaveCertificateIamRoleRequest$RoleArn`) — the documented ceiling on one
/// certificate's roles, which is where this surface stops rather than at a ceiling of this
/// emulator's choosing.
pub const MAX_ENCLAVE_CERTIFICATE_IAM_ROLES: usize = 16;

/// Refuse a certificate ARN this surface will not resolve: HTTP 400
/// [`INVALID_CERTIFICATE_ARN_MALFORMED`], in the wording the real service answers — the
/// message carries no part of the request, because what was wrong with it is that it names
/// no certificate at all.
pub fn invalid_certificate_arn_malformed<E>() -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_CERTIFICATE_ARN_MALFORMED,
        400,
        "The request must contain a valid certificate arn",
    )
}

/// Refuse a role ARN this surface will not resolve: HTTP 400
/// [`INVALID_ROLE_ARN_MALFORMED`], worded as the certificate refusal beside it is — the two
/// ARN members of one request are read by one validator, which says which of them it will
/// not take and nothing about the value it was given.
///
/// The certificate half of that pair is the one a recorded response pins; this half follows
/// it by symmetry, so a later recording of a malformed role ARN is what would correct the
/// wording here.
pub fn invalid_role_arn_malformed<E>() -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_ROLE_ARN_MALFORMED,
        400,
        "The request must contain a valid role arn",
    )
}

/// Is `arn` an ACM certificate ARN — `arn:<partition>:acm:<region>:<account>:certificate/<id>`?
///
/// The form ACM's own `Arn` shape fixes for a certificate: six colon-separated fields, the
/// `acm` service segment, a digits-only account, a resource of type
/// [`CERTIFICATE_RESOURCE_TYPE`] carrying a non-empty id, and a length inside
/// [`CERTIFICATE_ARN_CHARS`]. The ACCOUNT is not held to this emulator's own, for the reason
/// [`iam_role_arn_is_well_formed`] gives: the certificate is the caller's.
///
/// The ONE reading of a `CertificateArn` member in this crate, so every operation that takes a
/// certificate ARN accepts exactly the same set of values — the enclave certificate surface
/// whose refusals are declared beside it, and [`super::acm_certificates`], which resolves one
/// against the service that holds it. It is deliberately narrower than ACM's `Arn` pattern in
/// two ways that pattern leaves open and a real certificate ARN never uses: the Region segment
/// may not be empty, and the resource id is one segment, so `…:certificate/<a>/<b>` is not one
/// of these either.
pub fn acm_certificate_arn_is_well_formed(arn: &str) -> bool {
    if !CERTIFICATE_ARN_CHARS.contains(&arn.chars().count()) {
        return false;
    }
    let fields: Vec<&str> = arn.split(':').collect();
    let [prefix, partition, service, region, account, resource] = fields.as_slice() else {
        return false;
    };
    let partition_is_well_formed = !partition.is_empty()
        && partition
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-');
    let region_is_well_formed = !region.is_empty()
        && region
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-');
    let account_is_well_formed =
        !account.is_empty() && account.bytes().all(|byte| byte.is_ascii_digit());
    *prefix == "arn"
        && partition_is_well_formed
        && *service == ACM_SERVICE
        && region_is_well_formed
        && account_is_well_formed
        && resource
            .strip_prefix(CERTIFICATE_RESOURCE_TYPE)
            .and_then(|rest| rest.strip_prefix('/'))
            .is_some_and(|certificate_id| {
                !certificate_id.is_empty()
                    && certificate_id
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || b"+=,.@_-".contains(&byte))
            })
}

/// The certificate ARN `requested` names, or the refusal for a value that is not one.
///
/// `CertificateArn` is `@required` AND `@clientOptional`, so an EMPTY value reaches a
/// handler rather than being refused at the boundary — and an empty value names no
/// certificate, so it is answered the same refusal as any other value that is not a
/// certificate ARN.
pub fn require_enclave_certificate_arn<E>(requested: &str) -> Result<&str, Scaffolded<E>> {
    if acm_certificate_arn_is_well_formed(requested) {
        Ok(requested)
    } else {
        Err(invalid_certificate_arn_malformed())
    }
}

/// The role ARN `requested` names, or the refusal for a value that is not one.
///
/// Read through [`iam_role_arn_is_well_formed`], the one reading of an IAM role ARN in this
/// crate, so a role this surface accepts is one every other operation that takes a role
/// accepts too.
pub fn require_enclave_certificate_role_arn<E>(requested: &str) -> Result<&str, Scaffolded<E>> {
    if iam_role_arn_is_well_formed(requested) {
        Ok(requested)
    } else {
        Err(invalid_role_arn_malformed())
    }
}

/// The Amazon S3 bucket an enclave certificate bundle is uploaded to, for an account's
/// certificates in `region`.
///
/// "The name of the Amazon S3 bucket to which the certificate was uploaded"
/// (`AssociateEnclaveCertificateIamRoleResult$CertificateS3BucketName`). The bucket is the
/// SERVICE's, not the caller's — no request names it — and AWS publishes no spelling for
/// its name, so the name is CHOSEN: a fixed prefix, the Region the first association was
/// made in and the account the store belongs to, which keeps it inside the 63 characters an
/// S3 bucket name may carry and makes it one name per account and Region rather than a value
/// that differs per call.
pub fn enclave_certificate_s3_bucket_name(region: &str) -> String {
    format!(
        "aws-ec2-enclave-certificate-{region}-{account}",
        account = ::engine::emulator_account_id()
    )
}

/// The Amazon S3 object key the certificate, its chain and its encrypted private key are
/// stored under for one association.
///
/// "The object key is formatted as follows: `role_arn`/`certificate_arn`"
/// (`AssociatedRole$CertificateS3ObjectKey`) — derived from the pair rather than stored, so
/// the association call and every later report of it answer one key.
pub fn enclave_certificate_s3_object_key(role_arn: &str, certificate_arn: &str) -> String {
    format!("{role_arn}/{certificate_arn}")
}

/// What this store holds for ONE ACM certificate: the roles associated with it, and the two
/// facts of the certificate's bundle that every one of those associations reports.
///
/// The bucket and the KMS key id are the certificate's rather than the role's — "the Amazon
/// S3 bucket in which the Amazon S3 object is stored" and "the ID of the KMS key used to
/// encrypt the private key" (`AssociatedRole`) are facts of the uploaded bundle — so they are
/// decided once, when the certificate is first associated with a role, and every later
/// association of that certificate reports the same two values.
#[derive(Debug, Clone)]
pub struct EnclaveCertificateRecord {
    /// The bucket the certificate bundle was uploaded to
    /// ([`enclave_certificate_s3_bucket_name`] for the Region of the first association).
    pub s3_bucket_name: String,
    /// The KMS key the bundle's private key is encrypted with: a key id this store MINTED
    /// for the certificate, never a value from a request.
    pub encryption_kms_key_id: String,
    /// The ARNs of the IAM roles the certificate is associated with, at most
    /// [`MAX_ENCLAVE_CERTIFICATE_IAM_ROLES`] of them. A set, because one role is associated
    /// with one certificate once; ordered by ARN, the one stable order a report of them
    /// reads.
    pub role_arns: BTreeSet<String>,
}

/// The caller-chosen bytes one association retains: the certificate ARN and the role ARN,
/// which are the only parts of the record a caller sizes (the bucket name and the KMS key id
/// are this service's own).
///
/// The ONE definition of that charge: a writer charges exactly it.
pub fn enclave_certificate_iam_role_payload_bytes(certificate_arn: &str, role_arn: &str) -> usize {
    certificate_arn.len().saturating_add(role_arn.len())
}

/// One association as every report of it reads — the modelled [`AssociatedRole`] shape.
///
/// The one projection of a stored association, so `AssociateEnclaveCertificateIamRole` and
/// `GetAssociatedEnclaveCertificateIamRoles` cannot answer one association differently.
pub fn associated_enclave_certificate_iam_role(
    certificate_arn: &str,
    role_arn: &str,
    record: &EnclaveCertificateRecord,
) -> AssociatedRole {
    AssociatedRole {
        associated_role_arn: Some(role_arn.to_string()),
        certificate_s3_bucket_name: Some(record.s3_bucket_name.clone()),
        certificate_s3_object_key: Some(enclave_certificate_s3_object_key(
            role_arn,
            certificate_arn,
        )),
        encryption_kms_key_id: Some(record.encryption_kms_key_id.clone()),
    }
}

impl ComAmazonawsEc2Amazonec2State {
    /// The IAM roles associated with `certificate_arn`, as every report of them reads.
    ///
    /// A pure READ: a certificate this store holds no association for has an empty list of
    /// roles rather than no answer, which is what the real service reports for a certificate
    /// nothing has been associated with.
    pub fn associated_enclave_certificate_iam_roles(
        &self,
        certificate_arn: &str,
    ) -> Vec<AssociatedRole> {
        let Some(record) = self.enclave_certificate_iam_roles.get(certificate_arn) else {
            return Vec::new();
        };
        record
            .role_arns
            .iter()
            .map(|role_arn| {
                associated_enclave_certificate_iam_role(certificate_arn, role_arn, record)
            })
            .collect()
    }

    /// How many IAM roles `certificate_arn` is already associated with.
    ///
    /// What a writer asks against [`MAX_ENCLAVE_CERTIFICATE_IAM_ROLES`] before associating
    /// another one.
    pub fn enclave_certificate_iam_role_count(&self, certificate_arn: &str) -> usize {
        self.enclave_certificate_iam_roles
            .get(certificate_arn)
            .map_or(0, |record| record.role_arns.len())
    }

    /// Whether `certificate_arn` is already associated with `role_arn`.
    ///
    /// One role is associated with one certificate ONCE, so a request that names a pair this
    /// store already holds adds nothing — it is answered with the association that is
    /// already in force.
    pub fn enclave_certificate_has_iam_role(&self, certificate_arn: &str, role_arn: &str) -> bool {
        self.enclave_certificate_iam_roles
            .get(certificate_arn)
            .is_some_and(|record| record.role_arns.contains(role_arn))
    }

    /// Associate `role_arn` with `certificate_arn` and answer the association as every
    /// report of it will read, or `None` when this store has no room for it.
    ///
    /// The ONE writer seam. Everything the service owns is decided here rather than taken
    /// from the caller — the bucket the bundle is uploaded to and the KMS key its private key
    /// is encrypted with, both minted for the certificate the FIRST time it is associated
    /// with a role and unchanged by every later association of it — so two writers cannot
    /// report one certificate's bundle differently. Associating a pair this store already
    /// holds writes nothing and answers the association in force, which is what makes a
    /// repeated request idempotent rather than a second association of one role.
    ///
    /// `region` is the Region the request was made in, used only when the certificate is
    /// first associated: it names the bucket, and a certificate's bucket does not move
    /// afterwards.
    pub fn associate_enclave_certificate_iam_role(
        &mut self,
        certificate_arn: &str,
        role_arn: &str,
        region: &str,
    ) -> Option<AssociatedRole> {
        if let Some(record) = self.enclave_certificate_iam_roles.get(certificate_arn) {
            if record.role_arns.contains(role_arn) {
                return Some(associated_enclave_certificate_iam_role(
                    certificate_arn,
                    role_arn,
                    record,
                ));
            }
        }
        let payload_bytes =
            enclave_certificate_iam_role_payload_bytes(certificate_arn, role_arn)
                .saturating_add(STORED_RECORD_OVERHEAD_BYTES);
        if self.would_exceed_stored_bytes(payload_bytes) {
            return None;
        }
        let is_new_certificate = !self
            .enclave_certificate_iam_roles
            .contains_key(certificate_arn);
        if is_new_certificate
            && self.enclave_certificate_iam_roles.len() >= MAX_STORED_RECORDS_PER_STORE
        {
            return None;
        }
        if is_new_certificate {
            // The bundle's bucket and its encryption key are decided ONCE, here: a
            // certificate already in this store keeps the two it was first stored with. The
            // key id is minted as a bare UUID, which is the form a KMS key id takes.
            let record = EnclaveCertificateRecord {
                s3_bucket_name: enclave_certificate_s3_bucket_name(region),
                encryption_kms_key_id: self.mint_uuid_suffixed_id(""),
                role_arns: BTreeSet::new(),
            };
            self.enclave_certificate_iam_roles
                .insert(certificate_arn.to_string(), record);
        }
        let record = self.enclave_certificate_iam_roles.get_mut(certificate_arn)?;
        record.role_arns.insert(role_arn.to_string());
        let association =
            associated_enclave_certificate_iam_role(certificate_arn, role_arn, record);
        self.add_stored_bytes(payload_bytes);
        Some(association)
    }

    /// Disassociate `role_arn` from `certificate_arn`.
    ///
    /// The ONE remover seam, the exact inverse of
    /// [`Self::associate_enclave_certificate_iam_role`]: the role goes, the bytes that
    /// association was charged go back to the store's budget, and the certificate's record
    /// goes with the LAST of its roles — "disassociating an IAM role from an ACM certificate
    /// removes the Amazon S3 object that contains the certificate, certificate chain, and
    /// encrypted private key from the Amazon S3 bucket" (`DisassociateEnclaveCertificateIamRole`),
    /// so a certificate no role is associated with has no bundle stored for it, and a later
    /// association of it uploads one again and decides its bucket and encryption key then.
    ///
    /// A pair this store does not hold leaves it exactly as it was: there is no association
    /// to remove, and the state the request asks for — this role has no access to this
    /// certificate — is the state that already holds.
    pub fn disassociate_enclave_certificate_iam_role(
        &mut self,
        certificate_arn: &str,
        role_arn: &str,
    ) {
        let Some(record) = self.enclave_certificate_iam_roles.get_mut(certificate_arn) else {
            return;
        };
        if !record.role_arns.remove(role_arn) {
            return;
        }
        if record.role_arns.is_empty() {
            self.enclave_certificate_iam_roles.remove(certificate_arn);
        }
        self.release_stored_bytes(
            enclave_certificate_iam_role_payload_bytes(certificate_arn, role_arn)
                .saturating_add(STORED_RECORD_OVERHEAD_BYTES),
        );
    }
}
