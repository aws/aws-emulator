// SPDX-License-Identifier: CC0-1.0

//! The message text QuickSight's conflict and not-found refusals carry.
//!
//! QuickSight identifies the resource a refusal is about the way the service itself
//! does, and every handler that refuses the same error class renders the same
//! template, so the wording a client reads does not depend on which operation it
//! called. Two templates cover the resources this crate holds:
//!
//! * a resource addressed by an ARN (a folder, a data source, a data set, a theme)
//!   is named by that ARN — `"<Kind> <arn> already exists"` for a conflict and
//!   `"<Kind> <arn> is not found"` for a miss, neither ending in a period;
//! * a group or a user, which QuickSight's identity path serves, is named by its name
//!   together with the account it belongs to.
//!
//! A refusal about an allowance names the resource whose allowance ran out, and
//! that resource is the parent the counted children hang on, not the child the
//! refused call named.
//!
//! The ARN is built from the serving region and the request's own path labels by
//! the builders beside these in [`crate::state`], so the resource a message names
//! is the resource the request addressed.

use super::{data_set_arn, data_source_arn, folder_arn};

/// The resource kind a folder refusal names.
const FOLDER: &str = "Folder";

/// The resource kind a data-source refusal names — one word, as the service's own
/// `DataSource` shape spells it, not the two words its documentation prose uses.
const DATA_SOURCE: &str = "DataSource";

/// `"<kind> <arn> already exists"`: the conflict a create hits when the id it names
/// is already taken by a resource of that kind.
pub fn arn_exists_message(kind: &str, arn: &str) -> String {
    format!("{kind} {arn} already exists")
}

/// `"<kind> <arn> is not found"`: the miss any operation keyed on an absent resource
/// of that kind reports.
pub fn arn_not_found_message(kind: &str, arn: &str) -> String {
    format!("{kind} {arn} is not found")
}

/// The conflict `CreateFolder` reports for a `FolderId` the account already holds.
pub fn folder_exists_message(region: &str, aws_account_id: &str, folder_id: &str) -> String {
    arn_exists_message(FOLDER, &folder_arn(region, aws_account_id, folder_id))
}

/// The miss a folder operation reports for a `FolderId` the account holds no folder
/// under.
pub fn folder_not_found_message(region: &str, aws_account_id: &str, folder_id: &str) -> String {
    arn_not_found_message(FOLDER, &folder_arn(region, aws_account_id, folder_id))
}

/// The miss a data-source operation reports for a `DataSourceId` the account holds
/// no data source under — the same text whether the call reads, updates or deletes.
pub fn data_source_not_found_message(
    region: &str,
    aws_account_id: &str,
    data_source_id: &str,
) -> String {
    arn_not_found_message(
        DATA_SOURCE,
        &data_source_arn(region, aws_account_id, data_source_id),
    )
}

/// The per-data-set ceiling `CreateRefreshSchedule` reports once a data set already
/// holds every refresh schedule it may hold.
///
/// The exhausted resource is the data set the schedules refresh, so the sentence
/// carries the allowance and that data set's ARN — QuickSight's refresh path calls a
/// data set it prepares for SPICE a prepared data source — rather than the schedule
/// the refused call named. No trailing period.
pub fn refresh_schedule_limit_message(
    region: &str,
    aws_account_id: &str,
    data_set_id: &str,
    max: usize,
) -> String {
    format!(
        "Reached max limit of {max} refresh schedules for preparedDataSource {}",
        data_set_arn(region, aws_account_id, data_set_id)
    )
}

/// The conflict `CreateGroup` reports for a group name its namespace already holds.
///
/// QuickSight's identity path names the group rather than an ARN, and reports the
/// account it belongs to and the group name as labelled fields after the sentence,
/// which ends in a period and a space.
pub fn group_exists_message(group_name: &str, aws_account_id: &str) -> String {
    format!(
        "Group {group_name} already exists. AWS account id: {aws_account_id}, groupName: {group_name}. "
    )
}

/// The conflict `RegisterUser` reports for a user name its namespace already holds.
///
/// A user is a resource of the same identity path a group is, so it is named the same
/// way: the user name in the sentence, then the account it belongs to and that name as
/// labelled fields, ending in a period and a space.
pub fn user_exists_message(user_name: &str, aws_account_id: &str) -> String {
    format!(
        "User {user_name} already exists. AWS account id: {aws_account_id}, userName: {user_name}. "
    )
}

/// The miss a user operation reports for a `UserName` the namespace holds no user
/// under — the same text whether the call reads, updates or deletes, and the same
/// template the group reads answer a group nobody holds with.
pub fn user_not_found_message(user_name: &str, namespace: &str, aws_account_id: &str) -> String {
    format!("User {user_name} not found in namespace {namespace} in account {aws_account_id}.")
}
