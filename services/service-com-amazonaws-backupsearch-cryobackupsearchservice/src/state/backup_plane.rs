// SPDX-License-Identifier: CC0-1.0

//! The recovery points AWS Backup holds for this account, as this service's scope
//! resolution sees them.
//!
//! A search job searches BACKUPS, and the component that owns a backup is AWS Backup:
//! `StartBackupJob` takes one, stores it in a vault as a recovery point and — when the
//! call asked for one — builds the backup index that makes its contents searchable. So
//! the inventory a `SearchScope` is resolved against is not a list this service keeps of
//! its own: it is READ from the Backup store in the same emulator, the way an
//! `EncryptionKeyArn` is resolved in KMS's store
//! ([`crate::ops::start_search_job`]) — a recovery point a client has just created
//! through Backup's own API is therefore in scope on the very next call, and one Backup
//! no longer holds is in scope for nothing.
//!
//! Only a recovery point carrying a backup index takes part in a search at all: the index
//! is what a scan reads, so a backup taken without one is not part of the searchable
//! inventory and is not reported as covered either. The index's own status travels with
//! the projected record, which is what decides whether the scan can read that backup
//! ([`index_searchable`]) or only display it.

use super::*;

use ::service_com_amazonaws_backup_cryocontrollerusermanager::state as backup;

/// The searchable recovery points AWS Backup holds for this account at `now`: every
/// stored recovery point whose backup has finished and which carries a backup index,
/// projected into the record a `SearchScope` is resolved against and
/// `ListSearchJobBackups` reports.
///
/// The Backup store moves a recovery point and its index by the clock exactly as this
/// service moves a search job, so the transitions the clock has ALREADY made due are
/// materialised first ([`backup::ComAmazonawsBackupCryocontrollerusermanagerState::settle_recovery_points`])
/// — the same step Backup's own reads take, so the two components cannot disagree about
/// which backups exist and which of their indexes are built. Nothing else is written
/// there: this call creates no backup and takes none away.
///
/// A recovery point whose backup is still being taken is left out for the reason Backup
/// leaves it out of every call that addresses one: until the backup finishes there is no
/// stored recovery point to search.
///
/// The ITEMS a projected backup's index holds are left empty: the Backup component in
/// this emulator stores no object or file inventory for a backup it took, so there is
/// nothing to report at the item layer rather than something to invent. A test that
/// drives the item layer holds its records in
/// [`ComAmazonawsBackupsearchCryobackupsearchserviceState::recovery_points`], which
/// supersedes this projection for the same backup.
///
/// Answers EMPTY when no Backup service is mounted beside this one: there is then no
/// component holding a backup for this account, so no scope can resolve to one.
pub fn indexed_recovery_points<Eng: Send + Sync + 'static>(
    engine: &Eng,
    now: SystemTime,
) -> Vec<IndexedRecoveryPoint> {
    let Some(state) = backup::service_state(engine) else {
        // Visible to an operator: a search whose scope resolves to nothing because the
        // component holding this account's backups is not mounted is otherwise
        // indistinguishable from one whose scope really selected no backup.
        ::tracing::warn!(
            "no Backup service is mounted beside BackupSearch; a SearchScope resolves \
             against no recovery point"
        );
        return Vec::new();
    };
    let mut store = state.lock();
    store.settle_recovery_points(now);
    store
        .recovery_points_by_vault
        .iter()
        .flat_map(|((_region, vault_name), arns)| {
            arns.iter().filter_map(|arn| {
                let point = store.recovery_points.get(arn)?;
                searchable_backup(point).map(|backup| IndexedRecoveryPoint {
                    backup,
                    backup_vault_name: Some(vault_name.clone()),
                    items: Vec::new(),
                })
            })
        })
        .collect()
}

/// One stored recovery point as a `SearchJobBackupsResult`, or `None` when it is not part
/// of the searchable inventory at all.
///
/// Every member comes off the stored record: the recovery point's own ARN is the backup
/// resource, the protected resource it was taken of is the source resource, and the
/// Backup resource type it reports is the type a scope's `BackupResourceTypes` selects on
/// (Backup spells `S3` and `EBS` exactly as this service's `ResourceType` enum does, so a
/// backup of any other type is simply selected by no scope).
///
/// `Status` is the state of the BACKUP INDEX, not of the backup: that is the state the
/// member reports and what decides whether a scan reads this backup or only displays it
/// (`smithy_query(query="doc", shape="ListSearchJobBackups")`). `IndexCreationTime` is
/// the instant that index was built, which is the instant the clock made it readable, so
/// a record reporting an index that is not built yet reports no creation time for it
/// either.
fn searchable_backup(point: &backup::RecoveryPoint) -> Option<SearchJobBackupsResult> {
    if point.status == backup::RECOVERY_POINT_STATUS_CREATING {
        return None;
    }
    let index_status = point.index_status.clone()?;
    let index_built = index_status == backup::INDEX_STATUS_ACTIVE;
    Some(SearchJobBackupsResult {
        status: Some(index_status),
        // The Backup store holds no message for an index status, and its own reads report
        // none either, so none is reported here rather than one being written.
        status_message: None,
        resource_type: point.resource_type.clone(),
        backup_resource_arn: Some(point.arn.clone()),
        source_resource_arn: Some(point.resource_arn.clone()),
        index_creation_time: index_built.then(|| timestamp(point.settles_at)),
        backup_creation_time: Some(timestamp(point.creation_date)),
    })
}
