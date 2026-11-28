// SPDX-License-Identifier: CC0-1.0

//! The AWS Support case a tax-exemption submission opens, written to the control
//! plane that owns support cases.
//!
//! `PutTaxExemption` submits an exemption certificate for review and answers "the
//! customer support case ID" of the case that review happens on
//! (`smithy_query(query="doc", shape="PutTaxExemptionResponse", member="caseId")`),
//! which is why the operation models a failure to "upload the tax exemption
//! document to Amazon Web Services Support case" and the Support "case creation
//! limit for your account". A `caseId` is therefore a handle into AWS Support: the
//! caller hands it to `support:DescribeCases` and `support:DescribeCommunications`
//! to follow the review of the document it submitted.
//!
//! So the case is created where cases live — the AWS Support store on the one
//! shared engine, through that service's own single case writer — rather than
//! being an id this service makes up. The id this service answers is the one that
//! store minted, and the certificate is attached to the case's first
//! communication, so the submitted document is retained by the service that
//! reviews it.
//!
//! A one-directional, guard-free write, exactly like [`super::organization`]'s
//! read: it takes the Support store's lock, holds no other while it does, hands
//! back owned values — so call it with NO guard of this service's own store held.

use ::service_com_amazonaws_support_awssupport_20130415::state as support;

use crate::ops::types::{
    AttachmentUploadException, Authority, CaseCreationLimitExceededException, ExemptionCertificate,
    InternalServerException,
};

/// The `issueType` of the case a submission opens: `customer-service`, one of the
/// two values `CreateCaseRequest$issueType` documents ("You can specify
/// `customer-service` or `technical`"). Reviewing a submitted tax-exemption
/// certificate is an account matter, not a technical fault.
const EXEMPTION_ISSUE_TYPE: &str = "customer-service";

/// Why no case was opened for a submission, as one of the errors
/// `PutTaxExemption` models for exactly these failures.
#[derive(Debug)]
pub enum ExemptionCaseNotOpened {
    /// The engine this service is mounted on holds no AWS Support store at all,
    /// so there is nowhere for the submission's case to be opened. An
    /// unexpected failure of the service rather than anything the request did.
    Unavailable(InternalServerException),
    /// AWS Support would open no further case for this account.
    CaseLimit(CaseCreationLimitExceededException),
    /// The certificate could not be uploaded to the case.
    Upload(AttachmentUploadException),
}

/// Open the support case a tax-exemption submission opens and answer its
/// `caseId`: the id AWS Support minted, which resolves there.
///
/// `accounts` is how many accounts the submission addressed and `certificate` is
/// the document it submitted for review, attached to the case under the name the
/// request gave it.
pub fn open_exemption_case<Eng: Send + Sync + 'static>(
    engine: &Eng,
    exemption_type: &str,
    authority: &Authority,
    accounts: usize,
    certificate: &ExemptionCertificate,
) -> Result<String, ExemptionCaseNotOpened> {
    let cases = support::service_state(engine).ok_or_else(|| {
        ExemptionCaseNotOpened::Unavailable(InternalServerException {
            message: "The Amazon Web Services Support case store is unavailable.".to_string(),
            error_code: None,
        })
    })?;
    let (time_created, time_created_instant) = support::now().ok_or_else(|| {
        ExemptionCaseNotOpened::Unavailable(InternalServerException {
            message: "The time the Amazon Web Services Support case was opened could not be \
                      rendered."
                .to_string(),
            error_code: None,
        })
    })?;

    let submission = support::CaseSubmission {
        subject: subject(exemption_type, authority),
        communication_body: communication_body(exemption_type, authority, accounts, certificate),
        issue_type: Some(EXEMPTION_ISSUE_TYPE.to_string()),
        // The submission names no support service, category, severity or
        // language, and this service has none to put there: those are the codes
        // `support:DescribeServices` and `support:DescribeSeverityLevels`
        // publish for a case a client raises itself.
        service_code: None,
        category_code: None,
        severity_code: None,
        language: None,
        cc_email_addresses: Vec::new(),
        // The certificate is the document the review is about, attached under
        // the file name the request gave it.
        attachments: vec![support::SubmittedFile {
            file_name: certificate.document_name.clone(),
            data: certificate.document_file.clone(),
        }],
    };

    let mut store = cases.lock();
    store
        .open_case(submission, time_created, time_created_instant)
        .map_err(|refusal| match refusal {
            support::CaseNotOpened::StoreFull => {
                ExemptionCaseNotOpened::CaseLimit(CaseCreationLimitExceededException {
                    message: "You've exceeded the Amazon Web Services Support case creation limit \
                              for your account."
                        .to_string(),
                })
            }
            support::CaseNotOpened::AttachmentStoreFull { .. } => {
                ExemptionCaseNotOpened::Upload(AttachmentUploadException {
                    message: "Failed to upload the tax exemption document to Amazon Web Services \
                              Support case."
                        .to_string(),
                })
            }
            // Unreachable: a submission carries no case correspondents, so
            // there is no cc list for the store to refuse.
            support::CaseNotOpened::TooManyCcEmailAddresses(_) => {
                ExemptionCaseNotOpened::Unavailable(InternalServerException {
                    message: "The Amazon Web Services Support case could not be opened."
                        .to_string(),
                    error_code: None,
                })
            }
        })
}

/// The case's `subject`: what the case is about, which is the exemption the
/// submission claims and the jurisdiction it claims it in.
fn subject(exemption_type: &str, authority: &Authority) -> String {
    format!(
        "Tax exemption certificate for review: {} in {}",
        exemption_type.trim(),
        jurisdiction(authority)
    )
}

/// The body that describes the issue — the case's first communication: what was
/// submitted, for how many accounts, and which document is attached.
fn communication_body(
    exemption_type: &str,
    authority: &Authority,
    accounts: usize,
    certificate: &ExemptionCertificate,
) -> String {
    format!(
        "A tax exemption was submitted for review.\nExemption type: {}\nJurisdiction: {}\n\
         Accounts: {accounts}\nCertificate: {} ({} bytes), attached to this case.",
        exemption_type.trim(),
        jurisdiction(authority),
        certificate.document_name,
        certificate.document_file.len()
    )
}

/// The jurisdiction an `Authority` names, as the case text states it: the
/// country, and the state where the exemption is claimed in one.
fn jurisdiction(authority: &Authority) -> String {
    match authority.state.as_deref().map(str::trim) {
        Some(state) if !state.is_empty() => format!("{}/{state}", authority.country.trim()),
        _ => authority.country.trim().to_string(),
    }
}
