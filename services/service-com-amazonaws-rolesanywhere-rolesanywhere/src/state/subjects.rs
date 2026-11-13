// SPDX-License-Identifier: CC0-1.0

//! Subjects: the record this service keeps of every temporary-credential request one
//! certificate identity has made.
//!
//! "Gets a subject, which associates a certificate identity with authentication attempts.
//! The subject stores auditing information such as the status of the last authentication
//! attempt, the certificate data used in the attempt, and the last time the associated
//! identity attempted authentication" (`GetSubject`, this service's model). So a subject is
//! not created by any control-plane call: it is written by the data plane, once per
//! credential request, and read back by `GetSubject` / `ListSubjects`.
//!
//! One record per `x509Subject` — the authenticating certificate's own subject
//! distinguished name — so a workload that asks twice is one subject whose `lastSeenAt`
//! moves, rather than two. [`CredentialSummary`] is "the temporary session credentials
//! vended at the last authenticating call with this subject"
//! (`smithy_query(query="shape", shape="SubjectDetail")`, member `credentials`), so the
//! list holds the LAST attempt and nothing older: the record cannot grow with the number of
//! calls made against it.
//!
//! The map itself is bounded by [`MAX_SUBJECTS`], the construct every other store in this
//! service is bounded by.

use std::time::SystemTime;

use crate::ops::types::{CredentialSummary, InstanceProperty, SubjectDetail};

/// The most subjects this emulator holds at once.
///
/// A subject is written by a data-plane request, so the map needs a declared ceiling: no
/// subject can be given a TTL or evicted, because `GetSubject` has to keep answering with
/// what the request that minted it recorded. The value is this emulator's own ceiling, the
/// same construct [`crate::state::MAX_TRUST_ANCHORS`] is, and sits far above any realistic
/// fleet, so a caller never meets it by accident; AWS publishes no such limit.
pub const MAX_SUBJECTS: usize = 10_000;

/// One recorded subject. Every `SubjectDetail` member this service can know about is
/// persisted here; [`Self::to_detail`] is the only place a response is built from it.
#[derive(Debug, Clone)]
pub struct SubjectRecord {
    pub subject_id: String,
    pub subject_arn: String,
    /// The authenticating certificate's subject distinguished name, which is what makes two
    /// requests by the same identity one subject.
    pub x509_subject: String,
    pub enabled: bool,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
    /// "The ISO-8601 timestamp of the last time this subject requested temporary session
    /// credentials."
    pub last_seen_at: SystemTime,
    /// The credentials of the LAST request this subject made, successful or not.
    pub credentials: Vec<CredentialSummary>,
    /// The instance properties the last request presented; empty when it presented none.
    pub instance_properties: Vec<InstanceProperty>,
}

impl SubjectRecord {
    /// The `SubjectDetail` every subject operation returns.
    pub fn to_detail(&self) -> SubjectDetail {
        SubjectDetail {
            subject_arn: Some(self.subject_arn.clone()),
            subject_id: Some(self.subject_id.clone()),
            enabled: Some(self.enabled),
            x509_subject: Some(self.x509_subject.clone()),
            last_seen_at: Some(::aws_smithy_types::DateTime::from(self.last_seen_at)),
            created_at: Some(::aws_smithy_types::DateTime::from(self.created_at)),
            updated_at: Some(::aws_smithy_types::DateTime::from(self.updated_at)),
            credentials: Some(self.credentials.clone()),
            instance_properties: Some(self.instance_properties.clone()),
        }
    }
}

/// The ARN of the subject this id names, in the Region the request was served in: the ARN
/// the record that minted it carries, so a refusal naming a subject that is not stored
/// words the SAME ARN the read of a stored one answers with.
pub fn subject_arn(region: &str, subject_id: &str) -> String {
    ::engine::emulator_arn(
        crate::state::ARN_NAMESPACE,
        region,
        &format!("subject/{subject_id}"),
    )
}

/// One credential request, as the subject record remembers it.
#[derive(Debug, Clone)]
pub struct RecordedAttempt {
    /// The authenticating certificate's subject distinguished name.
    pub x509_subject: String,
    /// The certificate's serial number, as `CredentialSummary$serialNumber` reports it.
    pub serial_number: String,
    /// The issuer of the authenticating certificate.
    pub issuer: String,
    /// The PEM of the certificate the request presented.
    pub x509_certificate_data: String,
    /// Whether the request was refused — `CredentialSummary$failed` "indicates whether the
    /// temporary credential request was successful".
    pub failed: bool,
}

/// Why no subject could be recorded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordRefusal {
    /// The store already holds [`MAX_SUBJECTS`] subjects and this request names a new one.
    AtCapacity,
}

impl RecordRefusal {
    /// The sentence a refused request carries, naming the ceiling that refused it.
    pub fn message(self) -> String {
        match self {
            Self::AtCapacity => format!(
                "this emulator holds at most {MAX_SUBJECTS} subjects; delete none, as \
                 nothing evicts a subject, and no further certificate identity can be \
                 recorded"
            ),
        }
    }
}

impl crate::state::ComAmazonawsRolesanywhereRolesanywhereState {
    /// Record `attempt` against the subject its certificate identity names, and answer the
    /// record as it now stands.
    ///
    /// The subject already held for that `x509Subject` is UPDATED in place — its
    /// `lastSeenAt` and `updatedAt` move and its `credentials` become this attempt's, the
    /// "last authenticating call" the member is documented as carrying — and an identity
    /// this account has not seen before is recorded as a new subject under a freshly minted
    /// id. So the subject id a workload is known by is stable across its calls.
    pub fn record_credential_attempt(
        &mut self,
        region: &str,
        now: SystemTime,
        attempt: &RecordedAttempt,
    ) -> Result<SubjectRecord, RecordRefusal> {
        let credentials = vec![CredentialSummary {
            seen_at: Some(::aws_smithy_types::DateTime::from(now)),
            serial_number: Some(attempt.serial_number.clone()),
            issuer: Some(attempt.issuer.clone()),
            // The credential is the presented certificate, which authenticated this
            // request: a certificate this service refuses outright never reaches here.
            enabled: Some(true),
            x509_certificate_data: Some(attempt.x509_certificate_data.clone()),
            failed: Some(attempt.failed),
        }];
        let held = self
            .subjects
            .values_mut()
            .find(|record| record.x509_subject == attempt.x509_subject);
        if let Some(record) = held {
            record.last_seen_at = now;
            record.updated_at = now;
            record.credentials = credentials;
            return Ok(record.clone());
        }
        if self.subjects.len() >= MAX_SUBJECTS {
            return Err(RecordRefusal::AtCapacity);
        }
        let subject_id = self.mint_uuid();
        let record = SubjectRecord {
            subject_arn: subject_arn(region, &subject_id),
            subject_id: subject_id.clone(),
            x509_subject: attempt.x509_subject.clone(),
            // A subject starts enabled: "the enabled status of the subject"
            // (`smithy_query(query="shape", shape="SubjectDetail")`), and nothing has
            // disabled one that has only just been seen.
            enabled: true,
            created_at: now,
            updated_at: now,
            last_seen_at: now,
            credentials,
            instance_properties: Vec::new(),
        };
        self.subjects.insert(subject_id, record.clone());
        Ok(record)
    }
}
