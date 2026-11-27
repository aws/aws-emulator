// SPDX-License-Identifier: CC0-1.0

//! Whether this ACCOUNT has Athena's system tables enabled, and under which key.
//!
//! `SystemTableStatus` states the scope: "Because system tables are enabled at the account
//! level, every status reflects the same account-wide state." So the enablement is not a
//! property of a workgroup, of a catalog or of one table — it is ONE configuration the
//! account either holds or does not, which is why it lives here as a single record rather
//! than as a registry keyed by anything.
//!
//! `CreateSystemTableConfiguration` is what writes it ("creating this configuration enables
//! them"), `UpdateSystemTableConfiguration` changes the key it holds,
//! `DeleteSystemTableConfiguration` takes it away, and `GetSystemTableConfiguration` answers
//! out of that same record. Both projections of the enablement derive from it as well — the
//! `ListSystemTableStatuses` page and the `SystemTableStatuses` a read of any workgroup
//! reports ("The read-only enablement status of each system table for the account. This
//! field is returned by `GetWorkGroup` and is ignored on write") — so a client cannot find
//! the account enabled in one answer and disabled in another.
//!
//! The configuration has no lifecycle the clock owes: a create answers with `CreatedAt`
//! already set and the system tables already enabled, so nothing here is materialised by a
//! later read.

use std::time::SystemTime;

use super::*;

use crate::ops::types::{SystemTableStatus, Tag};

/// The one value `SystemTableName` takes, from
/// `smithy_query(query="shape", shape="SystemTableName")`.
///
/// `CreateSystemTableConfigurationInput$TableName` documents what it means: "The only
/// accepted value is ALL, the reserved keyword that enables all system tables for the
/// account." The boundary walk refuses every other value against the modelled enum, so a
/// handler that reaches its own body has been given this one.
pub const SYSTEM_TABLE_NAME_ALL: &str = "ALL";

/// What this account holds when its system tables are enabled: when the configuration was
/// created, when it was last changed, and the key its data is encrypted under.
///
/// There is at most one of these per account (see the module docs), so it carries no name:
/// `TableName` is reported as [`SYSTEM_TABLE_NAME_ALL`] — "Always ALL", as
/// `CreateSystemTableConfigurationOutput$TableName` puts it — rather than stored.
#[derive(Debug, Clone)]
pub struct SystemTableConfigurationRecord {
    /// "The Amazon Resource Name (ARN) of the AWS Key Management Service (AWS KMS) key to
    /// use for encrypting the system table data", as the write that set it carried it, and
    /// `None` for a configuration no write gave a key.
    pub kms_key_arn: Option<String>,
    /// When the configuration was created. Set once, by the create, so every read reports
    /// the same instant for it.
    pub created_at: SystemTime,
    /// When it was last modified: the create's own instant until an update changes the
    /// configuration, and that update's instant from then on.
    pub last_modified_at: SystemTime,
    /// The tags the create carried. Kept on the record so nothing a request submitted is
    /// dropped; Athena's tagging calls address "workgroups, data catalogs, and capacity
    /// reservations" (`Tag`), which does not include this configuration, so no ARN names it
    /// and no tagging call reaches these.
    pub tags: Vec<Tag>,
}

/// What every answer about the account's system table configuration carries: the three
/// members `CreateSystemTableConfigurationOutput`, `GetSystemTableConfigurationOutput` and
/// `UpdateSystemTableConfigurationOutput` each declare, plus the table name all three report.
///
/// One projection for the three operations, so a create, a read and an update can never
/// report different values for one configuration.
#[derive(Debug, Clone)]
pub struct ReportedSystemTableConfiguration {
    /// `TableName`, which is "Always ALL" — see [`SYSTEM_TABLE_NAME_ALL`].
    pub table_name: String,
    /// The key the configuration holds, and `None` for a configuration no write gave one.
    pub kms_key_arn: Option<String>,
    /// `CreatedAt`, at the resolution this service reports an instant at.
    pub created_at: ::aws_smithy_types::DateTime,
    /// `LastModifiedAt`, at that same resolution.
    pub last_modified_at: ::aws_smithy_types::DateTime,
}

