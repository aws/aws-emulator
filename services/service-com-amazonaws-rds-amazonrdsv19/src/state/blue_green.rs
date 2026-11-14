// SPDX-License-Identifier: CC0-1.0

//! The blue/green deployment records `CreateBlueGreenDeployment`,
//! `DeleteBlueGreenDeployment` and `DescribeBlueGreenDeployments` share.
//!
//! A blue/green deployment is a pairing of two databases this service already holds: the
//! SOURCE (the production database in the blue environment) and the TARGET this service
//! cloned from it into the green environment. Only the pairing lives here — the two
//! databases themselves stay the DB instance / DB cluster records they already are, so a
//! deployment cannot report a source or a target that disagrees with what a describe of
//! that database reports.

use std::time::Instant;

use super::{PendingChange, PendingStatus, ARN_NAMESPACE};
use crate::ops::types::{BlueGreenDeployment, SwitchoverDetail, Tag};

/// How many blue/green deployments one region holds.
///
/// Amazon RDS publishes no quota for this, and every record is caller-created, so the cap
/// is this emulator's — the same reason every other per-region quota in `src/state.rs`
/// exists: it bounds the store and every scan over it.
pub const BLUE_GREEN_DEPLOYMENT_QUOTA_PER_REGION: usize = 20;

/// `BlueGreenDeployment$Status` — "`PROVISIONING` - Resources are being created in the
/// green environment."
pub const BLUE_GREEN_STATUS_PROVISIONING: &str = "PROVISIONING";

/// `BlueGreenDeployment$Status` — "`AVAILABLE` - Resources are available in the green
/// environment."
pub const BLUE_GREEN_STATUS_AVAILABLE: &str = "AVAILABLE";

/// `BlueGreenDeployment$Status` — "`DELETING` - The blue/green deployment is being
/// deleted."
pub const BLUE_GREEN_STATUS_DELETING: &str = "DELETING";

/// The prefix of the identifier Amazon RDS assigns a blue/green deployment
/// (`bgd-` followed by a lowercase hexadecimal token). The identifier is SYSTEM-generated
/// — `CreateBlueGreenDeployment` takes a `BlueGreenDeploymentName` from the caller and
/// hands the identifier back — so a request that names one of another form names nothing
/// this service could ever have minted.
pub const BLUE_GREEN_IDENTIFIER_PREFIX: &str = "bgd-";

/// How many characters [`mint_blue_green_deployment_identifier`] mints after
/// [`BLUE_GREEN_IDENTIFIER_PREFIX`], which is the length of the identifiers AWS mints.
const MINTED_IDENTIFIER_TOKEN_LEN: usize = 16;

/// The longest token this service accepts after [`BLUE_GREEN_IDENTIFIER_PREFIX`]: the
/// identifier's own length bound (255) less the prefix, so nothing inside the modelled
/// bound is refused for its length alone.
const MAX_IDENTIFIER_TOKEN_LEN: usize = 255 - BLUE_GREEN_IDENTIFIER_PREFIX.len();

/// The ARN resource type of a blue/green deployment.
const ARN_RESOURCE_TYPE: &str = "blue-green-deployment";

/// The identifier this service assigns a new blue/green deployment: the published
/// `bgd-<token>` form, with the token minted as every other server-assigned token of this
/// service is ([`super::minted_token`]), so no two deployments can be minted the same id.
pub fn mint_blue_green_deployment_identifier(mint: u64) -> String {
    format!(
        "{BLUE_GREEN_IDENTIFIER_PREFIX}{}",
        super::minted_token(
            "blue-green-deployment",
            mint,
            MINTED_IDENTIFIER_TOKEN_LEN
        )
        .to_ascii_lowercase()
    )
}

/// The blue/green deployment identifier `named` carries: `named` itself, or the resource
/// part of it when it is a blue/green deployment ARN.
pub fn blue_green_deployment_identifier_of(named: &str) -> &str {
    match named.strip_prefix("arn:") {
        None => named,
        Some(rest) => rest.rsplit(':').next().unwrap_or(named),
    }
}

/// Refuse a `BlueGreenDeploymentIdentifier` that is not of the form this service MINTS.
///
/// The identifier is system-generated — `CreateBlueGreenDeployment` takes a NAME from the
/// caller and hands the identifier back — so a value that does not carry the prefix every
/// assigned identifier carries is not an unknown deployment but an unusable parameter
/// value, and the two are different answers: this refusal is the `InvalidParameterValue`
/// the real service answers an arbitrary identifier with, while an identifier of the
/// assigned form naming no stored deployment is the modelled not-found fault. `Err`
/// carries the message the caller is refused with.
pub fn check_blue_green_deployment_identifier(member: &str, value: &str) -> Result<(), String> {
    let identifier = blue_green_deployment_identifier_of(value);
    let token = identifier
        .strip_prefix(BLUE_GREEN_IDENTIFIER_PREFIX)
        .unwrap_or_default();
    if !token.is_empty() && token.len() <= MAX_IDENTIFIER_TOKEN_LEN {
        return Ok(());
    }
    Err(format!(
        "invalid value '{value}' for {member}: a blue/green deployment identifier is \
         assigned by the service and reads {BLUE_GREEN_IDENTIFIER_PREFIX} followed by the \
         token it assigned"
    ))
}

