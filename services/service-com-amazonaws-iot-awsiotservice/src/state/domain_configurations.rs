// SPDX-License-Identifier: CC0-1.0

//! The account's DOMAIN CONFIGURATIONS: the record one holds, the server certificate
//! it presents, and what every reader of one reports.
//!
//! A domain configuration is the endpoint settings a device connects under — which
//! domain name it dials, which certificate the service presents in the TLS
//! handshake, which custom authorizer runs — so a record is those settings plus the
//! status the configuration is in. The name is the configuration's identity: the
//! model binds it as the `@httpLabel` of
//! `/domainConfigurations/{domainConfigurationName}` on every single-configuration
//! operation (`smithy_query(query="operation",operation="CreateDomainConfiguration")`),
//! which is why the store is a `BTreeMap` keyed by it.
//!
//! One record carries everything a reader hands back, so
//! `DescribeDomainConfiguration` after `CreateDomainConfiguration` reports the
//! created configuration member for member.
//!
//! The SERVER CERTIFICATE a configuration presents belongs to Certificate Manager,
//! not to this service: "The ARNs of the certificates that IoT passes to the device
//! during the TLS handshake" (`CreateDomainConfiguration$serverCertificateArns`). A
//! configuration may only name a certificate that EXISTS, and whether one exists is
//! the holder's answer — which this service reaches through the one shared record of a
//! certificate's serving material that Certificate Manager writes
//! ([`certificate_key_material`]). A certificate that record holds nothing for is one
//! this service cannot present, so the create is refused and nothing is written.

use std::collections::BTreeMap;
use std::time::SystemTime;

use crate::ops::types::{
    AuthorizerConfig, ClientCertificateConfig, DescribeDomainConfigurationOutput,
    ServerCertificateConfig, ServerCertificateSummary, TlsConfig,
};

/// How many domain configurations this emulator keeps for the account.
///
/// AWS publishes a service quota on an account's domain configurations; the exact
/// figure is not in the model, so this is the emulator's own ceiling on a store a
/// caller can grow — the same reason [`super::MAX_POLICIES`] exists — and
/// `CreateDomainConfiguration` past it refuses with the modelled
/// `LimitExceededException` rather than growing without bound.
pub const MAX_DOMAIN_CONFIGURATIONS: usize = 100;

/// How many tags one domain configuration may carry: the same documented IoT tagging
/// limit [`super::MAX_POLICY_TAGS`] records, for the same reason.
pub const MAX_DOMAIN_CONFIGURATION_TAGS: usize = super::MAX_POLICY_TAGS;

/// The `serviceType` a configuration created without one carries.
///
/// `CreateDomainConfiguration$serviceType` is "The type of service delivered by the
/// endpoint", with the note "Amazon Web Services IoT Core currently supports only the
/// DATA service type", so a request that names none is a data endpoint. It is one of
/// [`crate::ops::types::SERVICE_TYPE_VALUES`].
pub const DEFAULT_SERVICE_TYPE: &str = "DATA";

/// The `domainConfigurationStatus` a freshly created configuration carries.
///
/// A configuration serves connections from the moment it is created —
/// `UpdateDomainConfiguration` is what DISABLES one — so a create leaves it enabled.
/// It is one of [`crate::ops::types::DOMAIN_CONFIGURATION_STATUS_VALUES`].
pub const DOMAIN_CONFIGURATION_ENABLED: &str = "ENABLED";

/// The `domainType` of a configuration that named its own `domainName`.
///
/// `DomainType` is `ENDPOINT | AWS_MANAGED | CUSTOMER_MANAGED`
/// (`smithy_query(query="shape",shape="DomainType")`), and a configuration carrying a
/// domain name of the customer's own with the certificate to present for it is the
/// customer-managed kind.
pub const DOMAIN_TYPE_CUSTOMER_MANAGED: &str = "CUSTOMER_MANAGED";

/// The `domainType` of a configuration that named no `domainName`.
///
/// `CreateDomainConfiguration$serverCertificateArns` is "not required for Amazon Web
/// Services-managed domains", so a configuration with neither a domain name nor a
/// certificate configures the AWS-managed domain the account's endpoint already
/// serves.
pub const DOMAIN_TYPE_AWS_MANAGED: &str = "AWS_MANAGED";

/// The `serverCertificateStatus` of a certificate a configuration presents.
///
/// A configuration may only name a certificate that exists, so the certificate it
/// holds is one the service could resolve — `VALID`, one of
/// [`crate::ops::types::SERVER_CERTIFICATE_STATUS_VALUES`].
pub const SERVER_CERTIFICATE_VALID: &str = "VALID";

/// The ARN of a domain configuration:
/// `arn:aws:iot:<region>:<account>:domainconfiguration/<domainConfigurationName>`.
///
/// `domainconfiguration/` is the resource type AWS IoT's domain-configuration ARNs
/// use — it is what `CreateDomainConfiguration$domainConfigurationArn` reports; the
/// namespace and account segments are the ones [`super::thing_arn`] documents.
pub fn domain_configuration_arn(region: &str, domain_configuration_name: &str) -> String {
    ::emulator_server::emulator_arn(
        "iot",
        region,
        &format!("domainconfiguration/{domain_configuration_name}"),
    )
}

