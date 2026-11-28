// SPDX-License-Identifier: CC0-1.0

//! The catalog of modules a caller can attach to a link's flows.
//!
//! A module is not a resource of this account: nothing in this API publishes one,
//! and `ListAvailableModules`, `GetAvailableModule`, `ListAvailableModuleVersions`
//! and `GetAvailableModuleVersion` are four READS over a catalog the service owns.
//! The modelled `SubscriptionType` says where an entry in that catalog comes from:
//! `MARKETPLACE` through an AWS Marketplace subscription, `PRIVATE` from a vendor
//! that shares a module with one account, and `AWS_BUILT_IN` for a module
//! "available to everyone"
//! (`smithy_query(query="shape", shape="SubscriptionType")`). This emulator models
//! neither Marketplace subscriptions nor private shares, so the catalog below is
//! the built-ins alone: every entry is `AWS_BUILT_IN`, and a caller filtering for
//! either of the other two types is answered with the empty page that is the truth
//! about this store.
//!
//! The built-ins themselves are the emulator's own, one per member of the modelled
//! `ConfigurableAction` enum — the capabilities a flow can turn on
//! (`smithy_query(query="shape", shape="ConfigurableAction")`) — each holding the
//! permissions that enum documents as the capability's minimum
//! (`OBSERVE` "Requires READ_REQUEST_BODY or READ_REQUEST_HEADERS", `FILTER`
//! "Requires FILTER", and so on). AWS publishes its built-in catalog nowhere this
//! emulator can read, so the names and versions here are this emulator's; what the
//! four reads answer is read out of this ONE catalog, so a module a listing reports
//! is a module its own reader resolves, with the same versions under both of its
//! readers.
//!
//! A catalog entry is fixed data rather than state a call writes, so its instants
//! are fixed too: a read of the catalog reports the same `createdAt` however often
//! it is taken, which is what a caller paging by it depends on.

use std::time::{Duration, SystemTime};

/// `SubscriptionType` member: the module is an AWS built-in, available to
/// everyone.
pub const SUBSCRIPTION_TYPE_AWS_BUILT_IN: &str = "AWS_BUILT_IN";

/// `SubscriptionStatus` member: the access is current, so the module's versions
/// can be attached.
pub const SUBSCRIPTION_STATUS_ACTIVE: &str = "ACTIVE";

/// `ModuleVersionStatus` member: the version is attachable.
pub const MODULE_VERSION_STATUS_ACTIVE: &str = "ACTIVE";
/// `ModuleVersionStatus` member: terminal, the version can no longer be attached
/// but is still listed.
pub const MODULE_VERSION_STATUS_DEPRECATED: &str = "DEPRECATED";

/// One version of a catalog module.
#[derive(Debug, Clone, Copy)]
pub struct AvailableModuleVersion {
    /// The modelled `SemanticVersion`: three numeric components.
    pub version: &'static str,
    /// A member of the modelled `ModuleVersionStatus`.
    pub status: &'static str,
    /// The data-access grants the version holds, members of the modelled
    /// `ModulePermission`.
    pub permissions: &'static [&'static str],
    /// The capabilities a flow can turn on when it attaches the version, members
    /// of the modelled `ConfigurableAction`.
    pub configurable_actions: &'static [&'static str],
    /// The JSON Schema for the parameters a flow may set when it attaches the
    /// version; `None` for a version that takes none.
    pub configuration_schema: Option<&'static str>,
    /// The vendor's release notes, when the version carries any.
    pub release_notes: Option<&'static str>,
    /// Seconds since the epoch at which the version was created.
    pub created_at: u64,
    /// Seconds since the epoch at which the version last changed.
    pub updated_at: u64,
}

impl AvailableModuleVersion {
    /// Whether a read of the catalog reports this version at all: the two readers
    /// of a version report the `ACTIVE` and `DEPRECATED` versions of a module, and
    /// a version in any other status is answered for as a missing one.
    pub fn listed(&self) -> bool {
        self.status == MODULE_VERSION_STATUS_ACTIVE
            || self.status == MODULE_VERSION_STATUS_DEPRECATED
    }

