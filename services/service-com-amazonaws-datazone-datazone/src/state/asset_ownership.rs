// SPDX-License-Identifier: CC0-1.0

//! Asset ownership transfers: the record an asset keeps of the move of its ownership to
//! another project, and the move itself once the clock owes it.
//!
//! A transfer is ASYNCHRONOUS — "The transfer runs asynchronously, and the operation
//! returns HTTP status code 202. To follow the transfer, call GetAsset and check
//! ownershipTransfer.status until it is SUCCEEDED or FAILED" (smithy_query query="doc"
//! shape="UpdateAssetOwner") — so the call that starts one records it
//! [`OWNERSHIP_TRANSFER_STATUS_IN_PROGRESS`] with the instant it is due, and
//! [`settle_asset_ownership`] is what moves the ownership when that instant has passed.
//! No handler ever sleeps for it.
//!
//! What MOVES is the ownership itself: the asset's owning project, and with it the
//! catalog record of that asset — the listing, which every subscription to the asset
//! resolves its owning project through, so the listing and the subscriptions on it follow
//! the asset by being read back out of this one store rather than by being copied.

use super::{date_time, Asset, AssetRevision, Domain, ENTITY_TYPE_ASSET, PROJECT_STATUS_ACTIVE};

/// The `AssetOwnershipTransferStatus` members (model enum
/// `com.amazonaws.datazone#AssetOwnershipTransferStatus`: FAILED | IN_PROGRESS |
/// SUCCEEDED).
///
/// `IN_PROGRESS` is the only one a transfer is recorded with, and the only one that owes
/// the clock anything: the other two are terminal.
pub const OWNERSHIP_TRANSFER_STATUS_IN_PROGRESS: &str = "IN_PROGRESS";
pub const OWNERSHIP_TRANSFER_STATUS_SUCCEEDED: &str = "SUCCEEDED";
pub const OWNERSHIP_TRANSFER_STATUS_FAILED: &str = "FAILED";

/// The span this emulator takes to finish an ownership transfer: how long after the call
/// that starts one the move is due.
///
/// AWS does not document how long a transfer takes, so this is a CHOSEN span. It is
/// deliberately non-zero: "The transfer runs asynchronously … call GetAsset and check
/// ownershipTransfer.status until it is SUCCEEDED or FAILED" (smithy_query query="doc"
/// shape="UpdateAssetOwner"), so a caller that reads the asset immediately after the call
/// must still observe the in-progress transfer and the refusals it carries.
pub const OWNERSHIP_TRANSFER_SETTLE_MS: i64 = 3_000;

/// One recorded ownership transfer of an asset: the move `UpdateAssetOwner` started, as
/// it stands now.
///
/// An asset keeps its MOST RECENT transfer ([`Asset::ownership_transfer`]), which is what
/// `GetAsset` reports, and at most one of them is ever in progress
/// ([`Asset::transfer_in_progress`]).
#[derive(Debug, Clone)]
pub struct OwnershipTransfer {
    /// One of [`OWNERSHIP_TRANSFER_STATUS_IN_PROGRESS`] /
    /// [`OWNERSHIP_TRANSFER_STATUS_SUCCEEDED`] / [`OWNERSHIP_TRANSFER_STATUS_FAILED`].
    pub status: String,
    /// "The ID of the project that owned the asset when the transfer started"
    /// (smithy_query shape="AssetOwnershipTransfer" member="sourceProjectId") — the
    /// project the asset goes back to when the transfer fails.
    pub source_project_id: String,
    pub target_project_id: String,
    pub started_at_ms: i64,
    /// When the status this transfer reports was last changed: the instant it started,
    /// until it reaches a terminal status.
    pub updated_at_ms: i64,
    /// When an [`OWNERSHIP_TRANSFER_STATUS_IN_PROGRESS`] transfer is due to land.
    /// `None` for a transfer that has already reached a terminal status.
    pub status_due_ms: Option<i64>,
    /// The DataZone user whose call started the transfer, so the records the landing
    /// transfer moves name who moved them.
    pub started_by: String,
    /// The `clientToken` of the `UpdateAssetOwner` that started this transfer, so a
    /// replay of that request while the transfer is in progress answers with it instead
    /// of starting a second one.
    pub client_token: Option<String>,
    /// The asset revision this transfer recorded when it started: the `revision` the
    /// call that started it answered with, so a replay answers with the same one.
    pub revision: u32,
}