/// The message every operation that addresses ONE domain configuration refuses a name
/// the account holds none under with.
///
/// The fixed text is the family's, with the name the request asked about bracketed the
/// way the neighbouring configuration families state a name they report back (see
/// [`super::missing_certificate_provider_message`]).
pub fn missing_domain_configuration_message(domain_configuration_name: &str) -> String {
    format!("Domain configuration with name [{domain_configuration_name}] not found")
}

/// How this service refuses a `serverCertificateArn` that names no certificate.
///
/// The fixed text is the class's — a certificate the holder does not have is not a
/// certificate this service can pass to a device — and the variable part is the ARN
/// the request named.
pub fn missing_server_certificate_message(certificate_arn: &str) -> String {
    format!("Certificate {certificate_arn} does not exist.")
}

/// The SHARED record of each Certificate Manager certificate's serving material, in
/// the shared engine: call it with a handler's `&*self.engine`.
///
/// A certificate is HELD by Certificate Manager and PRESENTED here, so the material is
/// one directory under one shared type
/// (`service_common::acm_certificate_key_material::CertificateKeyMaterialDirectory`)
/// and not a copy inside [`super::ComAmazonawsIotAwsiotserviceState`]: the engine keys
/// state by TYPE, so a copy of its own here would be invisible to the holder.
///
/// LOCK ORDER: take [`super::service_state`] first and this one second, which is the
/// order every other reader of a shared directory takes.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn certificate_key_material<Eng: Send + Sync + 'static>(
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

/// One domain configuration in the store, as this service persists it.
///
/// The optional configuration blocks are kept as the MODELLED structures the request
/// carried, so a reader hands back what the writer sent rather than a reassembled
/// copy that could drift from it.
#[derive(Debug, Clone)]
pub struct RegisteredDomainConfiguration {
    /// The name the configuration was created under; also its key in the store.
    pub domain_configuration_name: String,
    /// `arn:aws:iot:<region>:<account>:domainconfiguration/<name>`, minted on create.
    pub domain_configuration_arn: String,
    /// `CreateDomainConfiguration$domainName`, when the request carried one.
    pub domain_name: Option<String>,
    /// The ARNs of the certificates the service presents for this domain, each one
    /// resolved before the record was written.
    pub server_certificate_arns: Vec<String>,
    /// `CreateDomainConfiguration$validationCertificateArn`, when the request carried
    /// one.
    pub validation_certificate_arn: Option<String>,
    /// `CreateDomainConfiguration$authorizerConfig`, when the request carried one.
    pub authorizer_config: Option<AuthorizerConfig>,
    /// Which of [`crate::ops::types::SERVICE_TYPE_VALUES`] the endpoint delivers.
    pub service_type: String,
    /// Which of [`crate::ops::types::DOMAIN_TYPE_VALUES`] the configuration is of.
    pub domain_type: String,
    /// Which of [`crate::ops::types::DOMAIN_CONFIGURATION_STATUS_VALUES`] it is in.
    pub status: String,
    /// `CreateDomainConfiguration$tlsConfig`, when the request carried one.
    pub tls_config: Option<TlsConfig>,
    /// `CreateDomainConfiguration$serverCertificateConfig`, when carried.
    pub server_certificate_config: Option<ServerCertificateConfig>,
    /// `CreateDomainConfiguration$authenticationType`, when carried.
    pub authentication_type: Option<String>,
    /// `CreateDomainConfiguration$applicationProtocol`, when carried.
    pub application_protocol: Option<String>,
    /// `CreateDomainConfiguration$clientCertificateConfig`, when carried.
    pub client_certificate_config: Option<ClientCertificateConfig>,
    /// `Tag$Key` -> `Tag$Value` for the tags the create request carried.
    pub tags: BTreeMap<String, Option<String>>,
    /// When the configuration last changed status —
    /// `DescribeDomainConfiguration$lastStatusChangeDate`. The create is the first
    /// such change.
    pub last_status_change_date: SystemTime,
}

impl RegisteredDomainConfiguration {
    /// The record as `DescribeDomainConfiguration` reports it — every member of the
    /// stored record, so the read and the create cannot drift.
    pub fn to_description(&self) -> DescribeDomainConfigurationOutput {
        DescribeDomainConfigurationOutput {
            domain_configuration_name: Some(self.domain_configuration_name.clone()),
            domain_configuration_arn: Some(self.domain_configuration_arn.clone()),
            domain_name: self.domain_name.clone(),
            server_certificates: Some(
                self.server_certificate_arns
                    .iter()
                    .map(|certificate_arn| ServerCertificateSummary {
                        server_certificate_arn: Some(certificate_arn.clone()),
                        server_certificate_status: Some(SERVER_CERTIFICATE_VALID.to_string()),
                        server_certificate_status_detail: None,
                    })
                    .collect(),
            ),
            authorizer_config: self.authorizer_config.clone(),
            domain_configuration_status: Some(self.status.clone()),
            service_type: Some(self.service_type.clone()),
            domain_type: Some(self.domain_type.clone()),
            last_status_change_date: Some(::aws_smithy_types::DateTime::from(
                self.last_status_change_date,
            )),
            tls_config: self.tls_config.clone(),
            server_certificate_config: self.server_certificate_config.clone(),
            authentication_type: self.authentication_type.clone(),
            application_protocol: self.application_protocol.clone(),
            client_certificate_config: self.client_certificate_config.clone(),
        }
    }
}