    /// The instant this version was created.
    pub fn created_at(&self) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(self.created_at)
    }

    /// The instant this version last changed.
    pub fn updated_at(&self) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(self.updated_at)
    }

    /// This version's ARN, as the modelled `ModuleVersionArn` pattern spells it:
    /// the partition and this service's ARN namespace, the Region, an EMPTY
    /// account field — a catalog module belongs to no account — then the module
    /// and the version.
    pub fn arn(&self, module_id: &str, region: &str) -> String {
        format!(
            "arn:aws:{}:{}::modules/{module_id}/versions/{}",
            super::ARN_VENDOR,
            super::region_dns_label(region),
            self.version
        )
    }

    /// The cursor `ListAvailableModuleVersions` pages this version by.
    ///
    /// That listing reports the highest version first, counted numerically rather
    /// than lexically, while the shared page walk resumes strictly after a cursor
    /// in ascending order — so the cursor counts each component DOWN from the
    /// largest the modelled `SemanticVersion` pattern admits, and ascending cursor
    /// order is descending version order.
    pub fn cursor(&self) -> String {
        let (major, minor, patch) = version_order(self.version);
        format!(
            "{:016x}{:016x}{:016x}",
            u64::MAX - major,
            u64::MAX - minor,
            u64::MAX - patch
        )
    }
}

/// One module in the catalog.
#[derive(Debug, Clone, Copy)]
pub struct AvailableModule {
    /// The modelled `ModuleId`: `module-` and alphanumerics.
    pub module_id: &'static str,
    /// A label, not unique.
    pub module_name: &'static str,
    pub description: Option<&'static str>,
    /// A member of the modelled `SubscriptionType`.
    pub subscription_type: &'static str,
    /// A member of the modelled `SubscriptionStatus`.
    pub subscription_status: &'static str,
    /// The Marketplace product the module is sold under, "present only when
    /// subscriptionType is MARKETPLACE".
    pub marketplace_product_id: Option<&'static str>,
    /// Seconds since the epoch at which the module was created.
    pub created_at: u64,
    /// Every version of this module the catalog holds.
    pub versions: &'static [AvailableModuleVersion],
}

impl AvailableModule {
    /// The instant this module was created.
    pub fn created_at(&self) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(self.created_at)
    }

    /// This module's ARN, as the modelled `ModuleArn` pattern spells it: see
    /// [`AvailableModuleVersion::arn`] for the fields.
    pub fn arn(&self, region: &str) -> String {
        format!(
            "arn:aws:{}:{}::modules/{}",
            super::ARN_VENDOR,
            super::region_dns_label(region),
            self.module_id
        )
    }

    /// The versions of this module a read of the catalog reports, highest version
    /// first, counted numerically rather than lexically — so `10.1.0` is a later
    /// version than `2.0.0`.
    pub fn listed_versions(&self) -> Vec<&'static AvailableModuleVersion> {
        let mut listed: Vec<&'static AvailableModuleVersion> =
            self.versions.iter().filter(|it| it.listed()).collect();
        listed.sort_unstable_by_key(|version| {
            let (major, minor, patch) = version_order(version.version);
            (
                ::std::cmp::Reverse(major),
                ::std::cmp::Reverse(minor),
                ::std::cmp::Reverse(patch),
            )
        });
        listed
    }

    /// The version `version` of this module, if the catalog reports it: a version
    /// in a status the readers do not report resolves to nothing here, exactly as
    /// one the catalog does not hold does.
    pub fn listed_version(&self, version: &str) -> Option<&'static AvailableModuleVersion> {
        self.versions
            .iter()
            .find(|it| it.version == version && it.listed())
    }

    /// The cursor `ListAvailableModules` pages this module by.
    ///
    /// That listing reports the newest module first, while the shared page walk
    /// resumes strictly after a cursor in ascending order — so the cursor counts
    /// the creation instant DOWN, and the module's own id follows it to make the
    /// order total when two modules were created at the same instant.
    pub fn cursor(&self) -> String {
        format!(
            "{:016x}:{}",
            u64::MAX - self.created_at,
            self.module_id
        )
    }
}

