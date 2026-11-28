// SPDX-License-Identifier: CC0-1.0

//! Amazon SageMaker AI's published default-quota catalog.

use super::*;

/// Amazon SageMaker AI's published default-quota catalog.
///
/// SOURCE: AWS's published Service Quotas data for the `sagemaker` service code (the
/// "Amazon SageMaker AI endpoints and quotas" page), for each row's name, value and
/// adjustability, and — for the HyperPod cluster plane below — the `QuotaCode` the quota is
/// published under.
///
/// The per-instance-type rows are the planes a notebook instance and a hosted endpoint are
/// counted on; the HyperPod plane is counted on the account's CLUSTERS, across every one of
/// them, and is the bound SageMaker's own `BatchAddClusterNodes` refuses a scale-out past.
/// A row carries no `Description` here because no source available to this catalog states
/// the sentence AWS publishes for it, and inventing one would put words in the service's
/// mouth (`super` states that rule).
pub(in crate::state) fn sagemaker_quotas() -> Vec<QuotaRow> {
    vec![
        quota("ml.m5.xlarge for notebook instance usage", 2.0),
        quota("ml.m5.xlarge for endpoint usage", 2.0),
        quota("Maximum number of training jobs", 100.0),
        // The instances the account may run across ALL of its SageMaker HyperPod clusters,
        // published under AWS's own code: the plane SageMaker measures a cluster scale-out
        // against, and the one a client reads to learn how much capacity it may ask for.
        quota(
            "Total number of instances allowed across SageMaker Clusters",
            48.0,
        )
        .code("L-3308CCC7"),
    ]
}
