// SPDX-License-Identifier: CC0-1.0

//! The account's CERTIFICATE PROVIDERS: the record one holds, and which provider
//! holds an operation's ACCOUNT DEFAULT.
//!
//! A certificate provider is a Lambda function AWS IoT calls instead of signing a
//! certificate itself, together with the operations it is the account's default for —
//! `CreateCertificateProvider$accountDefaultForOperations` is `@required` and modelled
//! `length 1..1` over the single-valued `CertificateProviderOperation` enum
//! (`smithy_query(query="operation",operation="CreateCertificateProvider")`), so every
//! provider claims exactly one operation's default.
//!
//! The name is the provider's identity — the model binds it as the `@httpLabel` of
//! `/certificate-providers/{certificateProviderName}` on every single-provider
//! operation — so the store is a `BTreeMap` keyed by it, and one record carries
//! everything a reader hands back: `DescribeCertificateProvider` after
//! `CreateCertificateProvider` reports the created provider member for member.
//!
//! WHICH provider holds an operation's default is DERIVED from the records
//! ([`super::ComAmazonawsIotAwsiotserviceState::default_certificate_provider`]) and
//! never recorded beside them, so the account cannot hold two defaults for one
//! operation, and the slot a deleted provider held is free again the moment its record
//! is gone.

use std::collections::BTreeMap;
use std::time::SystemTime;

use crate::ops::types::{
    DescribeCertificateProviderOutput, CERTIFICATE_PROVIDER_OPERATION_VALUES,
};

/// Every operation a certificate provider may be the account default for.
///
/// The catalog is the modelled `CertificateProviderOperation` enum's own values
/// (`smithy_query(query="shape",shape="CertificateProviderOperation")`), reached
/// through the generated constant so the two cannot drift.
pub const CERTIFICATE_PROVIDER_OPERATIONS: &[&str] = CERTIFICATE_PROVIDER_OPERATION_VALUES;

/// How many certificate providers this emulator keeps for the account.
///
/// Every provider claims an operation's default and no operation has two
/// ([`super::ComAmazonawsIotAwsiotserviceState::default_certificate_provider`]), so the
/// records an account can hold at once cannot outnumber the operations there are to
/// claim; this is that bound, stated as the store's ceiling.
pub const MAX_CERTIFICATE_PROVIDERS: usize = CERTIFICATE_PROVIDER_OPERATIONS.len();

/// How many tags one certificate provider may carry: the same documented IoT tagging
/// limit [`super::MAX_POLICY_TAGS`] records, for the same reason.
pub const MAX_CERTIFICATE_PROVIDER_TAGS: usize = super::MAX_POLICY_TAGS;

/// The ARN of a certificate provider:
/// `arn:aws:iot:<region>:<account>:certificateprovider/<certificateProviderName>`.
///
/// `certificateprovider/` is the resource type AWS IoT's certificate-provider ARNs
/// use — it is what `CreateCertificateProvider$certificateProviderArn` reports; the
/// namespace and account segments are the ones [`super::thing_arn`] documents.
pub fn certificate_provider_arn(region: &str, certificate_provider_name: &str) -> String {
    ::emulator_server::emulator_arn(
        "iot",
        region,
        &format!("certificateprovider/{certificate_provider_name}"),
    )
}

/// The message every operation that addresses ONE certificate provider refuses a name
/// the account holds none under with.
///
/// The fixed text is the family's — the provider is either one this account created or
/// no provider at all — and the variable part is the name the request asked about,
/// bracketed as this family states a name it is reporting back.
pub fn missing_certificate_provider_message(certificate_provider_name: &str) -> String {
    format!("Certificate provider with name [{certificate_provider_name}] not found")
}

/// How this service refuses a provider that claims an operation's account default
/// while another provider holds it.
///
/// The fixed text is the class's; the variable parts are the provider the request
/// named and the operation whose default it asked for.
pub fn default_provider_conflict_message(
    certificate_provider_name: &str,
    operation: &str,
) -> String {
    format!(
        "Cannot set provider [{certificate_provider_name}] as default for operation {operation} \
         because another provider was set to be default for the operation"
    )
}

/// How this service refuses an `accountDefaultForOperations` entry that is no
/// `CertificateProviderOperation`.
pub fn unsupported_provider_operation_message(operation: &str) -> String {
    format!(
        "accountDefaultForOperations {operation} is not a supported certificate provider \
         operation; the supported operations are {}",
        CERTIFICATE_PROVIDER_OPERATIONS.join(", ")
    )
}

/// One certificate provider in the store, as this service persists it.
#[derive(Debug, Clone)]
pub struct RegisteredCertificateProvider {
    /// The name the provider was created under; also its key in the store.
    pub certificate_provider_name: String,
    /// `arn:aws:iot:<region>:<account>:certificateprovider/<name>`, minted on create.
    pub certificate_provider_arn: String,
    /// `CreateCertificateProvider$lambdaFunctionArn` — "The ARN of the Lambda function
    /// that defines the authentication logic".
    pub lambda_function_arn: String,
    /// The operations this provider is the account's default for, as the request
    /// listed them.
    pub account_default_for_operations: Vec<String>,
    /// `Tag$Key` -> `Tag$Value` for the tags the create request carried.
    pub tags: BTreeMap<String, Option<String>>,
    /// When the provider was created — `DescribeCertificateProvider$creationDate`.
    pub creation_date: SystemTime,
    /// When the provider last changed —
    /// `DescribeCertificateProvider$lastModifiedDate`. Equal to
    /// [`RegisteredCertificateProvider::creation_date`] until a later write moves it.
    pub last_modified_date: SystemTime,
}

impl RegisteredCertificateProvider {
    /// A newly created certificate provider: the minted identity plus the requested
    /// function, defaults and tags.
    pub fn new(
        certificate_provider_name: String,
        certificate_provider_arn: String,
        lambda_function_arn: String,
        account_default_for_operations: Vec<String>,
        tags: BTreeMap<String, Option<String>>,
        created_at: SystemTime,
    ) -> Self {
        Self {
            certificate_provider_name,
            certificate_provider_arn,
            lambda_function_arn,
            account_default_for_operations,
            tags,
            creation_date: created_at,
            last_modified_date: created_at,
        }
    }

    /// The record as `DescribeCertificateProvider` reports it — every member of the
    /// stored record, so the read and the create cannot drift.
    pub fn to_description(&self) -> DescribeCertificateProviderOutput {
        DescribeCertificateProviderOutput {
            certificate_provider_name: Some(self.certificate_provider_name.clone()),
            certificate_provider_arn: Some(self.certificate_provider_arn.clone()),
            lambda_function_arn: Some(self.lambda_function_arn.clone()),
            account_default_for_operations: Some(self.account_default_for_operations.clone()),
            creation_date: Some(::aws_smithy_types::DateTime::from(self.creation_date)),
            last_modified_date: Some(::aws_smithy_types::DateTime::from(self.last_modified_date)),
        }
    }
}

impl super::ComAmazonawsIotAwsiotserviceState {
    /// The name of the provider that is the account's default for `operation`, or
    /// `None` while no provider claims it.
    ///
    /// Derived from the records themselves, so the account holds at most one default
    /// per operation by construction and the slot is free again as soon as the
    /// provider holding it is deleted.
    pub fn default_certificate_provider(&self, operation: &str) -> Option<&str> {
        self.certificate_providers
            .values()
            .find(|provider| {
                provider
                    .account_default_for_operations
                    .iter()
                    .any(|claimed| claimed == operation)
            })
            .map(|provider| provider.certificate_provider_name.as_str())
    }
}