/// The three numeric components of a `SemanticVersion`, for comparing two
/// versions numerically. The modelled pattern admits exactly three decimal
/// components, each at most ten digits, so a component that cannot be read is
/// zero rather than a refusal.
fn version_order(version: &str) -> (u64, u64, u64) {
    let mut components = version.split('.').map(|part| part.parse().unwrap_or(0));
    (
        components.next().unwrap_or(0),
        components.next().unwrap_or(0),
        components.next().unwrap_or(0),
    )
}

/// `2024-01-01T00:00:00Z`, the instant the oldest catalog entry carries. The
/// entries below are dated from it in whole quarters, so no two share a
/// `createdAt` and the order a listing reports them in is fixed.
const CATALOG_EPOCH: u64 = 1_704_067_200;
/// A quarter of a year in seconds, the step between two catalog instants.
const QUARTER: u64 = 91 * 24 * 60 * 60;

/// The JSON Schema the filtering built-in's current version takes its parameters
/// under.
const FILTER_SCHEMA: &str = concat!(
    r#"{"type":"object","properties":{"#,
    r#""blockedDomains":{"type":"array","items":{"type":"string"}}},"#,
    r#""additionalProperties":false}"#
);

/// The modules this emulator's catalog holds: one built-in per member of the
/// modelled `ConfigurableAction`, each with the permissions that member
/// documents as its minimum, and each with the versions a reader reports.
pub static AVAILABLE_MODULES: &[AvailableModule] = &[
    AvailableModule {
        module_id: "module-filter",
        module_name: "Filter",
        description: Some("Drops a bid request that matches the flow's filter parameters."),
        subscription_type: SUBSCRIPTION_TYPE_AWS_BUILT_IN,
        subscription_status: SUBSCRIPTION_STATUS_ACTIVE,
        marketplace_product_id: None,
        created_at: CATALOG_EPOCH,
        versions: &[
            AvailableModuleVersion {
                version: "1.0.0",
                status: MODULE_VERSION_STATUS_DEPRECATED,
                permissions: &["READ_REQUEST_BODY", "FILTER"],
                configurable_actions: &["FILTER"],
                configuration_schema: None,
                release_notes: Some("The first release of the filtering built-in."),
                created_at: CATALOG_EPOCH,
                updated_at: CATALOG_EPOCH + 2 * QUARTER,
            },
            AvailableModuleVersion {
                version: "2.0.0",
                status: MODULE_VERSION_STATUS_ACTIVE,
                permissions: &["READ_REQUEST_BODY", "FILTER"],
                configurable_actions: &["FILTER"],
                configuration_schema: Some(FILTER_SCHEMA),
                release_notes: None,
                created_at: CATALOG_EPOCH + QUARTER,
                updated_at: CATALOG_EPOCH + QUARTER,
            },
            AvailableModuleVersion {
                version: "10.1.0",
                status: MODULE_VERSION_STATUS_ACTIVE,
                permissions: &["READ_REQUEST_BODY", "READ_REQUEST_HEADERS", "FILTER"],
                configurable_actions: &["FILTER", "OBSERVE"],
                configuration_schema: Some(FILTER_SCHEMA),
                release_notes: Some("Filters on the bid request's headers as well as its body."),
                created_at: CATALOG_EPOCH + 3 * QUARTER,
                updated_at: CATALOG_EPOCH + 3 * QUARTER,
            },
        ],
    },
    AvailableModule {
        module_id: "module-enrichext",
        module_name: "Enrich-Request-Ext",
        description: Some("Writes the flow's parameters into the bid request's ext object."),
        subscription_type: SUBSCRIPTION_TYPE_AWS_BUILT_IN,
        subscription_status: SUBSCRIPTION_STATUS_ACTIVE,
        marketplace_product_id: None,
        created_at: CATALOG_EPOCH + QUARTER,
        versions: &[
            AvailableModuleVersion {
                version: "1.0.0",
                status: MODULE_VERSION_STATUS_ACTIVE,
                permissions: &["READ_REQUEST_BODY", "ENRICH_REQUEST_EXT"],
                configurable_actions: &["ENRICH_REQUEST_EXT"],
                configuration_schema: None,
                release_notes: None,
                created_at: CATALOG_EPOCH + QUARTER,
                updated_at: CATALOG_EPOCH + QUARTER,
            },
            AvailableModuleVersion {
                version: "1.2.0",
                status: MODULE_VERSION_STATUS_ACTIVE,
                permissions: &["READ_REQUEST_BODY", "ENRICH_REQUEST_EXT"],
                configurable_actions: &["ENRICH_REQUEST_EXT", "OBSERVE"],
                configuration_schema: None,
                release_notes: None,
                created_at: CATALOG_EPOCH + 2 * QUARTER,
                updated_at: CATALOG_EPOCH + 2 * QUARTER,
            },
        ],
    },
    AvailableModule {
        module_id: "module-enrichheaders",
        module_name: "Enrich-Request-Headers",
        description: Some("Adds the flow's parameters to the bid request as headers."),
        subscription_type: SUBSCRIPTION_TYPE_AWS_BUILT_IN,
        subscription_status: SUBSCRIPTION_STATUS_ACTIVE,
        marketplace_product_id: None,
        created_at: CATALOG_EPOCH + 2 * QUARTER,
        versions: &[AvailableModuleVersion {
            version: "1.0.0",
            status: MODULE_VERSION_STATUS_ACTIVE,
            permissions: &["READ_REQUEST_HEADERS", "ENRICH_REQUEST_HEADERS"],
            configurable_actions: &["ENRICH_REQUEST_HEADERS"],
            configuration_schema: None,
            release_notes: None,
            created_at: CATALOG_EPOCH + 2 * QUARTER,
            updated_at: CATALOG_EPOCH + 2 * QUARTER,
        }],
    },
    AvailableModule {
        module_id: "module-observe",
        module_name: "Observe",
        description: Some("Reports on the bid requests a link carries, changing none of them."),
        subscription_type: SUBSCRIPTION_TYPE_AWS_BUILT_IN,
        subscription_status: SUBSCRIPTION_STATUS_ACTIVE,
        marketplace_product_id: None,
        created_at: CATALOG_EPOCH + 3 * QUARTER,
        versions: &[
            AvailableModuleVersion {
                version: "1.0.0",
                status: MODULE_VERSION_STATUS_DEPRECATED,
                permissions: &["READ_REQUEST_BODY"],
                configurable_actions: &["OBSERVE"],
                configuration_schema: None,
                release_notes: None,
                created_at: CATALOG_EPOCH + 3 * QUARTER,
                updated_at: CATALOG_EPOCH + 4 * QUARTER,
            },
            AvailableModuleVersion {
                version: "2.0.0",
                status: MODULE_VERSION_STATUS_ACTIVE,
                permissions: &["READ_REQUEST_BODY", "READ_REQUEST_HEADERS"],
                configurable_actions: &["OBSERVE"],
                configuration_schema: None,
                release_notes: Some("Reports on the bid request's headers as well as its body."),
                created_at: CATALOG_EPOCH + 4 * QUARTER,
                updated_at: CATALOG_EPOCH + 4 * QUARTER,
            },
        ],
    },
];

