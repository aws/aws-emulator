// SPDX-License-Identifier: CC0-1.0

//! What a workgroup's configuration holds when nobody said, and what a read reports for it.
//!
//! Two different questions, answered in two different places, and the difference is what
//! makes an update that names one switch leave the others where they were:
//!
//! * [`default_work_group_configuration`] is the configuration a workgroup is CREATED with
//!   when the create carried none at all. It is written once, by the create, and from then
//!   on it is the workgroup's own configuration like any other.
//! * [`effective_work_group_configuration`] is how a read REPORTS the configuration a
//!   workgroup holds: every switch the shape carries with the value it has in effect, and
//!   the per-system-table statuses and the query scheduling type AWS serves beside them. It
//!   derives, and writes nothing.
//!
//! So a create that named no configuration and a create that named one which happens to be
//! empty are two different workgroups — the first holds Athena's defaults, the second holds
//! what it was given — and both read back as what they hold.

use crate::ops::types::WorkGroupConfiguration;

/// What one of the boolean switches of a `WorkGroupConfiguration` means when no create or
/// update ever set it: off. `WorkGroupConfiguration$RequesterPaysEnabled` states it for its
/// own member — "The default is `false`" — and it is the reading every one of them documents,
/// each describing a workgroup that does less when the switch is not set. See
/// [`effective_work_group_configuration`].
pub const WORK_GROUP_SWITCH_UNSET: bool = false;

/// The system tables Athena publishes a status for, reported under
/// `WorkGroupConfiguration.SystemTableStatuses` and by `ListSystemTableStatuses`.
///
/// The value list of the model's `SystemTableType` enum, as the generated types render it
/// (`SYSTEM_TABLE_TYPE_VALUES` in [`crate::ops::types`]) — that enum is what
/// `SystemTableStatus$TableName` targets, so the catalog is read from the model rather than
/// written out here. One entry per table Athena has, so a client reading a workgroup or
/// listing the statuses discovers the whole set rather than having to tell a table that is
/// off from one that does not exist.
pub const SYSTEM_TABLES: &[&str] = crate::ops::types::SYSTEM_TABLE_TYPE_VALUES;

/// The system table that reports this account's query executions, the one table AWS serves a
/// status for: the one value of the model's `SystemTableType` enum, named here for the
/// readers that speak about that one table rather than about the whole catalog
/// ([`SYSTEM_TABLES`]).
pub const QUERY_EXECUTIONS_SYSTEM_TABLE: &str = "QUERY_EXECUTIONS";

/// How a workgroup schedules the queries submitted to it when nothing selected another way:
/// Athena's own scheduling.
///
/// RECORDED BEHAVIOUR, like [`SYSTEM_TABLES`]: the model declares no `QuerySchedulingType`
/// member, so there is nothing a request can set and every workgroup reports this one.
pub const QUERY_SCHEDULING_TYPE_DEFAULT: &str = "DEFAULT";

/// The configuration a workgroup is created with when the create carried NONE at all.
///
/// RECORDED BEHAVIOUR: `CreateWorkGroup$Configuration` is optional and the model states no
/// default for it, but AWS does not create a workgroup with nothing configured — a workgroup
/// created from the name alone comes up with Athena's own settings, in which the workgroup's
/// settings override the client's (`EnforceWorkGroupConfiguration`) and query metrics are
/// published to CloudWatch (`PublishCloudWatchMetricsEnabled`). The switches those settings
/// do not name keep the reading their own documentation gives them
/// ([`WORK_GROUP_SWITCH_UNSET`]).
///
/// This is the create path's decision, taken once and stored, which is why a create that
/// carried a configuration of its own does NOT get these defaults mixed into it: that
/// request said what the workgroup's configuration is.
#[must_use]
pub fn default_work_group_configuration() -> WorkGroupConfiguration {
    WorkGroupConfiguration {
        enforce_work_group_configuration: Some(DEFAULT_WORK_GROUP_SETTINGS_OVERRIDE),
        publish_cloud_watch_metrics_enabled: Some(DEFAULT_WORK_GROUP_METRICS_PUBLISHED),
        ..WorkGroupConfiguration::default()
    }
}

