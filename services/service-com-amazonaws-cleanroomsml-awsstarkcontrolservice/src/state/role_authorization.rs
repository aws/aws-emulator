// SPDX-License-Identifier: CC0-1.0

//! Whether the IAM role a create hands Clean Rooms ML lets it do the create's work.
//!
//! ## The rule this module implements
//!
//! A create that names an IAM role names the role Clean Rooms ML ACTS AS: it assumes that
//! role and then reads, under it, the data the request points at
//! (`CreateTrainingDatasetRequest$roleArn` is "the ARN of the IAM role that Clean Rooms ML
//! can assume to read the data referred to in the dataSource field of each dataset"). So the
//! role is resolved BEFORE anything is persisted, and a call the role does not let the service
//! make is refused with the modelled `AccessDeniedException` (HTTP 403) — the error that
//! reports insufficient access — in the three states such a resolution can fail in:
//!
//! * the request names no role at all, so there is no principal whose policies could allow
//!   the call ([`role_arn_not_present_message`]); the seam that answers that one BEFORE
//!   request validation runs is `crate::service`'s, because the framework has already chosen
//!   a status by the time a handler could speak;
//! * the service cannot assume the role it names ([`role_not_assumable_message`]);
//! * it can assume the role, but the permissions in force on that role do not let it read the
//!   Glue table the request points at ([`table_read_denied_message`]).
//!
//! Nothing is created in any of the three, so a later Get or List reports no such resource.
//!
//! ## What it does NOT judge
//!
//! Only what IAM ANSWERED. Every reading here is the tree's one reading of a caller-supplied
//! role ([`::service_common::iam_role`]), which establishes nothing — and so refuses nothing —
//! for a value naming no role of this account, an answer this emulator could not read, or an
//! emulator carrying no IAM to ask at all. The table-read half asks what the permissions in
//! force on the role PERMIT, and the service has to make that read for the create to mean
//! anything, so a policy set that does not allow it withholds the create whether it says so
//! explicitly or says nothing at all — which is the reading
//! [`::service_common::iam_role::role_withholds_action`] takes of an answer about a role this
//! account's IAM holds.

use ::service_com_amazonaws_glue_awsglue::state as glue;

use crate::ops::types::Dataset;

/// The service principal Clean Rooms ML assumes a caller-supplied role as.
///
/// The service's own principal, as its model states it
/// (`aws.api#service`'s `cloudTrailEventSource` for `AWSStarkControlService` is
/// `cleanrooms-ml.amazonaws.com`), which is the principal a role's trust policy has to admit
/// for this service to take the role on.
pub const SERVICE_PRINCIPAL: &str = "cleanrooms-ml.amazonaws.com";

/// The IAM action a read of one Data Catalog table asks for: AWS Glue's own read of a table
/// (`glue:GetTable`), which is the call Clean Rooms ML makes under the supplied role to
/// resolve the table a dataset points at.
pub const GLUE_TABLE_READ_ACTION: &str = "glue:GetTable";

/// The ARN service segment of a Data Catalog resource.
const GLUE_ARN_SERVICE: &str = "glue";

/// The ARN resource-type segment of a Data Catalog table, whose resource portion carries the
/// table's database as well as its own name (`table/<database>/<table>`).
const GLUE_TABLE_ARN_SEGMENT: &str = "table";

/// How many of a create's datasets this judges.
///
/// `CreateTrainingDatasetRequest$trainingData` is a `DatasetList` modelled
/// `@length(min: 1, max: 1)`, so one request names ONE dataset and that list's own validation
/// refuses a longer one before this runs. The bound is stated here rather than assumed, so the
/// number of authorization questions one request can ask is a constant of this module.
const MAX_JUDGED_DATASETS: usize = 1;

/// The member every operation of this service expects a role ARN at, as the wire spells it.
pub const ROLE_ARN_MEMBER: &str = "roleArn";

/// The sentence Clean Rooms ML words for a role it cannot take on: fixed text with the role
/// ARN the request carried.
#[must_use]
pub fn role_not_assumable_message(role_arn: &str) -> String {
    format!("Service was not able to assume role {role_arn}.")
}

/// The sentence Clean Rooms ML words for a call that carries no role at all: fixed text with
/// the member the role was expected at and the IAM action the call asks for, both read off the
/// request, closed by the clause every authorization refusal of this service ends in
/// ([`super::resource_authorization::denied_by_no_resource_based_policy`]).
#[must_use]
pub fn role_arn_not_present_message(member: &str, action: &str) -> String {
    super::resource_authorization::denied_by_no_resource_based_policy(
        &format!(
            "Role Arn not present in request. Please provide a valid role arn in the path: \
             {member}."
        ),
        action,
    )
}

/// The IAM action `operation` asks for, spelled as a policy names it — the one spelling every
/// refusal of this service names an action by
/// ([`super::resource_authorization::iam_action`]).
pub use super::resource_authorization::iam_action;

/// The sentence Clean Rooms ML words for a role it assumed and that may not read the table a
/// dataset points at: fixed text with the role ARN the request carried and the ARN of the
/// table it named.
#[must_use]
pub fn table_read_denied_message(role_arn: &str, table_arn: &str) -> String {
    format!("Role {role_arn} does not have read access to Glue table {table_arn}.")
}

