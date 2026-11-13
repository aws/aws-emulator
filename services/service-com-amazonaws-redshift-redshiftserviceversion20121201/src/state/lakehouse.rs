// SPDX-License-Identifier: CC0-1.0

//! The lakehouse configuration a cluster carries: whether the cluster is
//! registered with Amazon Redshift federated permissions, the Glue data catalog
//! that registration is against, and the IAM Identity Center application whose
//! trusted identity propagation it is associated with.
//!
//! Two of the three are MODELLED members of the cluster
//! (`Cluster$LakehouseRegistrationStatus` and `$CatalogArn`), so they live on the
//! cluster's own record and a `DescribeClusters` reports what a
//! `ModifyLakehouseConfiguration` wrote; the third has no `Cluster` member and is
//! held beside it ([`StoredCluster::lakehouse_idc_application_arn`]). One rule
//! renders all three into the configuration a response reports
//! ([`ComAmazonawsRedshiftRedshiftserviceversion20121201State::lakehouse_configuration`]),
//! so the write path and the read path cannot report a different configuration.

use super::*;

use crate::ops::types::ModifyLakehouseConfigurationOutput;

/// `LakehouseRegistration` asking for the cluster to be registered with Amazon
/// Redshift federated permissions.
///
/// Source: `smithy_query(query="shape", shape="LakehouseRegistration")`, whose
/// values are `Register` and `Deregister`.
pub const LAKEHOUSE_REGISTER: &str = "Register";

/// `LakehouseRegistration` asking for that registration to be withdrawn. See
/// [`LAKEHOUSE_REGISTER`].
pub const LAKEHOUSE_DEREGISTER: &str = "Deregister";

/// `LakehouseIdcRegistration` asking for the Identity Center application to be
/// associated with the cluster.
///
/// Source: `smithy_query(query="shape", shape="LakehouseIdcRegistration")`, whose
/// values are `Associate` and `Disassociate`.
pub const LAKEHOUSE_IDC_ASSOCIATE: &str = "Associate";

/// `LakehouseIdcRegistration` asking for that association to be withdrawn. See
/// [`LAKEHOUSE_IDC_ASSOCIATE`].
pub const LAKEHOUSE_IDC_DISASSOCIATE: &str = "Disassociate";

/// `Cluster$LakehouseRegistrationStatus` of a cluster that IS registered with
/// Amazon Redshift federated permissions.
///
/// The member is "the current status of the lakehouse registration. Indicates
/// whether the cluster is successfully registered with the lakehouse"
/// (`smithy_query(query="shape", shape="LakehouseConfiguration")`) and the model
/// publishes no value list for it, so the status is the state the registration
/// reached, spelled as the past participle of the `LakehouseRegistration` that
/// asked for it ([`LAKEHOUSE_REGISTER`]).
pub const LAKEHOUSE_REGISTRATION_REGISTERED: &str = "Registered";

/// `Cluster$LakehouseRegistrationStatus` of a cluster whose registration has been
/// withdrawn. See [`LAKEHOUSE_REGISTRATION_REGISTERED`].
pub const LAKEHOUSE_REGISTRATION_DEREGISTERED: &str = "Deregistered";

/// The ARN namespace of the Glue data catalog a lakehouse registration is
/// against.
///
/// Source: `smithy_query(query="doc", shape="ModifyLakehouseConfigurationMessage",
/// member="CatalogName")` — "the name of the Glue data catalog that will be
/// associated with the cluster" — so the catalog is a resource of Glue, named
/// under that service's own namespace, exactly as `CreateCluster` derives the
/// `CatalogArn` of the catalog its own `CatalogName` names.
pub const GLUE_ARN_NAMESPACE: &str = "glue";

/// The ARN of the Glue data catalog named `catalog_name`, in `region`.
///
/// One rule for every path that reports a cluster's `CatalogArn`, so the create
/// path and the lakehouse-configuration path cannot name the same catalog
/// differently.
#[must_use]
pub fn catalog_arn(catalog_name: &str, region: &str) -> String {
    ::engine::emulator_arn(
        GLUE_ARN_NAMESPACE,
        region,
        &format!("catalog/{catalog_name}"),
    )
}

impl ComAmazonawsRedshiftRedshiftserviceversion20121201State {
    /// Whether `application_arn` names an IAM Identity Center application this
    /// account registered with Amazon Redshift.
    ///
    /// `LakehouseIdcApplicationArn` is "the ARN of the IAM Identity Center
    /// application used for enabling Amazon Web Services IAM Identity Center
    /// trusted identity propagation on a cluster" — the IDENTITY CENTER
    /// application, which a Redshift IdC application record carries as its
    /// `IdcManagedApplicationArn`, beside the ARN of the Redshift application
    /// that manages it (`RedshiftIdcApplicationArn`). Either ARN names the same
    /// registered application, so either resolves, and an ARN naming neither
    /// names an application that "doesn't exist".
    #[must_use]
    pub fn holds_identity_center_application(&self, application_arn: &str) -> bool {
        self.redshift_idc_applications.iter().any(|application| {
            application.idc_managed_application_arn.as_deref() == Some(application_arn)
                || application.redshift_idc_application_arn.as_deref() == Some(application_arn)
        })
    }

    /// The lakehouse configuration the cluster at `position` carries, as a
    /// response reports it.
    ///
    /// Every value is read off the cluster's own stored record, so what this
    /// reports and what a `DescribeClusters` reports for the same cluster cannot
    /// disagree.
    #[must_use]
    pub fn lakehouse_configuration(&self, position: usize) -> ModifyLakehouseConfigurationOutput {
        let stored = &self.clusters[position];
        ModifyLakehouseConfigurationOutput {
            cluster_identifier: stored.cluster.cluster_identifier.clone(),
            lakehouse_idc_application_arn: stored.lakehouse_idc_application_arn.clone(),
            lakehouse_registration_status: stored.cluster.lakehouse_registration_status.clone(),
            catalog_arn: stored.cluster.catalog_arn.clone(),
        }
    }
}