/// The sentence a `BlueGreenDeploymentNotFoundFault` carries for a
/// `BlueGreenDeploymentIdentifier` that names no stored deployment.
///
/// Amazon RDS words this fault as a statement about the deployment, with the identifier the
/// request named QUOTED inside it, and the same sentence whichever call failed to resolve
/// it — so it lives here beside the family's other rules rather than in each handler.
/// `named` is the deployment identifier the request carried: the member also accepts the
/// deployment's ARN, so it is the identifier part of that value
/// ([`blue_green_deployment_identifier_of`]), which is the handle the caller gave the
/// deployment.
pub fn blue_green_deployment_not_found_message(named: &str) -> String {
    format!(
        "Blue Green Deployment '{}' doesn't exist.",
        blue_green_deployment_identifier_of(named)
    )
}

/// The identifier of the database this service creates in the GREEN environment for a
/// deployment whose source is `source_identifier`.
///
/// Amazon RDS names the green database after the blue one with a `-green-<token>` suffix,
/// the token being server-assigned; the token here is minted as every other
/// server-assigned token of this service is ([`super::minted_token`]). `max_len` is the
/// identifier bound the green database's own type carries, and the name is kept inside it
/// by shortening the part taken from the source.
pub fn green_identifier(source_identifier: &str, mint: u64, max_len: usize) -> String {
    let suffix = format!(
        "-green-{}",
        super::minted_token("green-database", mint, 6).to_ascii_lowercase()
    );
    let room = max_len.saturating_sub(suffix.len());
    let mut prefix: String = source_identifier.chars().take(room).collect();
    // An identifier "can't end with a hyphen", which a shortened prefix might.
    while prefix.ends_with('-') {
        prefix.pop();
    }
    format!("{prefix}{suffix}")
}

/// A persisted blue/green deployment.
#[derive(Debug, Clone)]
pub struct BlueGreenDeploymentRecord {
    /// The minted `BlueGreenDeploymentIdentifier`, which is half this record's key.
    pub identifier: String,
    /// `BlueGreenDeploymentName`, as the caller named it.
    pub name: String,
    /// The region whose credential scope the create was signed with.
    pub region: String,
    /// `Source`: the ARN of the blue (production) database.
    pub source_arn: String,
    /// `Target`: the ARN of the green database this service cloned from the source.
    pub target_arn: String,
    /// Whether the two databases are DB CLUSTERS rather than DB instances, which is what
    /// decides where a delete of the green environment looks for them.
    pub target_is_cluster: bool,
    /// `Status`.
    pub status: String,
    /// The status change the clock owes this record, if any.
    pub pending_status: Option<PendingStatus>,
    /// `CreateTime`.
    pub create_time: ::aws_smithy_types::DateTime,
    /// `DeleteTime`, once a delete has been accepted.
    pub delete_time: Option<::aws_smithy_types::DateTime>,
    /// `TagList`.
    pub tags: Vec<Tag>,
}

impl BlueGreenDeploymentRecord {
    /// Apply a change whose due instant has passed. Returns whether the record is now
    /// gone, so the caller drops it from the store — the same contract as
    /// `DbInstanceRecord::settle`.
    pub(super) fn settle(&mut self, now: Instant) -> bool {
        if self.pending_status.as_ref().is_some_and(|p| now >= p.due_at) {
            let pending = self.pending_status.take().expect("checked just above");
            match pending.change {
                PendingChange::Status(status) => self.status = status,
                PendingChange::Removal => return true,
            }
        }
        false
    }

    /// The deployment's ARN, from the stored region and identifier.
    pub fn arn(&self) -> String {
        emulator_server::emulator_arn(
            ARN_NAMESPACE,
            &self.region,
            &format!("{ARN_RESOURCE_TYPE}:{}", self.identifier),
        )
    }

    /// Whether a delete has already been accepted for this deployment, so it is on its
    /// way out and nothing further can be done to it.
    pub fn is_deleting(&self) -> bool {
        self.status == BLUE_GREEN_STATUS_DELETING
    }

    /// Accept a delete: the deployment reports `DELETING` from now on, and the clock owes
    /// its removal from the store, after which every read of the identifier is the
    /// modelled not-found fault. Nothing sleeps — the due instant is stored.
    pub fn begin_deletion(&mut self) {
        self.status = BLUE_GREEN_STATUS_DELETING.to_string();
        self.delete_time = Some(super::now_as_wire_timestamp());
        self.pending_status = Some(PendingStatus::removal_in_a_moment());
    }

    /// Project the stored record onto the modelled `BlueGreenDeployment` shape.
    ///
    /// `SwitchoverDetails` pairs the one source resource with the one target resource this
    /// deployment holds, and its status follows the deployment's own: a resource in a
    /// deployment that is still provisioning is not yet ready to switch over. `Tasks` is
    /// LEFT OUT rather than invented — it reports the tasks the real service runs to build
    /// the green environment, and this service has no state for any of them.
    pub fn to_wire(&self) -> BlueGreenDeployment {
        BlueGreenDeployment {
            blue_green_deployment_identifier: Some(self.identifier.clone()),
            blue_green_deployment_name: Some(self.name.clone()),
            source: Some(self.source_arn.clone()),
            target: Some(self.target_arn.clone()),
            switchover_details: Some(vec![SwitchoverDetail {
                source_member: Some(self.source_arn.clone()),
                target_member: Some(self.target_arn.clone()),
                status: Some(self.status.clone()),
            }]),
            tasks: None,
            status: Some(self.status.clone()),
            status_details: None,
            create_time: Some(self.create_time),
            delete_time: self.delete_time,
            tag_list: Some(self.tags.clone()),
        }
    }
}
