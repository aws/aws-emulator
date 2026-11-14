// SPDX-License-Identifier: CC0-1.0

//! The table restores a `RestoreTableFromClusterSnapshot` asked for, and how a
//! read reports their progress.
//!
//! `RestoreTableFromClusterSnapshot` "creates a new table from a table in an
//! Amazon Redshift cluster snapshot" and "you must create the new table within
//! the Amazon Redshift cluster that the snapshot was taken from"
//! (`smithy_query(query="doc", operation="RestoreTableFromClusterSnapshot")`), and
//! `DescribeTableRestoreStatus` "lists the status of one or more table restore
//! requests made using the RestoreTableFromClusterSnapshot API action"
//! (`smithy_query(query="doc", operation="DescribeTableRestoreStatus")`). So a
//! restore request is a RECORD of this control plane: the call that asks for it
//! appends one, the request keeps the identifier it was minted with, and a read
//! reports the record rather than recomputing it.
//!
//! The table itself is DATA, and a cluster's data lives in the SQL engine the
//! Redshift Data API owns ([`super::cluster_data`]) — so the one thing this
//! module does NOT hold is the table: the restore is performed through that
//! crate's own bounded seam and what it answers is what this record's status
//! reports. A record whose copy did not happen is `FAILED` and says why, never
//! `SUCCEEDED`.

use super::*;

/// `TableRestoreStatus$Status` of a restore whose copy is being made.
///
/// Source: `smithy_query(query="shape", shape="TableRestoreStatusType")` — the
/// enum's values are `PENDING`, `IN_PROGRESS`, `SUCCEEDED`, `FAILED` and
/// `CANCELED`.
pub const TABLE_RESTORE_STATUS_IN_PROGRESS: &str = "IN_PROGRESS";
/// `TableRestoreStatus$Status` of a restore whose table is in the cluster. See
/// [`TABLE_RESTORE_STATUS_IN_PROGRESS`].
pub const TABLE_RESTORE_STATUS_SUCCEEDED: &str = "SUCCEEDED";
/// `TableRestoreStatus$Status` of a restore whose table could not be written.
/// See [`TABLE_RESTORE_STATUS_IN_PROGRESS`].
pub const TABLE_RESTORE_STATUS_FAILED: &str = "FAILED";

/// Why a request past [`IN_PROGRESS_TABLE_RESTORE_QUOTA`] is refused: the fault's
/// own documented sentence, which is what the caller has to act on — wait for the
/// restores in flight — with the bound named.
pub fn in_progress_table_restore_quota_message() -> String {
    format!(
        "You have exceeded the allowed number of table restore requests \
         ({IN_PROGRESS_TABLE_RESTORE_QUOTA}). Wait for your current table restore requests to \
         complete before making a new request."
    )
}

/// Why a restore of a table with more rows than one copy moves is refused,
/// naming the bound.
///
/// An emulator limit rather than a defect in the request, said so in the
/// sentence — the same way every other bound this store sets on what one call may
/// materialise says so. The bound itself is the engine's
/// ([`MAX_RESTORED_TABLE_ROWS`]), which is what the copy stops at, so the number
/// in the sentence is the number that was enforced.
pub fn table_restore_row_limit_message(limit: i64) -> String {
    format!(
        "Emulator limit, not a defect in this request: one table restore copies at most \
         {limit} rows."
    )
}

/// Why a restore whose names resolved to nothing is refused, naming WHICH of
/// them did.
///
/// `smithy_query(query="shape", shape="InvalidTableRestoreArgumentFault")` — "The
/// value specified for the sourceDatabaseName, sourceSchemaName, or
/// sourceTableName parameter, or a combination of these, doesn't exist in the
/// snapshot" — plus the operation's own "you cannot use
/// RestoreTableFromClusterSnapshot to restore a table with the same name as an
/// existing table in an Amazon Redshift cluster". Which value was wrong is what
/// the caller has to fix, so the sentence names it.
pub fn invalid_table_restore_argument_message(outcome: &TableRestoreOutcome) -> String {
    match outcome {
        TableRestoreOutcome::SourceDatabaseAbsent => {
            "The value specified for SourceDatabaseName doesn't exist in the snapshot."
        }
        TableRestoreOutcome::SourceTableAbsent => {
            "The value specified for the sourceDatabaseName, sourceSchemaName, or \
             sourceTableName parameter, or a combination of these, doesn't exist in the snapshot."
        }
        TableRestoreOutcome::TargetDatabaseAbsent => {
            "The value specified for TargetDatabaseName is not a database of the cluster the \
             table is restored to."
        }
        TableRestoreOutcome::TargetSchemaAbsent => {
            "The value specified for TargetSchemaName is not a schema of the database the table \
             is restored to."
        }
        TableRestoreOutcome::NewTableExists => {
            "The value specified for NewTableName is the name of an existing table in the \
             cluster: a restored table cannot overwrite an existing one."
        }
        // The two outcomes that are not a value the request got wrong: each is
        // answered by the handler with the refusal that fits it, so neither
        // reaches this sentence.
        TableRestoreOutcome::Restored { .. } | TableRestoreOutcome::SourceTableTooLarge { .. } => {
            "The value specified for the sourceDatabaseName, sourceSchemaName, or \
             sourceTableName parameter, or a combination of these, doesn't exist in the snapshot."
        }
    }
    .to_string()
}

