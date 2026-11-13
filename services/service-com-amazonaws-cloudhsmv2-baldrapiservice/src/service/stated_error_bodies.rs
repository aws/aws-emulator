// SPDX-License-Identifier: CC0-1.0

//! Which of this service's operations are served so that the error bodies they answer
//! with are the ones THIS service answers with.
//!
//! Two facts are this service's own and are stated here: the error code its constraint
//! refusals carry — the same one [`super::EmulatedService::override_refusal`] picks,
//! because a restated message has to travel with the code a client branches on — and which
//! operations are served through which layer. Everything else is shared: the member an
//! error body reports its text in ([`super::error_message_member`]), and the two halves of
//! a constraint refusal only the request can answer ([`super::validation`]).
//!
//! EVERY operation of this model is listed, because the member an error body reports its
//! text in is a property of this service's error bodies rather than of any one operation.
//! The collection pass is listed separately: an operation whose model declares a bounded
//! LIST or MAP member is the only kind whose refusal can carry a clause about a
//! collection, and it pays a second read of the request's body for it.

use std::sync::Arc;

use service_common::submitted_collections::AnsweringAs;
use smithy_server_runtime::OperationRoutingTable;

/// The error code a constraint refusal of this service carries: the one modelled error
/// whose documentation describes this refusal class ("The request was rejected because it
/// is not a valid request").
pub(super) const CLOUD_HSM_INVALID_REQUEST: &str = "CloudHsmInvalidRequestException";

/// Serve `$operation` so that a clause about a collection states the collection the
/// request submitted, and its error bodies report their text in the member this model
/// declares: one `crate::ops::<module>::<Operation>` per line, filed under its own
/// modelled name.
macro_rules! quoting {
    ($table:expr, $engine:expr, $($module:ident :: $operation:ident),+ $(,)?) => {
        $(
            $table.add_operation(
                <crate::ops::$module::$operation<Eng> as ::smithy_server_runtime::Operation>::NAME,
                Box::new(super::validation::QuotingSubmittedValues::new(
                    crate::ops::$module::$operation::new(::std::sync::Arc::clone($engine)),
                )),
            );
        )+
    };
}

/// Serve `$operation` with its error bodies reporting their text in that member and no
/// collection pass at all — for an operation whose model declares no bounded collection
/// member, where that pass could never produce a clause and would only read the body a
/// second time.
macro_rules! answering {
    ($table:expr, $engine:expr, $($module:ident :: $operation:ident),+ $(,)?) => {
        $(
            $table.add_operation(
                <crate::ops::$module::$operation<Eng> as ::smithy_server_runtime::Operation>::NAME,
                Box::new(AnsweringAs::new(
                    crate::ops::$module::$operation::new(::std::sync::Arc::clone($engine)),
                    super::error_message_member::with_modelled_message_member,
                )),
            );
        )+
    };
}

/// The same modelled operations the generated table holds, each wrapped so that the error
/// bodies it answers with are the ones this service answers with, and built from the SAME
/// shared engine, so a wrapped operation reads and writes the one store.
///
/// The operations served `quoting` are the ones whose model declares a bounded collection
/// member: `CreateCluster` (`SubnetIds` `@length` 1..10 over elements with a `@pattern`,
/// `TagList` 1..50), the tag writes and `CopyBackupToRegion` (`TagList` / `TagKeyList`
/// 1..50) and the two describes (`Filters` 0..30). Every other operation takes the cheap
/// path.
pub(super) fn stated_error_bodies<Eng: Send + Sync + 'static>(
    engine: &Arc<Eng>,
) -> OperationRoutingTable {
    let mut table = OperationRoutingTable::new();
    quoting!(
        table,
        engine,
        copy_backup_to_region::CopyBackupToRegion,
        create_cluster::CreateCluster,
        describe_backups::DescribeBackups,
        describe_clusters::DescribeClusters,
        tag_resource::TagResource,
        untag_resource::UntagResource,
    );
    answering!(
        table,
        engine,
        create_hsm::CreateHsm,
        delete_backup::DeleteBackup,
        delete_cluster::DeleteCluster,
        delete_hsm::DeleteHsm,
        delete_resource_policy::DeleteResourcePolicy,
        get_resource_policy::GetResourcePolicy,
        initialize_cluster::InitializeCluster,
        list_tags::ListTags,
        modify_backup_attributes::ModifyBackupAttributes,
        modify_cluster::ModifyCluster,
        put_resource_policy::PutResourcePolicy,
        restore_backup::RestoreBackup,
    );
    table
}