impl OwnershipTransfer {
    /// Whether this transfer is still running, i.e. whether the asset is in the state
    /// that admits no second transfer and no change to the asset or its listing.
    #[must_use]
    pub fn in_progress(&self) -> bool {
        self.status == OWNERSHIP_TRANSFER_STATUS_IN_PROGRESS
    }

    /// This transfer as the modelled `AssetOwnershipTransfer`, shared by every operation
    /// that reports one so a read and the call that started it cannot describe one move
    /// differently.
    #[must_use]
    pub fn view(&self) -> crate::ops::types::AssetOwnershipTransfer {
        crate::ops::types::AssetOwnershipTransfer {
            status: self.status.clone(),
            source_project_id: self.source_project_id.clone(),
            target_project_id: self.target_project_id.clone(),
            started_at: Some(date_time(self.started_at_ms)),
            updated_at: Some(date_time(self.updated_at_ms)),
        }
    }
}

/// The transfer `UpdateAssetOwner` records when it starts a move of `asset`'s ownership
/// to `target_project_id`, due to land at `due_ms`.
///
/// The one place a transfer is created, so the record a later path leaves behind cannot
/// be shaped differently.
#[must_use]
pub fn started_transfer(
    asset: &Asset,
    target_project_id: String,
    revision: u32,
    started_by: String,
    client_token: Option<String>,
    now: i64,
    due_ms: i64,
) -> OwnershipTransfer {
    OwnershipTransfer {
        status: OWNERSHIP_TRANSFER_STATUS_IN_PROGRESS.to_string(),
        source_project_id: asset.owning_project_id.clone(),
        target_project_id,
        started_at_ms: now,
        updated_at_ms: now,
        status_due_ms: Some(due_ms),
        started_by,
        client_token,
        revision,
    }
}

impl Asset {
    /// Whether a transfer of this asset's ownership is running: what makes a second
    /// transfer, and every change to the asset or to its listing, conflict with the
    /// asset's current state.
    #[must_use]
    pub fn transfer_in_progress(&self) -> bool {
        self.ownership_transfer
            .as_ref()
            .is_some_and(OwnershipTransfer::in_progress)
    }

    /// The transfer `token` started, if an `UpdateAssetOwner` carrying that `clientToken`
    /// started the one this asset is running.
    ///
    /// Only an IN-PROGRESS transfer is replayed: "If this token matches a previous
    /// request while its transfer is in progress, the service returns that transfer and
    /// doesn't start a new one" (smithy_query shape="UpdateAssetOwnerInput"
    /// member="clientToken"), so the same token presented after the move has landed
    /// starts a new transfer.
    #[must_use]
    pub fn transfer_with_token(&self, token: &str) -> Option<&OwnershipTransfer> {
        self.ownership_transfer
            .as_ref()
            .filter(|transfer| transfer.in_progress())
            .filter(|transfer| transfer.client_token.as_deref() == Some(token))
    }
}

/// The `ConflictException` message a request refused by a running transfer carries:
/// `change` is what the request would have done, so each refusal names the change it was.
///
/// Fixed text around two variable parts read from the request and the store — the asset
/// addressed and the change asked for. The fixed text is this service's own convention,
/// the one its other `ConflictException`s follow, rather than text recorded from the
/// service: no recording exercises this refusal, so a later one can correct the wording.
#[must_use]
pub fn transfer_in_progress_message(asset_id: &str, change: &str) -> String {
    format!(
        "Asset {asset_id} has an ownership transfer in progress, so it cannot be {change}"
    )
}