impl SystemTableConfigurationRecord {
    /// This configuration as every operation of the slice reports it.
    #[must_use]
    pub fn reported(&self) -> ReportedSystemTableConfiguration {
        ReportedSystemTableConfiguration {
            table_name: SYSTEM_TABLE_NAME_ALL.to_string(),
            kms_key_arn: self.kms_key_arn.clone(),
            created_at: reported_time(self.created_at),
            last_modified_at: reported_time(self.last_modified_at),
        }
    }
}

/// This account's system table configuration, and the create token that minted it.
///
/// A registry of at most one record (see the module docs). Every operation of the slice goes
/// through this type, so the enablement a listing reports is the enablement a read of the
/// configuration reports.
#[derive(Debug, Default)]
pub struct SystemTableConfigurationRegistry {
    /// The configuration this account holds, or `None` while its system tables are off —
    /// the position every account is in before any create call.
    configuration: Option<SystemTableConfigurationRecord>,
    /// The `ClientRequestToken` of the create that minted [`Self::configuration`], when it
    /// carried one, so a retry of that very create is answered rather than refused:
    /// `CreateSystemTableConfigurationInput$ClientRequestToken` is "used to ensure the
    /// request to create the system table configuration is idempotent (executes only once)".
    ///
    /// One token rather than a map of them, and dropped with the configuration it minted:
    /// the account holds at most one configuration, so at most one create is replayable at
    /// any instant and the registry cannot grow with the calls made to it.
    creation_token: Option<String>,
}

impl SystemTableConfigurationRegistry {
    /// The configuration this account holds, or `None` while its system tables are off.
    #[must_use]
    pub fn configuration(&self) -> Option<&SystemTableConfigurationRecord> {
        self.configuration.as_ref()
    }

    /// Whether this account's system tables are enabled, which is what every status
    /// reported for one of them reflects.
    #[must_use]
    pub fn enabled(&self) -> bool {
        self.configuration.is_some()
    }

    /// The configuration `token` minted: the one this account holds, when `token` is the
    /// `ClientRequestToken` of the create that wrote it — a retry of that very create rather
    /// than a second one. `None` for any other token, and for an account that holds none, so
    /// a caller of this never has to assert that a configuration is there.
    #[must_use]
    pub fn minted_configuration(&self, token: &str) -> Option<&SystemTableConfigurationRecord> {
        if self.creation_token.as_deref() == Some(token) {
            self.configuration.as_ref()
        } else {
            None
        }
    }

    /// Enable this account's system tables at `now`, under `kms_key_arn`, and answer with
    /// the configuration that create wrote.
    ///
    /// `LastModifiedAt` is the create's own instant: nothing has modified the configuration
    /// yet, and a member a client reads has to carry the instant it describes rather than
    /// nothing at all.
    pub fn create(
        &mut self,
        kms_key_arn: Option<String>,
        tags: Vec<Tag>,
        token: Option<String>,
        now: SystemTime,
    ) -> &SystemTableConfigurationRecord {
        self.creation_token = token;
        self.configuration
            .insert(SystemTableConfigurationRecord {
                kms_key_arn,
                created_at: now,
                last_modified_at: now,
                tags,
            })
    }

