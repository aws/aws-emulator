// SPDX-License-Identifier: CC0-1.0

//! The account's ENTITLEMENT for instances across its SageMaker HyperPod clusters.
//!
//! How many instances an account may run across ALL of its clusters is not this service's
//! number: it is an adjustable account-level Service Quotas quota, which a client reads with
//! `servicequotas:GetServiceQuota` and has raised with
//! `servicequotas:RequestServiceQuotaIncrease`. So the value a node addition is weighed
//! against is read out of the store that service serves those operations from, never decided
//! here — an account that has had the quota raised may run up to the raised value, and the
//! refusal a client reads states the same number its own quota read reports.
//!
//! An engine with no Service Quotas store mounted publishes no quota for the account to be
//! read at, so the entitlement is the PUBLISHED DEFAULT
//! ([`CLUSTER_INSTANCES_QUOTA_DEFAULT`]) there — the value that catalog publishes for this
//! quota as well.

use std::time::SystemTime;

/// The Service Quotas `ServiceCode` the account's SageMaker quotas are published under.
const QUOTA_SERVICE_CODE: &str = "sagemaker";

/// The `QuotaCode` AWS publishes the instances-across-clusters quota under.
const QUOTA_CODE: &str = "L-3308CCC7";

/// The published name of that quota, as a refusal states it.
///
/// Service Quotas publishes it for `sagemaker` under this name; the per-instance-type entry
/// beside it ([`super::cluster_usage_quota_name`]) bounds ONE instance type, where this one
/// bounds the account's total however the instances are typed.
pub const CLUSTER_INSTANCES_QUOTA_NAME: &str =
    "Total number of instances allowed across SageMaker Clusters";

/// The value the quota is in force at where the account's quotas cannot be read: the
/// DEFAULT published for it, which the account's own quota catalog publishes too.
///
/// It is below the 50 nodes ONE request may ask for
/// (`AddClusterNodeSpecification$IncrementTargetCountBy` is modelled `@range 1..50`, and its
/// own doc caps a whole request at 50 nodes across every instance group), so an account that
/// has asked for no increase cannot absorb a maximal scale-out — the request is weighed
/// against a bound it can actually exceed — while every cluster an emulator serves fits far
/// inside it.
pub const CLUSTER_INSTANCES_QUOTA_DEFAULT: i64 = 48;

/// How many instances this account may run across its HyperPod clusters: the value its
/// instances-across-clusters quota is in force at.
///
/// Read from the Service Quotas store — the one `GetServiceQuota` answers from, where an
/// approved increase is recorded — in the Region the quota is read at. The decision the
/// clock has already made on an open increase request is materialised by that service's own
/// settle first, so the value read here is the one that was owed at this instant even if
/// nothing has called Service Quotas since the request came due.
///
/// A value that is not a usable instance count — not finite, or below zero — bounds nothing,
/// so it reads as the published default.
///
/// Call it with NO state guard of this service held: it takes the other service's lock.
pub fn cluster_instance_entitlement<Eng: Send + Sync + 'static>(
    engine: &Eng,
    region: &str,
    now: SystemTime,
) -> i64 {
    let Some(quotas) =
        ::service_com_amazonaws_servicequotas_servicequotasv20190624::state::service_state(engine)
    else {
        return CLUSTER_INSTANCES_QUOTA_DEFAULT;
    };
    let mut quotas = quotas.lock();
    quotas.settle_due_requests(now);
    quotas
        .applied_value(region, QUOTA_SERVICE_CODE, QUOTA_CODE, None)
        .filter(|value| value.is_finite() && *value >= 0.0)
        .map_or(CLUSTER_INSTANCES_QUOTA_DEFAULT, |value| value as i64)
}