/// The catalog's modules a read reports, newest first by `createdAt`.
pub fn listed_modules() -> Vec<&'static AvailableModule> {
    let mut listed: Vec<&'static AvailableModule> = AVAILABLE_MODULES.iter().collect();
    listed.sort_unstable_by_key(|module| {
        (
            ::std::cmp::Reverse(module.created_at),
            module.module_id,
        )
    });
    listed
}

/// The catalog module `module_id` names, or `None` when the catalog holds none.
pub fn find_available_module(module_id: &str) -> Option<&'static AvailableModule> {
    AVAILABLE_MODULES
        .iter()
        .find(|module| module.module_id == module_id)
}

#[cfg(test)]
mod tests {
    use super::{
        find_available_module, listed_modules, version_order, AVAILABLE_MODULES,
        MODULE_VERSION_STATUS_ACTIVE, MODULE_VERSION_STATUS_DEPRECATED,
        SUBSCRIPTION_STATUS_ACTIVE, SUBSCRIPTION_TYPE_AWS_BUILT_IN,
    };

    /// Every entry of the catalog satisfies the model's own constraints on the
    /// members it fills, and every module resolves through its own reader.
    #[test]
    fn every_catalog_entry_satisfies_the_model() {
        assert!(
            !AVAILABLE_MODULES.is_empty(),
            "expected the catalog to hold at least one module"
        );
        for module in AVAILABLE_MODULES {
            let case = module.module_id;
            let suffix = case
                .strip_prefix("module-")
                .unwrap_or_else(|| panic!("case {case:?}: a moduleId starts with module-"));
            assert!(
                (1..=25).contains(&suffix.len())
                    && suffix.chars().all(|c| c.is_ascii_alphanumeric()),
                "case {case:?}: the modelled ModuleId pattern admits module- and 1..25 alphanumerics"
            );
            assert!(
                (1..=64).contains(&module.module_name.len()),
                "case {case:?}: the modelled ModuleName length is 1..64, got {:?}",
                module.module_name
            );
            assert_eq!(
                module.subscription_type, SUBSCRIPTION_TYPE_AWS_BUILT_IN,
                "case {case:?}: this emulator's catalog holds built-ins alone"
            );
            assert_eq!(
                module.subscription_status, SUBSCRIPTION_STATUS_ACTIVE,
                "case {case:?}: a built-in's access is current"
            );
            assert!(
                module.marketplace_product_id.is_none(),
                "case {case:?}: a marketplaceProductId belongs to a MARKETPLACE module alone"
            );
            assert!(
                find_available_module(module.module_id).is_some(),
                "case {case:?}: a listed module resolves through its own reader"
            );
            assert!(
                !module.listed_versions().is_empty(),
                "case {case:?}: a listed module has at least one listed version"
            );
            for version in module.versions {
                let case = format!("{case}@{}", version.version);
                assert_eq!(
                    version_order(version.version).0.to_string(),
                    version
                        .version
                        .split('.')
                        .next()
                        .unwrap_or_default()
                        .to_string(),
                    "case {case:?}: a SemanticVersion is three numeric components"
                );
                assert!(
                    version.status == MODULE_VERSION_STATUS_ACTIVE
                        || version.status == MODULE_VERSION_STATUS_DEPRECATED,
                    "case {case:?}: the catalog holds attachable and deprecated versions, got {:?}",
                    version.status
                );
                assert!(
                    (1..=16).contains(&version.permissions.len()),
                    "case {case:?}: the modelled ModulePermissionList length is 1..16"
                );
                assert!(
                    (1..=16).contains(&version.configurable_actions.len()),
                    "case {case:?}: the modelled ConfigurableActionList length is 1..16"
                );
                assert!(
                    version.updated_at >= version.created_at,
                    "case {case:?}: a version cannot have changed before it was created"
                );
                assert!(
                    module
                        .listed_version(version.version)
                        .is_some_and(|found| found.version == version.version),
                    "case {case:?}: a listed version resolves through its own reader"
                );
            }
        }
    }