/// Whether a workgroup Athena configured itself overrides client-side settings. See
/// [`default_work_group_configuration`].
pub const DEFAULT_WORK_GROUP_SETTINGS_OVERRIDE: bool = true;

/// Whether a workgroup Athena configured itself publishes its query metrics to CloudWatch.
/// See [`default_work_group_configuration`].
pub const DEFAULT_WORK_GROUP_METRICS_PUBLISHED: bool = true;

/// The configuration a read reports for a workgroup that holds `stored`: every switch the
/// shape carries as a boolean, reported with the value it has in effect, beside the
/// system-table statuses and the query scheduling type.
///
/// `WorkGroupConfiguration$SystemTableStatuses` is "The read-only enablement status of each
/// system table for the account. This field is returned by `GetWorkGroup` and is ignored on
/// write", so the statuses are projected from the ACCOUNT's system table configuration
/// (`system_tables_enabled`, see [`crate::state::system_table_statuses`]) and from nothing
/// the workgroup holds — a write that carried them sets nothing.
///
/// `WorkGroupConfiguration` carries four of those switches — `EnforceWorkGroupConfiguration`,
/// `PublishCloudWatchMetricsEnabled`, `RequesterPaysEnabled` and
/// `EnableMinimumEncryptionConfiguration` — and each is documented by what the workgroup does
/// when it is set and when it is not, `RequesterPaysEnabled` stating "The default is `false`"
/// outright. So a workgroup whose configuration names none of them is not one whose settings
/// are unknown but one with that switch off, and that is what a read of its configuration
/// reports: an OFF switch, not an absent member.
///
/// DERIVED at read time and never written: the record keeps exactly what the create or
/// update put in it, which is what makes the switch a later update leaves alone the one that
/// write set. Every other member of the stored configuration is reported as it stands.
#[must_use]
pub fn effective_work_group_configuration(
    stored: &WorkGroupConfiguration,
    system_tables_enabled: bool,
) -> WorkGroupConfiguration {
    WorkGroupConfiguration {
        enforce_work_group_configuration: Some(
            stored
                .enforce_work_group_configuration
                .unwrap_or(WORK_GROUP_SWITCH_UNSET),
        ),
        publish_cloud_watch_metrics_enabled: Some(
            stored
                .publish_cloud_watch_metrics_enabled
                .unwrap_or(WORK_GROUP_SWITCH_UNSET),
        ),
        requester_pays_enabled: Some(
            stored
                .requester_pays_enabled
                .unwrap_or(WORK_GROUP_SWITCH_UNSET),
        ),
        enable_minimum_encryption_configuration: Some(
            stored
                .enable_minimum_encryption_configuration
                .unwrap_or(WORK_GROUP_SWITCH_UNSET),
        ),
        system_table_statuses: Some(crate::state::system_table_statuses(system_tables_enabled)),
        query_scheduling_type: Some(
            stored
                .query_scheduling_type
                .clone()
                .unwrap_or_else(|| QUERY_SCHEDULING_TYPE_DEFAULT.to_string()),
        ),
        ..stored.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RULE: a read reports every boolean switch of the configuration with the value it has
    /// in effect — the value the stored configuration carries, and off for one it does not —
    /// over the whole switch set rather than one member.
    #[test]
    fn every_switch_is_reported_with_the_value_it_has_in_effect() {
        // (case, the stored configuration, the four switches it must read back as, in the
        // order the shape declares them: enforce, publish, requester pays, minimum
        // encryption)
        let cases: Vec<(&str, WorkGroupConfiguration, [bool; 4])> = vec![
            (
                "a configuration that names no switch",
                WorkGroupConfiguration::default(),
                [false, false, false, false],
            ),
            (
                "the configuration a create that named none is given",
                default_work_group_configuration(),
                [true, true, false, false],
            ),
            (
                "every switch named, alternating",
                WorkGroupConfiguration {
                    enforce_work_group_configuration: Some(false),
                    publish_cloud_watch_metrics_enabled: Some(true),
                    requester_pays_enabled: Some(false),
                    enable_minimum_encryption_configuration: Some(true),
                    ..WorkGroupConfiguration::default()
                },
                [false, true, false, true],
            ),
            (
                "one switch named off, where the default has it on",
                WorkGroupConfiguration {
                    enforce_work_group_configuration: Some(false),
                    ..default_work_group_configuration()
                },
                [false, true, false, false],
            ),
        ];

        for (case, stored, expected) in cases {
            let reported = effective_work_group_configuration(&stored, false);
            let actual = [
                reported.enforce_work_group_configuration,
                reported.publish_cloud_watch_metrics_enabled,
                reported.requester_pays_enabled,
                reported.enable_minimum_encryption_configuration,
            ];
            assert_eq!(
                actual,
                expected.map(Some),
                "case {case:?}: expected the switches {expected:?}, got {actual:?}"
            );
        }
    }

    /// RULE: a read reports one status per system table Athena publishes, and the query
    /// scheduling type, for every workgroup — whatever else its configuration holds.
    #[test]
    fn the_system_tables_and_the_scheduling_type_are_reported_for_every_work_group() {
        for (case, stored) in [
            ("a configuration that names nothing", WorkGroupConfiguration::default()),
            (
                "the configuration a create that named none is given",
                default_work_group_configuration(),
            ),
        ] {
            let reported = effective_work_group_configuration(&stored, false);
            let statuses = reported
                .system_table_statuses
                .unwrap_or_else(|| panic!("case {case:?}: expected the system-table statuses"));
            assert_eq!(
                statuses.len(),
                SYSTEM_TABLES.len(),
                "case {case:?}: expected one entry per system table, got {statuses:?}"
            );
            for (status, table) in statuses.iter().zip(SYSTEM_TABLES.iter().copied()) {
                assert_eq!(
                    status.table_name, table,
                    "case {case:?}: expected the catalog's order, got {statuses:?}"
                );
                assert!(
                    !status.enabled,
                    "case {case:?}: a table nothing enabled must read as disabled, got \
                     {statuses:?}"
                );
            }
            assert_eq!(
                reported.query_scheduling_type.as_deref(),
                Some(QUERY_SCHEDULING_TYPE_DEFAULT),
                "case {case:?}: expected the default scheduling type, got {:?}",
                reported.query_scheduling_type
            );
        }
    }

    /// RULE: the statuses a read reports follow the ACCOUNT's system-table enablement, and
    /// nothing the workgroup's own stored configuration carries: the member is "returned by
    /// `GetWorkGroup` and is ignored on write".
    #[test]
    fn the_reported_statuses_follow_the_account_and_not_the_stored_configuration() {
        // (case, the configuration stored for the workgroup) — one that says nothing about
        // the system tables, and one carrying the member a write cannot set, each read back
        // under both states the ACCOUNT can be in.
        let stored_cases = [
            (
                "a configuration that carries no statuses",
                WorkGroupConfiguration::default(),
            ),
            (
                "a configuration a write tried to enable a table through",
                WorkGroupConfiguration {
                    system_table_statuses: Some(vec![crate::ops::types::SystemTableStatus {
                        table_name: QUERY_EXECUTIONS_SYSTEM_TABLE.to_string(),
                        enabled: true,
                    }]),
                    ..WorkGroupConfiguration::default()
                },
            ),
        ];

        for (case, stored) in stored_cases {
            for account_enabled in [false, true] {
                let reported = effective_work_group_configuration(&stored, account_enabled)
                    .system_table_statuses
                    .expect("the statuses are reported");
                assert_eq!(
                    reported
                        .iter()
                        .find(|status| status.table_name == QUERY_EXECUTIONS_SYSTEM_TABLE)
                        .map(|status| status.enabled),
                    Some(account_enabled),
                    "case {case:?} with the account enabled {account_enabled}: expected the \
                     account's enablement, got {reported:?}"
                );
            }
        }
    }
}
