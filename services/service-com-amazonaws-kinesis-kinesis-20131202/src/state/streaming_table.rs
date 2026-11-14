// SPDX-License-Identifier: CC0-1.0

//! The destination streaming table a channel delivers to, created in S3 Tables when the
//! channel is created.
//!
//! A channel with a streaming table destination is a PROMISE about S3 Tables: "The name of
//! the destination table. Amazon Kinesis Data Streams creates this table in the specified
//! table bucket" (`S3TablesConfiguration$TableName`). So provisioning the channel provisions
//! that table — in the table bucket and namespace the configuration names, under the name it
//! names — and S3 Tables reports it as a table this service manages ("The service that
//! manages the table", `GetTable$managedByService`), which is what tells a caller a
//! Kinesis-managed streaming table from one it created itself.
//!
//! The table is S3 TABLES' resource, so it is written into the S3 Tables store in the shared
//! engine through that service's own entry point for a table a service provisions
//! ([`tables_store::ComAmazonawsS3tablesS3tablebucketsState::create_service_managed_table`]):
//! every rule a table carries — its minted id and ARN, the ARN index it is addressed
//! through, the storage class it inherits, the uniqueness of its name inside the namespace —
//! stays in the crate that owns tables, and `GetTable` and `ListTables` resolve what this
//! writes exactly as they resolve a table a caller created. Nothing is copied into the
//! channel's own record: a table dropped in S3 Tables is gone from S3 Tables.
//!
//! What this NEVER does is claim a table it did not create. A destination naming a table
//! bucket or a namespace this account and region do not hold has nowhere for its table, and a
//! name a caller's own table already holds is not taken over or overwritten — a table name is
//! unique inside its namespace, and whose table that is belongs to the caller who created it.
//! In each of those cases no table is written and nothing of this service reports one;
//! `GetTable` and `ListTables` answer about the destination exactly what S3 Tables holds.
//!
//! Such a create is still ACCEPTED, and deliberately so: no recording of this service answers
//! a create whose destination names a table bucket or namespace the account does not hold, so
//! there is no error class and no sentence to state it with, and a refusal invented here would
//! be this emulator's wording rather than AWS's. What the create reports is therefore only
//! what it did do — a stored channel, read back from its own record — and never a table; an
//! operator sees the destination that got none in the warning below. A recording that shows
//! how AWS refuses it is what turns this into a refusal, in its own change.
//!
//! LOCK ORDER: the channel store is already held by the handler that calls in, and the S3
//! Tables store is taken under it — this service locks Kinesis then S3 Tables, never the
//! other way round.

use std::time::SystemTime;

use ::service_com_amazonaws_s3tables_s3tablebuckets::state as tables_store;

use crate::ops::types::S3TablesConfiguration;

/// The service principal S3 Tables reports as the manager of a table this service creates on
/// an account's behalf: this service's own principal, which is its ARN namespace under the
/// Amazon Web Services service domain.
pub fn managing_service_principal() -> String {
    format!("{}.amazonaws.com", super::SERVICE_ARN_NAMESPACE)
}

/// Create the destination table of every configuration in `configurations`.
///
/// Idempotent for this service's own tables: a destination whose table this service already
/// manages is already provisioned, so nothing is written for it — which is what lets a
/// retried create reach the same state.
///
/// A destination S3 Tables cannot take the table for leaves that destination WITHOUT a table,
/// and says so to an operator: nothing is written, and no answer of this service reports a
/// table that does not exist.
pub fn provision_streaming_tables<Eng: ::std::marker::Send + ::std::marker::Sync + 'static>(
    engine: &Eng,
    configurations: &[S3TablesConfiguration],
    now: SystemTime,
) {
    let Some(tables) = tables_store::service_state(engine) else {
        // Visible to an operator: without this line an engine carrying no S3 Tables store at
        // all would be indistinguishable from one that provisioned the table.
        ::tracing::warn!(
            "no S3 Tables store is reachable beside this service, so no destination streaming \
             table was created for this channel"
        );
        return;
    };
    let mut tables = tables.lock();
    for configuration in configurations {
        let outcome = tables.create_service_managed_table(
            &configuration.table_bucket_arn,
            &configuration.namespace,
            &configuration.table_name,
            &managing_service_principal(),
            now,
        );
        if let Err(refusal) = outcome {
            // The caller's own values are rendered in their escaped `Debug` form, so a
            // destination name carrying a newline cannot forge a log line.
            ::tracing::warn!(
                table_bucket_arn = ?configuration.table_bucket_arn,
                namespace = ?configuration.namespace,
                table_name = ?configuration.table_name,
                reason = ?refusal,
                "S3 Tables did not take the destination streaming table of this channel, so \
                 the channel has none"
            );
        }
    }
}
