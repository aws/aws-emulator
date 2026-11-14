// SPDX-License-Identifier: CC0-1.0

//! The members of a deployment's record that are SUMMARISED or DEFAULTED rather
//! than echoed back as a client sent them: the status summary of the targets the
//! deployment holds, and the file-handling policy every deployment runs under.
//!
//! Both belong to the deployment RECORD, not to one reader: the summary is
//! computed by the single projection every reader answers from
//! ([`super::deployment_info`], which `GetDeployment` and `BatchGetDeployments`
//! share), and the policy is normalised where a deployment is WRITTEN
//! ([`super::ComAmazonawsCodedeployCodedeploy20141006State::record_deployment`],
//! the one funnel every deployment goes through, which is also where an inline
//! revision is given its digest) — so the value a read reports is the one the
//! deployment is running under, whichever reader asks and however the deployment
//! was created.

use crate::ops::types::DeploymentOverview;

use super::{
    Deployment, TARGET_STATUS_FAILED, TARGET_STATUS_IN_PROGRESS, TARGET_STATUS_PENDING,
    TARGET_STATUS_SKIPPED, TARGET_STATUS_SUCCEEDED,
};

/// The `FileExistsBehavior` a deployment that asked for none runs under: the
/// model documents this value as "also the default behavior if no option is
/// specified"
/// (`smithy_query(query="doc",shape="DeploymentInfo",member="fileExistsBehavior")`),
/// and the enum's other values (`crate::ops::types::FILE_EXISTS_BEHAVIOR_VALUES`:
/// DISALLOW | OVERWRITE | RETAIN) are only ever reported because a request asked
/// for them.
pub const FILE_EXISTS_BEHAVIOR_DISALLOW: &str = "DISALLOW";

/// The file-handling policy a deployment created with `requested` runs under.
///
/// A policy the request named is the policy the deployment runs under; a request
/// that named none — and an internally created deployment, which asks for
/// nothing — runs under the service's documented default. The deployment is
/// RECORDED with it, so a reader reports the policy the deployment has rather
/// than the absence of one in the request that created it.
#[must_use]
pub fn file_exists_behavior(requested: Option<String>) -> String {
    requested
        .map(|behavior| behavior.trim().to_string())
        .filter(|behavior| !behavior.is_empty())
        .unwrap_or_else(|| FILE_EXISTS_BEHAVIOR_DISALLOW.to_string())
}

/// The `DeploymentOverview` of `deployment`: "a summary of the deployment status
/// of the instances in the deployment"
/// (`smithy_query(query="doc",shape="DeploymentInfo",member="deploymentOverview")`).
///
/// Every deployment has such a summary — its six counts each default to 0
/// (`smithy_query(query="shape",shape="DeploymentOverview")`), so a deployment
/// that has deployed to nothing yet is summarised as all zeros rather than as no
/// summary at all — and each count is the number of the deployment's OWN targets
/// ([`Deployment::targets`], the snapshot its group resolved) in that state, so
/// the summary is one view of the same target state the target readers answer
/// from and cannot disagree with them.
///
/// This emulator deploys to every target of a deployment together, so each of
/// them is as far along as the deployment is ([`super::target_status`]) and the
/// targets are counted under the one status they hold. `Ready` counts "the
/// number of instances in a replacement environment ready to receive traffic in
/// a blue/green deployment": a replacement environment is provisioned from EC2
/// inventory this emulator does not keep, so no deployment here resolves such a
/// target (see [`super::deployment_targets`]) and nothing is counted under it.
#[must_use]
pub fn deployment_overview(deployment: &Deployment) -> DeploymentOverview {
    let mut overview = DeploymentOverview::default();
    let counted = i64::try_from(deployment.target_count()).unwrap_or(i64::MAX);
    match super::target_status(deployment) {
        TARGET_STATUS_PENDING => overview.pending = counted,
        TARGET_STATUS_IN_PROGRESS => overview.in_progress = counted,
        TARGET_STATUS_SUCCEEDED => overview.succeeded = counted,
        TARGET_STATUS_FAILED => overview.failed = counted,
        TARGET_STATUS_SKIPPED => overview.skipped = counted,
        // A `TargetStatus` this service maps no deployment onto leaves every
        // count where it is, which is the summary of a deployment whose targets
        // are in none of the states above.
        _ => {}
    }
    overview
}

#[cfg(test)]
mod tests {
    //! What file-handling policy a deployment runs under, over what the request
    //! that created it stated. The summary of a deployment's targets needs a
    //! deployment the service itself resolved targets for, so it is asserted end
    //! to end, through the operations a client calls, in
    //! `tests/all/deployment_info_members.rs` — beside this rule.

    use super::*;

    #[test]
    fn a_deployment_that_asked_for_no_file_policy_runs_under_the_default() {
        // Rows differ along what the request stated: each value the model's enum
        // carries, and the forms that state nothing.
        let mut cases: Vec<(String, Option<String>, &str)> = vec![
            (
                "a request that named none".to_string(),
                None,
                FILE_EXISTS_BEHAVIOR_DISALLOW,
            ),
            (
                "a request whose member is blank".to_string(),
                Some("  ".to_string()),
                FILE_EXISTS_BEHAVIOR_DISALLOW,
            ),
        ];
        // Every value the member accepts, from the model's own catalogue.
        for value in crate::ops::types::FILE_EXISTS_BEHAVIOR_VALUES {
            cases.push((
                format!("a request that named {value}"),
                Some((*value).to_string()),
                value,
            ));
        }

        for (case, requested, expected) in &cases {
            let answered = file_exists_behavior(requested.clone());
            assert_eq!(
                &answered, expected,
                "case {case:?}: expected the policy {expected:?}, got {answered:?}"
            );
        }
    }
}