    /// Change the key the held configuration is encrypted under to `kms_key_arn`, as of
    /// `now`, and answer with the configuration as it stands — or `None` when this account
    /// holds none, there being nothing to update.
    ///
    /// `UpdateSystemTableConfiguration` states the rule: "Fields that you omit retain their
    /// current values." So an update that names no key leaves the key the configuration
    /// holds exactly where it was, and only a key the request CARRIES replaces it. That is
    /// what makes the operation `@idempotent`: the same update applied twice leaves the same
    /// configuration, and `CreatedAt` is untouched throughout, the configuration not being
    /// created again.
    ///
    /// `LastModifiedAt` moves only where the configuration really changed — an update that
    /// names nothing, or names the key already held, modifies nothing, and a member that
    /// reports when the configuration was last modified must not move for a call that left
    /// it as it was.
    pub fn update(
        &mut self,
        kms_key_arn: Option<String>,
        now: SystemTime,
    ) -> Option<&SystemTableConfigurationRecord> {
        let record = self.configuration.as_mut()?;
        if let Some(kms_key_arn) = kms_key_arn {
            if record.kms_key_arn.as_deref() != Some(kms_key_arn.as_str()) {
                record.kms_key_arn = Some(kms_key_arn);
                record.last_modified_at = now;
            }
        }
        Some(record)
    }

    /// Disable this account's system tables, and answer whether a configuration was there to
    /// remove.
    ///
    /// The create token goes with the configuration it minted: a replay of that create after
    /// the configuration it made was deleted is a request to enable the system tables again,
    /// not a retry of a call whose result still stands.
    pub fn delete(&mut self) -> bool {
        self.creation_token = None;
        self.configuration.take().is_some()
    }
}

/// The status of every system table Athena publishes, given whether this account's system
/// tables are `enabled`.
///
/// One entry per table of [`SYSTEM_TABLES`], in that catalog's order, each carrying the SAME
/// account-wide state: `SystemTableStatus` documents exactly that — "Because system tables
/// are enabled at the account level, every status reflects the same account-wide state". So
/// a client discovers which system tables Athena has and whether this account has them,
/// rather than having to tell a table that is off from one that does not exist.
#[must_use]
pub fn system_table_statuses(enabled: bool) -> Vec<SystemTableStatus> {
    SYSTEM_TABLES
        .iter()
        .map(|table| SystemTableStatus {
            table_name: (*table).to_string(),
            enabled,
        })
        .collect()
}

/// The message a create addressed to an account that already holds a system table
/// configuration is refused with.
///
/// `CreateSystemTableConfiguration` states the rule and the remedy together — "If a
/// configuration already exists, use `UpdateSystemTableConfiguration` to update it" — and
/// the sentence interpolates nothing from the request: the account holds at most one
/// configuration, so there is no name to report.
pub const SYSTEM_TABLE_CONFIGURATION_EXISTS: &str =
    "A system table configuration already exists for this account. Use \
     UpdateSystemTableConfiguration to update it.";

/// The message a read or an update addressed to an account that holds no system table
/// configuration answers with.
///
/// Fixed text for the same reason: what is missing is the account's one configuration, not a
/// resource some name in the request addressed.
pub const SYSTEM_TABLE_CONFIGURATION_NOT_FOUND: &str =
    "No system table configuration exists for this account.";

/// The cursor a `ListSystemTableStatuses` page hands back: the catalog it is paging — the
/// account's, the operation modelling no member that narrows it — and the position to resume
/// at.
fn system_table_statuses_cursor(position: usize) -> String {
    format!("systemtablestatuses{CURSOR_SEPARATOR}{ACCOUNT_SCOPE}{CURSOR_SEPARATOR}{position}")
}

impl ComAmazonawsAthenaAmazonathenaState {
    /// The `NextToken` a `ListSystemTableStatuses` page resumed at `position` hands back.
    pub fn system_table_statuses_page_token(&self, position: usize) -> String {
        ::hex::encode(
            self.page_token_key
                .mint(&system_table_statuses_cursor(position)),
        )
    }