/// Finish every ownership transfer of `domain` the clock already owes at `now`.
///
/// A transfer that lands moves the ownership: the asset's owning project becomes the
/// target, and the catalog record of that asset — its listing, which is owned by whoever
/// owns the published entity — moves with it, so a subscription to the asset reports the
/// project that now owns it. A target project the domain no longer holds, or one that is
/// no longer active, cannot be given the asset, so that transfer FAILS and the source
/// project keeps it ("FAILED — The transfer didn't complete, and the service rolled back
/// its changes. The source project owns the asset.", smithy_query
/// shape="AssetOwnershipTransferStatus").
///
/// Writes exactly what was already owed at this instant whether or not anybody called,
/// which is what lets a reporting operation reach it.
pub fn settle_asset_ownership(domain: &mut Domain, now: i64) {
    // The transfers due, decided against the projects the domain holds NOW, before
    // anything is written: the walk reads the asset and the target project together.
    let landing: Vec<(String, i64, bool)> = domain
        .assets
        .values()
        .filter_map(|asset| {
            let transfer = asset.ownership_transfer.as_ref()?;
            let due_at = transfer.status_due_ms.filter(|due| *due <= now)?;
            let target_takes_it = domain
                .projects
                .get(&transfer.target_project_id)
                .is_some_and(|project| project.project_status == PROJECT_STATUS_ACTIVE);
            Some((asset.id.clone(), due_at, target_takes_it))
        })
        .collect();

    for (asset_id, due_at, target_takes_it) in landing {
        let Some(asset) = domain.assets.get_mut(&asset_id) else {
            continue;
        };
        let Some(transfer) = asset.ownership_transfer.as_mut() else {
            continue;
        };
        transfer.status_due_ms = None;
        transfer.updated_at_ms = due_at;
        if !target_takes_it {
            transfer.status = OWNERSHIP_TRANSFER_STATUS_FAILED.to_string();
            continue;
        }
        transfer.status = OWNERSHIP_TRANSFER_STATUS_SUCCEEDED.to_string();
        let target = transfer.target_project_id.clone();
        let moved_by = transfer.started_by.clone();
        asset.owning_project_id = target.clone();
        // The asset's catalog record follows its owner. Which project owns a listing is
        // a fact about the listing now, not a property of one snapshot, so every
        // recorded revision of it names the new owner: a subscription pins the revision
        // it was made against ([`super::SubscribedListingRef`]) and so would otherwise
        // still report the project that no longer owns the asset.
        if let Some(listing) = domain.listing_for_entity_mut(ENTITY_TYPE_ASSET, &asset_id) {
            for revision in listing.revisions.values_mut() {
                revision.owning_project_id = target.clone();
            }
            listing.updated_by = moved_by;
            listing.updated_at_ms = due_at;
        }
    }
}

/// The revision an ownership transfer records on `asset` when it starts: the asset as it
/// stands, recorded again under the next revision number.
///
/// "The revision of the asset that was created when the transfer started" (smithy_query
/// shape="UpdateAssetOwnerOutput" member="revision"): the transfer changes which project
/// owns the asset rather than any of the metadata a revision carries, so the new revision
/// is the latest one over again, attributed to the caller that started the transfer.
/// `None` for an asset with no revision at all, which this store never holds.
#[must_use]
pub fn transfer_revision(asset: &Asset, created_by: String, now: i64) -> Option<AssetRevision> {
    let latest = asset.latest_revision()?;
    Some(AssetRevision {
        revision: asset.next_revision_number(),
        created_by,
        created_at_ms: now,
        // The token belongs to the TRANSFER, which remembers it, so a replay is answered
        // from there rather than from a revision this call recorded.
        client_token: None,
        ..latest.clone()
    })
}

/// Record `revision` on `asset` and start `transfer` on it, together: the asset's record
/// of the move and the revision that move created cannot then disagree.
pub fn record_transfer(asset: &mut Asset, revision: AssetRevision, transfer: OwnershipTransfer) {
    asset.revisions.insert(revision.revision, revision);
    asset.ownership_transfer = Some(transfer);
}