    /// A listing reports the newest module first and the highest version first,
    /// counted numerically: the cursors the shared page walk takes are in the same
    /// order as the records they index.
    #[test]
    fn the_catalog_is_ordered_newest_and_highest_first() {
        let modules = listed_modules();
        for pair in modules.windows(2) {
            let (earlier, later) = (pair[0], pair[1]);
            assert!(
                earlier.created_at > later.created_at,
                "expected {:?} (created {}) before {:?} (created {})",
                earlier.module_id,
                earlier.created_at,
                later.module_id,
                later.created_at
            );
            assert!(
                earlier.cursor() < later.cursor(),
                "expected the cursor of {:?} to sort before that of {:?}",
                earlier.module_id,
                later.module_id
            );
        }
        for module in &modules {
            let versions = module.listed_versions();
            for pair in versions.windows(2) {
                let (earlier, later) = (pair[0], pair[1]);
                assert!(
                    version_order(earlier.version) > version_order(later.version),
                    "module {:?}: expected {:?} before {:?}",
                    module.module_id,
                    earlier.version,
                    later.version
                );
                assert!(
                    earlier.cursor() < later.cursor(),
                    "module {:?}: expected the cursor of {:?} to sort before that of {:?}",
                    module.module_id,
                    earlier.version,
                    later.version
                );
            }
        }
    }
}