/// The ARN of the Data Catalog table `table` of `database`, held in the catalog `catalog_id`
/// and addressed in `region`.
///
/// The canonical layout and partition come from the shared minter, so this ARN matches every
/// other ARN this emulator emits; only the ACCOUNT field is replaced, because a dataset may
/// name the catalog of another account and that account is what such an ARN names.
#[must_use]
pub fn glue_table_arn(region: &str, catalog_id: &str, database: &str, table: &str) -> String {
    let canonical = ::engine::emulator_arn(
        GLUE_ARN_SERVICE,
        region,
        &format!("{GLUE_TABLE_ARN_SEGMENT}/{database}/{table}"),
    );
    match ::service_common::arn::parse(&canonical) {
        Some(fields) => format!(
            "arn:{}:{}:{}:{catalog_id}:{}",
            fields.partition, fields.vendor, fields.region, fields.resource
        ),
        None => canonical,
    }
}

/// Why Clean Rooms ML cannot take `role_arn` on, or `None` when nothing establishes that it
/// cannot.
///
/// The whole question is the tree's one reading of it
/// ([`::service_common::iam_role::role_is_unassumable`]): which role of this account the ARN
/// names, IAM's own `GetRole`, and whether the trust document that role carries admits
/// [`SERVICE_PRINCIPAL`]. Shared by every create that names a role this service acts as, so
/// two of them cannot word the same refusal differently.
pub async fn unassumable_role(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    role_arn: &str,
) -> Option<String> {
    ::service_common::iam_role::role_is_unassumable(ctx, region, role_arn, SERVICE_PRINCIPAL)
        .await
        .then(|| role_not_assumable_message(role_arn))
}

/// Why Clean Rooms ML cannot read the training data `training_data` points at under
/// `role_arn`, or `None` when nothing establishes that it cannot.
///
/// The two states of one resolution, asked in the order the service performs them: the role is
/// taken on first ([`unassumable_role`]), and only a role it could take on is asked what it
/// may READ. The datasets are walked in the order the request lists them, bounded by
/// [`MAX_JUDGED_DATASETS`], so a create naming more than one is refused for the first table it
/// may not read — the one a caller fixes first.
pub async fn withheld_training_data_access(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    role_arn: &str,
    training_data: &[Dataset],
) -> Option<String> {
    if let Some(refusal) = unassumable_role(ctx, region, role_arn).await {
        return Some(refusal);
    }
    for dataset in training_data.iter().take(MAX_JUDGED_DATASETS) {
        let source = &dataset.input_config.data_source.glue_data_source;
        // The catalog a dataset leaves unnamed is the calling account's own, which is the
        // default AWS Glue documents for a `CatalogId` and the one the record of a created
        // dataset carries (`crate::state::stored_training_data`).
        let catalog_id = glue::resolve_catalog_id(source.catalog_id.as_ref());
        let table_arn = glue_table_arn(
            region,
            &catalog_id,
            &source.database_name,
            &source.table_name,
        );
        if ::service_common::iam_role::role_withholds_action(
            ctx,
            region,
            role_arn,
            GLUE_TABLE_READ_ACTION,
            &table_arn,
        )
        .await
        {
            return Some(table_read_denied_message(role_arn, &table_arn));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A table's ARN names the catalog that holds it in its account field and the database
    /// beside the table in its resource field. The rows span what the form generalises over:
    /// the region the request reached, the catalog the dataset named, and the database and
    /// table names.
    #[test]
    fn a_table_arn_names_its_catalog_database_and_table() {
        let account = ::engine::emulator_account_id();
        for (region, catalog, database, table) in [
            ("us-west-2", account, "clean_rooms_demo", "interactions"),
            ("eu-west-1", account, "lake", "events"),
            ("ap-southeast-2", "210987654321", "shared", "profiles"),
        ] {
            let built = glue_table_arn(region, catalog, database, table);
            let expected =
                format!("arn:aws:glue:{region}:{catalog}:table/{database}/{table}");
            assert_eq!(
                built, expected,
                "case {region:?}/{catalog:?}: expected {expected:?}, got {built:?}"
            );
        }
    }

    /// Each refusal is fixed text plus the values the request carried, character for
    /// character. The expected sentences are spelled out rather than built from the
    /// formatter under test, so a changed template fails here.
    #[test]
    fn a_refusal_names_the_values_the_request_carried() {
        for (role_arn, expected) in [
            (
                "arn:aws:iam::000000000000:role/CleanRoomsMLRole",
                "Service was not able to assume role \
                 arn:aws:iam::000000000000:role/CleanRoomsMLRole.",
            ),
            (
                "arn:aws-us-gov:iam::210987654321:role/service-role/reader",
                "Service was not able to assume role \
                 arn:aws-us-gov:iam::210987654321:role/service-role/reader.",
            ),
        ] {
            let stated = role_not_assumable_message(role_arn);
            assert_eq!(
                stated, expected,
                "case {role_arn:?}: expected {expected:?}, got {stated:?}"
            );
        }

        for (role_arn, table_arn, expected) in [
            (
                "arn:aws:iam::000000000000:role/CleanRoomsMLRole",
                "arn:aws:glue:us-west-2:000000000000:table/lake/events",
                "Role arn:aws:iam::000000000000:role/CleanRoomsMLRole does not have read \
                 access to Glue table arn:aws:glue:us-west-2:000000000000:table/lake/events.",
            ),
            (
                "arn:aws-us-gov:iam::210987654321:role/service-role/reader",
                "arn:aws:glue:eu-west-1:210987654321:table/shared/profiles",
                "Role arn:aws-us-gov:iam::210987654321:role/service-role/reader does not have \
                 read access to Glue table \
                 arn:aws:glue:eu-west-1:210987654321:table/shared/profiles.",
            ),
        ] {
            let stated = table_read_denied_message(role_arn, table_arn);
            assert_eq!(
                stated, expected,
                "case {role_arn:?}: expected {expected:?}, got {stated:?}"
            );
        }
    }
}