/// A table restore request as this service stores it.
#[derive(Debug, Clone)]
pub struct StoredTableRestore {
    /// Exactly what `DescribeTableRestoreStatus` reports, so a read never
    /// recomputes it.
    pub status: crate::ops::types::TableRestoreStatus,
    /// When the clock-owed [`TABLE_RESTORE_STATUS_IN_PROGRESS`] ->
    /// [`TABLE_RESTORE_STATUS_SUCCEEDED`] transition falls due, and `None` for a
    /// record that is already in a terminal status — a restore that failed owes
    /// no transition at all.
    ///
    /// See [`PROVISIONING_SPAN`]: writing a table into a running cluster is the
    /// same slow work provisioning one is, so a caller that reads straight after
    /// the call still observes the transient status.
    pub succeeded_at: Option<Instant>,
}

/// Why a request naming a `TableRestoreRequestId` this account made no restore
/// under is refused, naming the id the request asked about.
///
/// `smithy_query(query="shape", shape="TableRestoreNotFoundFault")` — "The
/// specified `TableRestoreRequestId` value was not found" — which is the fixed
/// text; the id the request named is the variable part, because that is what the
/// caller has to recognise.
pub fn table_restore_not_found_message(request_id: &str) -> String {
    format!("The specified TableRestoreRequestId value {request_id} was not found.")
}

/// A fresh `TableRestoreRequestId`.
///
/// "The unique identifier for the table restore request"
/// (`smithy_query(query="doc", shape="TableRestoreStatus",
/// member="TableRestoreRequestId")`): minted by the service, carrying nothing
/// from the request, and stored — so every later read of that restore reports the
/// same id. Drawn by the same generator every other wire-visible identifier of
/// this service is ([`new_usage_limit_id`]).
pub fn new_table_restore_request_id() -> String {
    new_usage_limit_id()
}

impl ComAmazonawsRedshiftRedshiftserviceversion20121201State {
    /// The stored table restore request with this `TableRestoreRequestId`, if
    /// any.
    pub fn table_restore(&self, request_id: &str) -> Option<&StoredTableRestore> {
        self.table_restores.iter().find(|stored| {
            stored.status.table_restore_request_id.as_deref() == Some(request_id)
        })
    }

    /// Append one table restore request, keeping at most `retained` of them: the
    /// OLDEST record is dropped to make room, so the log a served request appends
    /// to is bounded however many restores a caller asks for while the request
    /// just made is always readable.
    ///
    /// The bound is the CALLER's ([`TABLE_RESTORE_QUOTA`], which the handler
    /// passes), so it is stated at the call it governs rather than only here.
    pub fn record_table_restore(&mut self, restore: StoredTableRestore, retained: usize) {
        while self.table_restores.len() >= retained.max(1) {
            self.table_restores.remove(0);
        }
        self.table_restores.push(restore);
    }

    /// How many table restores are still in flight at `now`, which is what
    /// [`IN_PROGRESS_TABLE_RESTORE_QUOTA`] bounds.
    ///
    /// A record whose completion is already DUE is not in flight, whether or not
    /// anybody has called since: the count is read off the same clock
    /// [`Self::settle`] materialises that transition with.
    pub fn table_restores_in_progress(&self, now: Instant) -> usize {
        self.table_restores
            .iter()
            .filter(|stored| {
                stored.status.status.as_deref() == Some(TABLE_RESTORE_STATUS_IN_PROGRESS)
                    && stored.succeeded_at.is_some_and(|due| now < due)
            })
            .count()
    }

    /// The table restore requests this read reports, in request order: every one
    /// of them, or only those of the cluster `cluster_identifier` names.
    ///
    /// `DescribeTableRestoreStatus` "lists the status of one or more table
    /// restore requests made using the RestoreTableFromClusterSnapshot API
    /// action. If you don't specify a value for the TableRestoreRequestId
    /// parameter, then DescribeTableRestoreStatus returns the status of all table
    /// restore requests ordered by the date and time of the request in ascending
    /// order" (`smithy_query(query="doc", operation="DescribeTableRestoreStatus")`)
    /// — and the store holds them in request order, so that order IS the one a
    /// read reports.
    pub fn table_restores_of(&self, cluster_identifier: Option<&str>) -> Vec<&StoredTableRestore> {
        self.table_restores
            .iter()
            .filter(|stored| {
                cluster_identifier.is_none_or(|wanted| {
                    stored.status.cluster_identifier.as_deref() == Some(wanted)
                })
            })
            .collect()
    }
}