    /// The position `token` resumes a system-table-status listing at, or `None` for a token
    /// this store did not mint for THAT listing.
    pub fn system_table_statuses_page_position(&self, token: &str) -> Option<usize> {
        let cursor = self.page_cursor(token)?;
        cursor_position(&cursor, "systemtablestatuses", ACCOUNT_SCOPE)
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    /// RULE: every status reports the account-wide state, for every table of the catalog and
    /// for both states that account can be in.
    #[test]
    fn every_status_reports_the_account_wide_state() {
        for enabled in [false, true] {
            let statuses = system_table_statuses(enabled);
            assert_eq!(
                statuses.len(),
                SYSTEM_TABLES.len(),
                "case {enabled}: expected one status per system table, got {statuses:?}"
            );
            for (status, table) in statuses.iter().zip(SYSTEM_TABLES.iter().copied()) {
                assert_eq!(
                    status.table_name, table,
                    "case {enabled}: expected the catalog's order, got {statuses:?}"
                );
                assert_eq!(
                    status.enabled, enabled,
                    "case {enabled}: expected the account-wide state, got {statuses:?}"
                );
            }
        }
    }

    /// RULE: the registry holds at most one configuration, a create enables the account, an
    /// update replaces only a key the request carries and leaves every other value where it
    /// was, and a delete disables the account again.
    #[test]
    fn the_registry_holds_one_configuration_at_a_time() {
        let key = "arn:aws:kms:us-east-1:000000000000:key/system-tables";
        let other_key = "arn:aws:kms:us-east-1:000000000000:alias/system-tables";
        let created_at = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let updated_at = created_at + Duration::from_secs(60);
        let mut registry = SystemTableConfigurationRegistry::default();
        assert!(
            !registry.enabled(),
            "an account nothing wrote holds no configuration, got {registry:?}"
        );

        let created = registry.create(
            Some(key.to_string()),
            Vec::new(),
            Some("token".to_string()),
            created_at,
        );
        assert_eq!(
            (created.created_at, created.last_modified_at),
            (created_at, created_at),
            "a create reports its own instant under both members, got {created:?}"
        );
        assert!(registry.enabled(), "a create enables the account");
        assert!(
            registry.minted_configuration("token").is_some(),
            "the create's token names the configuration it minted"
        );
        assert!(
            registry.minted_configuration("another").is_none(),
            "another token names no configuration"
        );

        // An update that names no key retains the one the configuration holds, so nothing
        // was modified and the modification instant does not move.
        let retained = registry
            .update(None, updated_at)
            .expect("the held configuration is updated")
            .clone();
        assert_eq!(
            (
                retained.created_at,
                retained.last_modified_at,
                retained.kms_key_arn.clone()
            ),
            (created_at, created_at, Some(key.to_string())),
            "an omitted field retains its current value, got {retained:?}"
        );

        // A key the request carries replaces the one held, which is a change, so the
        // modification instant moves and the creation instant does not.
        let changed = registry
            .update(Some(other_key.to_string()), updated_at)
            .expect("the held configuration is updated")
            .clone();
        assert_eq!(
            (
                changed.created_at,
                changed.last_modified_at,
                changed.kms_key_arn.clone()
            ),
            (created_at, updated_at, Some(other_key.to_string())),
            "an update replaces the key it carries, got {changed:?}"
        );

        // The same update again changes nothing, so nothing moves.
        let again = registry
            .update(Some(other_key.to_string()), updated_at + Duration::from_secs(60))
            .expect("the held configuration is updated")
            .clone();
        assert_eq!(
            (again.created_at, again.last_modified_at),
            (created_at, updated_at),
            "an update that changes nothing moves nothing, got {again:?}"
        );

        assert!(registry.delete(), "the held configuration is removed");
        assert!(
            !registry.enabled(),
            "a delete disables the account, got {registry:?}"
        );
        assert!(
            registry.minted_configuration("token").is_none(),
            "the create token goes with the configuration it minted"
        );
        assert!(
            registry.update(None, updated_at).is_none(),
            "there is nothing left to update"
        );
        assert!(
            !registry.delete(),
            "a second delete finds nothing to remove"
        );
    }
}
