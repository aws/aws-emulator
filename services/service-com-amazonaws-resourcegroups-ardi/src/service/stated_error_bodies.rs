// SPDX-License-Identifier: CC0-1.0

//! How this service's operations are served so that the error bodies they answer with are
//! the ones THIS service answers with.
//!
//! The one fact stated here is WHICH operations are served through that layer: every
//! operation the model declares, because the member an error body reports its text in
//! ([`super::error_message_member`]) is a property of this service's error bodies rather
//! than of any one operation. The member itself, the renaming and the whole
//! deserialize -> validate -> handle -> serialize walk stay where they already live.

use std::sync::Arc;

use service_common::submitted_collections::AnsweringAs;
use smithy_server_runtime::OperationRoutingTable;

/// Serve `$operation` with its error bodies reporting their text in the member this model
/// declares: one `crate::ops::<module>::<Operation>` per line, filed under its own modelled
/// name.
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
pub(super) fn stated_error_bodies<Eng: Send + Sync + 'static>(
    engine: &Arc<Eng>,
) -> OperationRoutingTable {
    let mut table = OperationRoutingTable::new();
    answering!(
        table,
        engine,
        cancel_tag_sync_task::CancelTagSyncTask,
        create_group::CreateGroup,
        delete_group::DeleteGroup,
        get_account_settings::GetAccountSettings,
        get_group::GetGroup,
        get_group_configuration::GetGroupConfiguration,
        get_group_query::GetGroupQuery,
        get_tag_sync_task::GetTagSyncTask,
        get_tags::GetTags,
        group_resources::GroupResources,
        list_group_resources::ListGroupResources,
        list_grouping_statuses::ListGroupingStatuses,
        list_groups::ListGroups,
        list_tag_sync_tasks::ListTagSyncTasks,
        put_group_configuration::PutGroupConfiguration,
        search_resources::SearchResources,
        start_tag_sync_task::StartTagSyncTask,
        tag::Tag,
        ungroup_resources::UngroupResources,
        untag::Untag,
        update_account_settings::UpdateAccountSettings,
        update_group::UpdateGroup,
        update_group_query::UpdateGroupQuery,
    );
    table
}
