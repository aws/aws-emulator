// SPDX-License-Identifier: CC0-1.0

//! The published managed-database catalogs: the blueprints a database engine is
//! chosen from, the bundles it runs on, and the runtime parameters each engine
//! publishes.

use super::*;

/// One entry of the managed-database blueprint catalog: a database engine and one
/// MAJOR version of it a database can be created from.
///
/// "A blueprint describes the major engine version of a database" (the modelled
/// `RelationalDatabaseBlueprint` documentation), while
/// `RelationalDatabaseBlueprint$engineVersion` carries the full version "(for
/// example, `5.7.23`)" — so a blueprint's id names the major version and its
/// `engineVersion` names the patch level that major version is served at.
#[derive(Debug, Clone, Copy)]
pub struct RelationalDatabaseBlueprintSpec {
    /// The database software this blueprint installs.
    ///
    /// The modelled `RelationalDatabaseEngine` enum
    /// (`smithy_query(query="shape",shape="RelationalDatabaseEngine")`) still
    /// carries the single value `mysql`, which the service has outgrown:
    /// Lightsail also serves PostgreSQL managed databases, and the recorded
    /// `GetRelationalDatabaseBlueprints` answer reports `postgres` blueprints
    /// beside the MySQL ones. The catalog reports the engines the service
    /// serves — the member is the engine of the blueprint, not of the enum's
    /// last revision.
    pub engine: &'static str,
    /// How the engine is written in prose, as the engine descriptions show it
    /// (`MySQL`).
    pub engine_display: &'static str,
    /// `RelationalDatabaseBlueprint$engineDescription`: the edition of the engine
    /// the blueprint installs.
    pub engine_description: &'static str,
    /// The major engine version this blueprint describes (`8.0`).
    pub major_version: &'static str,
    /// The full engine version the major version is served at (`8.0.41`).
    pub engine_version: &'static str,
    /// The revision label the engine's releases carry in their version
    /// description, or empty where that engine's releases carry none.
    ///
    /// A managed PostgreSQL release is published as a REVISION of its community
    /// version (`R1` being the first revision of that version), which is what
    /// the recorded `engineVersionDescription` of every PostgreSQL blueprint
    /// ends with; a MySQL release carries no such label, and the recorded MySQL
    /// blueprint's description ends at its version. So the label belongs to the
    /// ENGINE rather than to the description, and
    /// [`RelationalDatabaseBlueprintSpec::view`] appends it when the engine has
    /// one.
    pub engine_version_revision: &'static str,
    /// Whether this is the engine's default version — true for exactly one
    /// blueprint per engine.
    pub is_engine_default: bool,
}

/// The database blueprints this service serves: the MySQL blueprint first, then
/// the PostgreSQL ones by ascending major version — the order the recorded
/// `GetRelationalDatabaseBlueprints` answer reports them in, whose first entry is
/// `mysql_8_4`.
///
/// Source: the engines and major versions the Lightsail user guide's "Database
/// engines in Amazon Lightsail" lists for a managed database — MySQL 8.4 and
/// PostgreSQL 13 through 18 — each pinned to the patch release of that series the
/// service serves it at (`mysql_8_4` at the recorded `8.4.11`; each PostgreSQL
/// series at the latest community patch release of that series). A major version
/// the service retires or adds is a row removed or added here, and the default
/// flag moves with the engine's default version — the series the service creates
/// a database at when the caller names no other, which for PostgreSQL is the
/// newest series that is not the opt-in latest.
///
/// Both engines are served even though the modelled `RelationalDatabaseEngine`
/// enum still names only `mysql`: see [`RelationalDatabaseBlueprintSpec::engine`].
pub const RELATIONAL_DATABASE_BLUEPRINTS: &[RelationalDatabaseBlueprintSpec] = &[
    RelationalDatabaseBlueprintSpec {
        engine: "mysql",
        engine_display: "MySQL",
        engine_description: "MySQL Community Edition",
        major_version: "8.4",
        engine_version: "8.4.11",
        engine_version_revision: "",
        is_engine_default: true,
    },
    RelationalDatabaseBlueprintSpec {
        engine: "postgres",
        engine_display: "PostgreSQL",
        engine_description: "PostgreSQL",
        major_version: "13",
        engine_version: "13.23-rds.20260514",
        engine_version_revision: POSTGRES_ENGINE_VERSION_REVISION,
        is_engine_default: false,
    },
    RelationalDatabaseBlueprintSpec {
        engine: "postgres",
        engine_display: "PostgreSQL",
        engine_description: "PostgreSQL",
        major_version: "14",
        engine_version: "14.24",
        engine_version_revision: POSTGRES_ENGINE_VERSION_REVISION,
        is_engine_default: false,
    },
    RelationalDatabaseBlueprintSpec {
        engine: "postgres",
        engine_display: "PostgreSQL",
        engine_description: "PostgreSQL",
        major_version: "15",
        engine_version: "15.19",
        engine_version_revision: POSTGRES_ENGINE_VERSION_REVISION,
        is_engine_default: false,
    },
    RelationalDatabaseBlueprintSpec {
        engine: "postgres",
        engine_display: "PostgreSQL",
        engine_description: "PostgreSQL",
        major_version: "16",
        engine_version: "16.15",
        engine_version_revision: POSTGRES_ENGINE_VERSION_REVISION,
        is_engine_default: false,
    },
    RelationalDatabaseBlueprintSpec {
        engine: "postgres",
        engine_display: "PostgreSQL",
        engine_description: "PostgreSQL",
        major_version: "17",
        engine_version: "17.11",
        engine_version_revision: POSTGRES_ENGINE_VERSION_REVISION,
        is_engine_default: false,
    },
    RelationalDatabaseBlueprintSpec {
        engine: "postgres",
        engine_display: "PostgreSQL",
        engine_description: "PostgreSQL",
        major_version: "18",
        engine_version: "18.6",
        engine_version_revision: POSTGRES_ENGINE_VERSION_REVISION,
        is_engine_default: true,
    },
];

/// The revision label every managed PostgreSQL release this service serves is
/// published at: the first revision of its community version. See
/// [`RelationalDatabaseBlueprintSpec::engine_version_revision`].
pub(super) const POSTGRES_ENGINE_VERSION_REVISION: &str = "R1";

/// How many blueprints one `GetRelationalDatabaseBlueprints` page carries.
///
/// CHOSEN: Lightsail publishes no page size for this operation, only that the
/// response carries a `nextPageToken` when more blueprints remain. It is above the
/// catalog's size, so the whole catalog is answered in one page — as AWS answers
/// it — and the paging walk still holds for a catalog that outgrows it.
pub const RELATIONAL_DATABASE_BLUEPRINT_PAGE_SIZE: usize = 10;

impl RelationalDatabaseBlueprintSpec {
    /// This blueprint's id: the engine and its major version, joined the way every
    /// Lightsail id token is (`mysql` + `8.0` -> `mysql_8_0`).
    pub fn blueprint_id(&self) -> String {
        format!("{}_{}", self.engine, self.major_version.replace('.', "_"))
    }

    /// This blueprint's major version as the NUMBERS it is written with (`8.4`
    /// -> `[8, 4]`), so two major versions of one engine order by version rather
    /// than by the text they are spelled in — where `13` would sort before
    /// `8.4`. A component that is not a number counts as zero, so an entry added
    /// to the catalog in any other form still orders rather than panicking.
    pub(super) fn major_version_numbers(&self) -> Vec<u32> {
        self.major_version
            .split('.')
            .map(|component| component.parse().unwrap_or(0))
            .collect()
    }

    /// Whether this blueprint describes a LATER major version of the engine than
    /// `other` does: a database's major version moves forward (`8.0` -> `8.4`),
    /// which is the direction `UpdateRelationalDatabase$relationalDatabaseBlueprintId`
    /// updates it in.
    pub fn is_newer_than(&self, other: &Self) -> bool {
        self.major_version_numbers() > other.major_version_numbers()
    }

    /// `RelationalDatabaseBlueprint$engineVersionDescription`: the engine named
    /// beside the version it is served at, followed by that release's revision
    /// label where the engine's releases carry one (`PostgreSQL 13.23-R1`,
    /// `MySQL 8.4.11`). See [`Self::engine_version_revision`].
    pub fn engine_version_description(&self) -> String {
        let version = &self.engine_version;
        let engine = &self.engine_display;
        if self.engine_version_revision.is_empty() {
            format!("{engine} {version}")
        } else {
            format!("{engine} {version}-{}", self.engine_version_revision)
        }
    }

    /// This entry as the modelled `RelationalDatabaseBlueprint`.
    pub fn view(&self) -> crate::ops::types::RelationalDatabaseBlueprint {
        crate::ops::types::RelationalDatabaseBlueprint {
            blueprint_id: Some(self.blueprint_id()),
            engine: Some(self.engine.to_string()),
            engine_version: Some(self.engine_version.to_string()),
            engine_description: Some(self.engine_description.to_string()),
            // `engineVersionDescription` describes the version the way the
            // engine itself is named beside it ("MySQL 5.7.44"), followed by the
            // revision label the engine's releases carry, where it has one
            // ([`Self::engine_version_revision`]).
            engine_version_description: Some(self.engine_version_description()),
            is_engine_default: Some(self.is_engine_default),
        }
    }
}

/// The blueprint `blueprint_id` names, or `None` when the catalog has no such
/// blueprint.
pub fn relational_database_blueprint(
    blueprint_id: &str,
) -> Option<RelationalDatabaseBlueprintSpec> {
    RELATIONAL_DATABASE_BLUEPRINTS
        .iter()
        .find(|spec| spec.blueprint_id() == blueprint_id)
        .copied()
}

/// The blueprint ids [`relational_database_blueprint`] accepts, for a refusal that
/// has to tell the caller what to send instead.
pub fn relational_database_blueprint_ids() -> String {
    RELATIONAL_DATABASE_BLUEPRINTS
        .iter()
        .map(RelationalDatabaseBlueprintSpec::blueprint_id)
        .collect::<Vec<_>>()
        .join(", ")
}

/// One page of `reported`: at most `page_size` database blueprints from the entry
/// `page_token` names onward, and the id of the first entry of the page after it
/// — present only while entries remain.
///
/// `None` is the answer when the token names no blueprint of `reported`: there is
/// no page to report, which the caller turns into a refusal. An absent token asks
/// for the first page.
///
/// The walk itself is [`catalog_page`], shared with this service's other paged
/// catalogs; only the member a token names is this catalog's own, and it is
/// DERIVED from the entry rather than held by it, which is what that walk's `Cow`
/// answer is for.
pub fn relational_database_blueprint_page(
    reported: &[RelationalDatabaseBlueprintSpec],
    page_size: usize,
    page_token: Option<&str>,
) -> Option<(Vec<RelationalDatabaseBlueprintSpec>, Option<String>)> {
    catalog_page(reported, page_size, page_token, |spec| {
        Some(std::borrow::Cow::Owned(spec.blueprint_id()))
    })
}

// ---------------------------------------------------------------------------
// Relational database bundles
// ---------------------------------------------------------------------------

// Like the blueprint catalog above, this is PUBLISHED data rather than
// per-account state: the managed-database plan list, identical for every
// account, which the reporting operation over it answers from. No call changes
// it, so reading it leaves every later caller the answer it would have had.

/// One entry of the managed-database bundle catalog: "A bundle describes the
/// performance specifications of the database" (the modelled
/// `RelationalDatabaseBundle` documentation) — the hardware, the included
/// monthly data transfer and the monthly price a database created on that plan
/// runs with.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RelationalDatabaseBundleSpec {
    /// `RelationalDatabaseBundle$bundleId` — "The ID for the database bundle".
    ///
    /// A database bundle id is `<size>_<major>_<minor>`, the `<size>_<version>`
    /// form every Lightsail bundle id carries, with an `ha` token after the size
    /// on the high-availability plans (`micro_ha_1_0`).
    pub bundle_id: &'static str,
    /// `RelationalDatabaseBundle$name` — "The name for the database bundle": the
    /// label Lightsail gives the plan.
    pub name: &'static str,
    /// `RelationalDatabaseBundle$cpuCount` — "The number of virtual CPUs
    /// (vCPUs) for the database bundle."
    pub cpu_count: i32,
    /// `RelationalDatabaseBundle$ramSizeInGb` — "The amount of RAM in GB (for
    /// example, `2.0`) for the database bundle."
    pub ram_size_in_gb: f32,
    /// `RelationalDatabaseBundle$diskSizeInGb` — "The size of the disk for the
    /// database bundle."
    pub disk_size_in_gb: i32,
    /// `RelationalDatabaseBundle$transferPerMonthInGb` — "The data transfer rate
    /// per month in GB for the database bundle."
    pub transfer_per_month_in_gb: i32,
    /// `RelationalDatabaseBundle$price` — "The cost of the database bundle in US
    /// currency", per month.
    pub price_usd_per_month: f32,
    /// `RelationalDatabaseBundle$isEncrypted` — "A Boolean value indicating
    /// whether the database bundle is encrypted."
    pub is_encrypted: bool,
    /// `RelationalDatabaseBundle$isActive` — "A Boolean value indicating whether
    /// the database bundle is active." A plan the service has retired stays in
    /// the catalog as inactive: a database already running on it must still
    /// resolve, and `GetRelationalDatabaseBundles` reports it only when asked to
    /// include inactive bundles.
    pub is_active: bool,
}

/// Every managed-database plan this service publishes: the standard
/// (single-database) plans of the generation it serves, cheapest first, then
/// their high-availability twins, then the plans of the generation it has
/// retired.
///
/// SOURCE: the managed-database plan table of the Lightsail pricing page — a
/// standard plan at $15 a month with 1 GB RAM, 2 vCPUs and 40 GB of SSD storage,
/// $30 with 2 GB / 2 vCPUs / 80 GB, $60 with 4 GB / 2 vCPUs / 120 GB, $115 with
/// 8 GB / 2 vCPUs / 240 GB, $245 with 16 GB / 4 vCPUs / 480 GB and $490 with
/// 32 GB / 8 vCPUs / 960 GB, with 100 GB of data transfer included on the three
/// smallest plans, 200 GB on the 8 GB plan, 300 GB on the 16 GB one and 400 GB
/// on the largest. Each price is the TABLE's own figure for that plan rather
/// than a step computed from the plan below it: the table doubles up to the 4 GB
/// plan and then charges $115, $245 and $490, so a price derived by doubling
/// would understate every plan from the 8 GB one up. Beside the standard column
/// stands the high-availability column of the same table, which offers the same
/// hardware with a standby in a second Availability Zone for twice the standard
/// plan's price. Lightsail encrypts managed-database storage and its automatic
/// snapshots at rest on every plan, so every entry is encrypted.
///
/// The GENERATION is a property of the catalog, as it is for an instance plan:
/// the plans published today are the `2_0` generation, and the superseded `1_0`
/// generation stays here as INACTIVE — a database created on it still resolves
/// its hardware, and `GetRelationalDatabaseBundles` reports it only when asked
/// to include inactive bundles.
///
/// The ORDER is this slice's own (cheapest first, standard plans before
/// high-availability ones, the published generation before the retired one): it
/// is what the paged walk reports, and it is stable so two reads answer
/// identically.
pub const RELATIONAL_DATABASE_BUNDLE_SPECS: &[RelationalDatabaseBundleSpec] = &[
    RelationalDatabaseBundleSpec {
        bundle_id: "micro_2_0",
        name: "Micro",
        cpu_count: 2,
        ram_size_in_gb: 1.0,
        disk_size_in_gb: 40,
        transfer_per_month_in_gb: 100,
        price_usd_per_month: 15.0,
        is_encrypted: true,
        is_active: true,
    },
    RelationalDatabaseBundleSpec {
        bundle_id: "small_2_0",
        name: "Small",
        cpu_count: 2,
        ram_size_in_gb: 2.0,
        disk_size_in_gb: 80,
        transfer_per_month_in_gb: 100,
        price_usd_per_month: 30.0,
        is_encrypted: true,
        is_active: true,
    },
    RelationalDatabaseBundleSpec {
        bundle_id: "medium_2_0",
        name: "Medium",
        cpu_count: 2,
        ram_size_in_gb: 4.0,
        disk_size_in_gb: 120,
        transfer_per_month_in_gb: 100,
        price_usd_per_month: 60.0,
        is_encrypted: true,
        is_active: true,
    },
    RelationalDatabaseBundleSpec {
        bundle_id: "large_2_0",
        name: "Large",
        cpu_count: 2,
        ram_size_in_gb: 8.0,
        disk_size_in_gb: 240,
        transfer_per_month_in_gb: 200,
        price_usd_per_month: 115.0,
        is_encrypted: true,
        is_active: true,
    },
    RelationalDatabaseBundleSpec {
        bundle_id: "xlarge_2_0",
        name: "Xlarge",
        cpu_count: 4,
        ram_size_in_gb: 16.0,
        disk_size_in_gb: 480,
        transfer_per_month_in_gb: 300,
        price_usd_per_month: 245.0,
        is_encrypted: true,
        is_active: true,
    },
    RelationalDatabaseBundleSpec {
        bundle_id: "2xlarge_2_0",
        name: "2Xlarge",
        cpu_count: 8,
        ram_size_in_gb: 32.0,
        disk_size_in_gb: 960,
        transfer_per_month_in_gb: 400,
        price_usd_per_month: 490.0,
        is_encrypted: true,
        is_active: true,
    },
    RelationalDatabaseBundleSpec {
        bundle_id: "micro_ha_2_0",
        name: "Micro with High Availability",
        cpu_count: 2,
        ram_size_in_gb: 1.0,
        disk_size_in_gb: 40,
        transfer_per_month_in_gb: 100,
        price_usd_per_month: 30.0,
        is_encrypted: true,
        is_active: true,
    },
    RelationalDatabaseBundleSpec {
        bundle_id: "small_ha_2_0",
        name: "Small with High Availability",
        cpu_count: 2,
        ram_size_in_gb: 2.0,
        disk_size_in_gb: 80,
        transfer_per_month_in_gb: 100,
        price_usd_per_month: 60.0,
        is_encrypted: true,
        is_active: true,
    },
    RelationalDatabaseBundleSpec {
        bundle_id: "medium_ha_2_0",
        name: "Medium with High Availability",
        cpu_count: 2,
        ram_size_in_gb: 4.0,
        disk_size_in_gb: 120,
        transfer_per_month_in_gb: 100,
        price_usd_per_month: 120.0,
        is_encrypted: true,
        is_active: true,
    },
    RelationalDatabaseBundleSpec {
        bundle_id: "large_ha_2_0",
        name: "Large with High Availability",
        cpu_count: 2,
        ram_size_in_gb: 8.0,
        disk_size_in_gb: 240,
        transfer_per_month_in_gb: 200,
        price_usd_per_month: 230.0,
        is_encrypted: true,
        is_active: true,
    },
    RelationalDatabaseBundleSpec {
        bundle_id: "xlarge_ha_2_0",
        name: "Xlarge with High Availability",
        cpu_count: 4,
        ram_size_in_gb: 16.0,
        disk_size_in_gb: 480,
        transfer_per_month_in_gb: 300,
        price_usd_per_month: 490.0,
        is_encrypted: true,
        is_active: true,
    },
    RelationalDatabaseBundleSpec {
        bundle_id: "2xlarge_ha_2_0",
        name: "2Xlarge with High Availability",
        cpu_count: 8,
        ram_size_in_gb: 32.0,
        disk_size_in_gb: 960,
        transfer_per_month_in_gb: 400,
        price_usd_per_month: 980.0,
        is_encrypted: true,
        is_active: true,
    },
    // The superseded first generation: still resolvable for a database created
    // on it, and reported only when a call asks for inactive bundles.
    RelationalDatabaseBundleSpec {
        bundle_id: "micro_1_0",
        name: "Micro",
        cpu_count: 1,
        ram_size_in_gb: 1.0,
        disk_size_in_gb: 40,
        transfer_per_month_in_gb: 100,
        price_usd_per_month: 15.0,
        is_encrypted: true,
        is_active: false,
    },
    RelationalDatabaseBundleSpec {
        bundle_id: "small_1_0",
        name: "Small",
        cpu_count: 1,
        ram_size_in_gb: 2.0,
        disk_size_in_gb: 80,
        transfer_per_month_in_gb: 100,
        price_usd_per_month: 30.0,
        is_encrypted: true,
        is_active: false,
    },
    RelationalDatabaseBundleSpec {
        bundle_id: "medium_1_0",
        name: "Medium",
        cpu_count: 2,
        ram_size_in_gb: 4.0,
        disk_size_in_gb: 120,
        transfer_per_month_in_gb: 100,
        price_usd_per_month: 60.0,
        is_encrypted: true,
        is_active: false,
    },
    RelationalDatabaseBundleSpec {
        bundle_id: "large_1_0",
        name: "Large",
        cpu_count: 2,
        ram_size_in_gb: 8.0,
        disk_size_in_gb: 240,
        transfer_per_month_in_gb: 200,
        price_usd_per_month: 115.0,
        is_encrypted: true,
        is_active: false,
    },
    RelationalDatabaseBundleSpec {
        bundle_id: "micro_ha_1_0",
        name: "Micro with High Availability",
        cpu_count: 1,
        ram_size_in_gb: 1.0,
        disk_size_in_gb: 40,
        transfer_per_month_in_gb: 100,
        price_usd_per_month: 30.0,
        is_encrypted: true,
        is_active: false,
    },
    RelationalDatabaseBundleSpec {
        bundle_id: "small_ha_1_0",
        name: "Small with High Availability",
        cpu_count: 1,
        ram_size_in_gb: 2.0,
        disk_size_in_gb: 80,
        transfer_per_month_in_gb: 100,
        price_usd_per_month: 60.0,
        is_encrypted: true,
        is_active: false,
    },
    RelationalDatabaseBundleSpec {
        bundle_id: "medium_ha_1_0",
        name: "Medium with High Availability",
        cpu_count: 2,
        ram_size_in_gb: 4.0,
        disk_size_in_gb: 120,
        transfer_per_month_in_gb: 100,
        price_usd_per_month: 120.0,
        is_encrypted: true,
        is_active: false,
    },
    RelationalDatabaseBundleSpec {
        bundle_id: "large_ha_1_0",
        name: "Large with High Availability",
        cpu_count: 2,
        ram_size_in_gb: 8.0,
        disk_size_in_gb: 240,
        transfer_per_month_in_gb: 200,
        price_usd_per_month: 230.0,
        is_encrypted: true,
        is_active: false,
    },
];

/// How many database bundles one `GetRelationalDatabaseBundles` page carries.
///
/// CHOSEN: Lightsail publishes no page size for this operation, only that the
/// response carries a `nextPageToken` when more bundles remain. It is above the
/// catalog's size, so the whole catalog is answered in one page — as AWS answers
/// it — and the paging walk still holds for a catalog that outgrows it.
pub const RELATIONAL_DATABASE_BUNDLE_PAGE_SIZE: usize = 20;

impl RelationalDatabaseBundleSpec {
    /// Whether this plan is a high-availability one: a database created on it
    /// runs with a standby in a second Availability Zone.
    ///
    /// From the id's own form, documented on [`Self::bundle_id`]: a
    /// high-availability plan carries an `ha` token after the size
    /// (`micro_ha_1_0`), which the standard plan of the same size does not.
    pub fn is_high_availability(&self) -> bool {
        self.bundle_id.contains("_ha_")
    }

    /// Whether this plan is SMALLER than `other` — any of the three hardware
    /// dimensions the catalog publishes (vCPU count, RAM, disk size) below
    /// `other`'s.
    ///
    /// A restore is held to this: "When creating a new database from a snapshot,
    /// you cannot choose a bundle that is smaller than the bundle of the source
    /// database" (the modelled `CreateRelationalDatabaseFromSnapshot$relationalDatabaseBundleId`
    /// documentation). The comparison is over the published HARDWARE rather than
    /// over the catalog's order, so a plan the catalog gains sits in the ordering
    /// by what it offers; two plans with the same hardware (a standard plan and
    /// its high-availability twin) are neither smaller than the other, which is
    /// what makes a restore onto a standby-backed plan of the same size legal.
    pub fn is_smaller_than(&self, other: &Self) -> bool {
        self.cpu_count < other.cpu_count
            || self.ram_size_in_gb < other.ram_size_in_gb
            || self.disk_size_in_gb < other.disk_size_in_gb
    }

    /// This plan as the modelled `RelationalDatabaseBundle`.
    pub fn view(&self) -> RelationalDatabaseBundle {
        RelationalDatabaseBundle {
            bundle_id: Some(self.bundle_id.to_string()),
            name: Some(self.name.to_string()),
            price: Some(self.price_usd_per_month),
            ram_size_in_gb: Some(self.ram_size_in_gb),
            disk_size_in_gb: Some(self.disk_size_in_gb),
            transfer_per_month_in_gb: Some(self.transfer_per_month_in_gb),
            cpu_count: Some(self.cpu_count),
            is_encrypted: Some(self.is_encrypted),
            is_active: Some(self.is_active),
        }
    }
}

/// The bundles a `GetRelationalDatabaseBundles` call reports out of `specs`,
/// before paging: every active plan, plus the inactive (retired) ones when
/// `include_inactive` is set.
///
/// The order is `specs`' own. Taking the catalog as an argument is what lets the
/// slice rule be exercised against a catalog that holds a retired plan, which the
/// published one does not yet.
///
/// Unlike its bucket counterpart [`bucket_bundles_from`], this answers the SPECS
/// rather than the modelled view: this operation is paged, and a page token names
/// an entry by its `bundle_id`, so [`relational_database_bundle_page`] slices the
/// specs first and calls [`RelationalDatabaseBundleSpec::view`] only on the
/// entries one page carries.
pub fn relational_database_bundles_from(
    specs: &[RelationalDatabaseBundleSpec],
    include_inactive: bool,
) -> Vec<RelationalDatabaseBundleSpec> {
    specs
        .iter()
        .filter(|spec| include_inactive || spec.is_active)
        .copied()
        .collect()
}

/// The database bundles a `GetRelationalDatabaseBundles` call reports:
/// [`relational_database_bundles_from`] over this service's published catalog,
/// [`RELATIONAL_DATABASE_BUNDLE_SPECS`].
pub fn relational_database_bundles(include_inactive: bool) -> Vec<RelationalDatabaseBundleSpec> {
    relational_database_bundles_from(RELATIONAL_DATABASE_BUNDLE_SPECS, include_inactive)
}

/// One page of the database bundle catalog: the bundles it carries and the token
/// that opens the page after it.
#[derive(Debug, Clone)]
pub struct RelationalDatabaseBundlePage {
    /// The page's bundles, as the modelled view.
    pub bundles: Vec<RelationalDatabaseBundle>,
    /// The id of the first bundle of the NEXT page, present only while bundles
    /// remain.
    pub next_page_token: Option<String>,
}

/// One page of the slice of `specs` that `include_inactive` selects: the slice
/// from the entry `page_token` names onward, at most `page_size` entries, with
/// `next_page_token` naming the first entry of the page after it.
///
/// `None` when `page_token` names no entry of that slice — the caller refuses it
/// with the modelled `InvalidInputException`. A token names an ENTRY rather than
/// an index, so a catalog that gained or lost a plan between two pages cannot
/// make the walk skip or repeat another one. An absent — or empty — token asks
/// for the first page.
pub fn relational_database_bundle_page(
    specs: &[RelationalDatabaseBundleSpec],
    include_inactive: bool,
    page_token: Option<&str>,
    page_size: usize,
) -> Option<RelationalDatabaseBundlePage> {
    // A page carries at least one entry: a zero-length page would name its own
    // first entry as the next token and never advance.
    let page_size = page_size.max(1);
    let slice = relational_database_bundles_from(specs, include_inactive);

    let start = match page_token.filter(|token| !token.is_empty()) {
        None => 0,
        Some(token) => slice.iter().position(|spec| spec.bundle_id == token)?,
    };

    let mut bundles = Vec::new();
    let mut next_page_token = None;
    for spec in &slice[start..] {
        if bundles.len() == page_size {
            next_page_token = Some(spec.bundle_id.to_string());
            break;
        }
        bundles.push(spec.view());
    }

    Some(RelationalDatabaseBundlePage {
        bundles,
        next_page_token,
    })
}

/// The database bundle `bundle_id` names — active or not, since a database
/// already running on a retired plan still has to resolve — or `None` when this
/// service publishes no such plan.
///
/// This is the lookup a call that NAMES a plan resolves it through:
/// `CreateRelationalDatabase`'s and `CreateRelationalDatabaseFromSnapshot`'s
/// `relationalDatabaseBundleId`.
pub fn relational_database_bundle_spec(bundle_id: &str) -> Option<RelationalDatabaseBundleSpec> {
    RELATIONAL_DATABASE_BUNDLE_SPECS
        .iter()
        .find(|spec| spec.bundle_id == bundle_id)
        .copied()
}

/// The bundle ids a `GetRelationalDatabaseBundles` page token may name out of
/// `specs`, for a refusal that has to tell the caller what to send instead.
pub fn relational_database_bundle_ids(specs: &[RelationalDatabaseBundleSpec]) -> String {
    specs
        .iter()
        .map(|spec| spec.bundle_id)
        .collect::<Vec<_>>()
        .join(", ")
}

// ---------------------------------------------------------------------------
// Relational database runtime parameters
// ---------------------------------------------------------------------------

// `GetRelationalDatabaseParameters` "returns all of the runtime parameters
// offered by the underlying database software, or engine, for a specific
// database" — so the parameter SET is the engine's, published data like the
// blueprint and bundle catalogs above, while which set a database has and what
// the memory-derived values in it come to are the database's own stored facts
// (its blueprint's engine, its bundle's RAM). Nothing here is per-account state:
// reporting it leaves every later caller the answer it would have had.

/// The `applyType` of a parameter a running engine can take on without being
/// restarted, as RDS and Lightsail report it.
pub const PARAMETER_APPLY_TYPE_DYNAMIC: &str = "dynamic";

/// The `applyType` of a parameter the engine only reads at startup, so a change
/// to it takes effect at the next reboot.
pub const PARAMETER_APPLY_TYPE_STATIC: &str = "static";

/// The `applyMethod` of a parameter a change takes effect on at once: the
/// modelled `RelationalDatabaseParameter$applyMethod` is "Indicates when
/// parameter updates are applied. Can be `immediate` or `pending-reboot`", and a
/// [dynamic](PARAMETER_APPLY_TYPE_DYNAMIC) parameter is exactly one a change can
/// be applied to immediately.
pub const PARAMETER_APPLY_METHOD_IMMEDIATE: &str = "immediate";

/// The `applyMethod` of a parameter a change waits for the next reboot on: a
/// [static](PARAMETER_APPLY_TYPE_STATIC) parameter, which is what this
/// operation's own documentation means by "whether changes require a reboot".
pub const PARAMETER_APPLY_METHOD_PENDING_REBOOT: &str = "pending-reboot";

/// The `dataType` of a parameter that is on or off.
pub const PARAMETER_DATA_TYPE_BOOLEAN: &str = "boolean";

/// The `dataType` of a parameter that takes a whole number.
pub const PARAMETER_DATA_TYPE_INTEGER: &str = "integer";

/// The `dataType` of a parameter that takes a fractional number.
pub const PARAMETER_DATA_TYPE_FLOAT: &str = "float";

/// The `dataType` of a parameter that takes free text.
pub const PARAMETER_DATA_TYPE_STRING: &str = "string";

/// Bytes in a gibibyte: the unit a bundle's `ramSizeInGb` is measured in, which
/// the memory-derived parameter values below are computed against.
pub(super) const BYTES_PER_GIB: f64 = 1024.0 * 1024.0 * 1024.0;

/// The memory, in bytes, of a database running on a bundle with `ram_size_in_gb`
/// gibibytes of RAM.
pub(super) fn relational_database_memory_bytes(ram_size_in_gb: f32) -> i64 {
    (f64::from(ram_size_in_gb) * BYTES_PER_GIB).round() as i64
}

/// What a runtime parameter's CURRENT value is on a database of a given size.
///
/// A managed database runs with the service's own parameter group, and a
/// parameter in it is in one of exactly these three positions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelationalDatabaseParameterValue {
    /// The group pins nothing: the parameter runs at the database engine's own
    /// default. RDS reports such a parameter with no `ParameterValue` at all —
    /// its source is the engine, not the group — and so does Lightsail: the
    /// `get-relational-database-parameters` example in the AWS CLI reference
    /// shows entries carrying every other member and no `parameterValue`.
    EngineDefault,
    /// The group pins a fixed value, the same on every database.
    Fixed(&'static str),
    /// The group derives the value from the memory of the database's own
    /// hardware: `min(memory_bytes * numerator / denominator, cap)`.
    ///
    /// This is how RDS's default parameter groups express a memory-sized
    /// parameter — as a formula over `DBInstanceClassMemory` rather than as a
    /// number — and the formula is published per parameter (see the entries that
    /// use it below). The formula's literal text is an RDS implementation detail
    /// of its parameter-group format; what a database actually RUNS with is the
    /// formula resolved against its own hardware, so that resolved number is
    /// what this emulator reports, computed from the bundle the database is
    /// stored with rather than from any canned body.
    FromMemory {
        /// The factor the memory is multiplied by.
        numerator: i64,
        /// The divisor the product is divided by — the unit of the parameter
        /// (bytes, 8 kB pages, bytes per connection).
        denominator: i64,
        /// The ceiling the formula is capped at, where the published formula
        /// carries one.
        cap: Option<i64>,
    },
}

impl RelationalDatabaseParameterValue {
    /// This value on a database whose bundle carries `ram_size_in_gb` gibibytes
    /// of RAM, or `None` when the parameter is left at the engine's own default
    /// and so reports no value.
    pub fn resolve(self, ram_size_in_gb: f32) -> Option<String> {
        match self {
            Self::EngineDefault => None,
            Self::Fixed(value) => Some(value.to_string()),
            Self::FromMemory {
                numerator,
                denominator,
                cap,
            } => {
                let memory = relational_database_memory_bytes(ram_size_in_gb);
                // A denominator of zero would be a malformed catalog entry, not
                // a request the caller can make; clamping it keeps the reporting
                // path total.
                let value = memory.saturating_mul(numerator) / denominator.max(1);
                Some(cap.map_or(value, |cap| value.min(cap)).to_string())
            }
        }
    }
}

/// One runtime parameter of a database engine: everything the modelled
/// `RelationalDatabaseParameter` reports about it except the value, which
/// depends on the database ([`RelationalDatabaseParameterValue`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RelationalDatabaseParameterSpec {
    /// `RelationalDatabaseParameter$parameterName` — "the name of the
    /// parameter", spelled as the engine itself spells it.
    pub parameter_name: &'static str,
    /// Where this parameter's current value comes from.
    pub value: RelationalDatabaseParameterValue,
    /// `RelationalDatabaseParameter$allowedValues` — "the valid range of values
    /// for the parameter". `None` where the parameter admits any value of its
    /// data type and the engine publishes no range, and then the member is left
    /// out rather than reported empty.
    pub allowed_values: Option<&'static str>,
    /// `RelationalDatabaseParameter$applyType` — "the engine-specific parameter
    /// type": [dynamic](PARAMETER_APPLY_TYPE_DYNAMIC) when the running engine
    /// can take a change on, [static](PARAMETER_APPLY_TYPE_STATIC) when it only
    /// reads the parameter at startup.
    pub apply_type: &'static str,
    /// `RelationalDatabaseParameter$dataType` — "the valid data type for the
    /// parameter", one of the `PARAMETER_DATA_TYPE_*` values.
    pub data_type: &'static str,
    /// `RelationalDatabaseParameter$description` — "a description of the
    /// parameter", from the engine's own documentation of it.
    pub description: &'static str,
    /// `RelationalDatabaseParameter$isModifiable` — "whether the parameter can
    /// be modified". False for a parameter the managed service owns and does not
    /// let the customer change.
    pub is_modifiable: bool,
}

impl RelationalDatabaseParameterSpec {
    /// When a change to this parameter takes effect.
    ///
    /// DERIVED from [`Self::apply_type`] rather than tracked beside it: a
    /// dynamic parameter is one the running engine can take a change on
    /// ([`PARAMETER_APPLY_METHOD_IMMEDIATE`]), and a static one is only read at
    /// startup, so a change to it can only land at the next reboot
    /// ([`PARAMETER_APPLY_METHOD_PENDING_REBOOT`]) — which is what this
    /// operation's "whether changes require a reboot" reports. Keeping the two
    /// apart would let them disagree.
    pub fn apply_method(&self) -> &'static str {
        if self.apply_type == PARAMETER_APPLY_TYPE_DYNAMIC {
            PARAMETER_APPLY_METHOD_IMMEDIATE
        } else {
            PARAMETER_APPLY_METHOD_PENDING_REBOOT
        }
    }

    /// This parameter as the modelled `RelationalDatabaseParameter`, on a
    /// database whose bundle carries `ram_size_in_gb` gibibytes of RAM.
    pub fn view(&self, ram_size_in_gb: f32) -> RelationalDatabaseParameter {
        RelationalDatabaseParameter {
            parameter_name: Some(self.parameter_name.to_string()),
            parameter_value: self.value.resolve(ram_size_in_gb),
            allowed_values: self.allowed_values.map(str::to_string),
            apply_method: Some(self.apply_method().to_string()),
            apply_type: Some(self.apply_type.to_string()),
            data_type: Some(self.data_type.to_string()),
            description: Some(self.description.to_string()),
            is_modifiable: Some(self.is_modifiable),
        }
    }
}

/// The runtime parameters of the MySQL engine, as a Lightsail MySQL database
/// reports them.
///
/// SOURCE: each entry's default, range and dynamic/static nature is the one the
/// MySQL manual's "Server System Variables" publishes for that variable, and the
/// description is that variable's own; the two memory-derived values and the
/// pinned time zone are what the managed service's parameter group sets on top
/// of the engine's defaults — RDS publishes `innodb_buffer_pool_size` as
/// `{DBInstanceClassMemory*3/4}` and `max_connections` as
/// `{DBInstanceClassMemory/12582880}` in its MySQL default parameter group, and
/// documents that a DB instance's time zone is UTC unless the customer changes
/// it. A parameter the group leaves alone reports no value
/// ([`RelationalDatabaseParameterValue::EngineDefault`]).
///
/// The ORDER is this slice's own — by parameter name, as RDS lists a parameter
/// group — and it is stable, so two reads answer identically and a page token
/// names the same entry in both.
pub const MYSQL_RELATIONAL_DATABASE_PARAMETERS: &[RelationalDatabaseParameterSpec] = &[
    RelationalDatabaseParameterSpec {
        parameter_name: "allow-suspicious-udfs",
        value: RelationalDatabaseParameterValue::EngineDefault,
        allowed_values: Some("0,1"),
        apply_type: PARAMETER_APPLY_TYPE_STATIC,
        data_type: PARAMETER_DATA_TYPE_BOOLEAN,
        description: "Controls whether user-defined functions that have only an xxx symbol for the main function can be loaded",
        // The managed service loads no such function and does not let the
        // customer ask for one.
        is_modifiable: false,
    },
    RelationalDatabaseParameterSpec {
        parameter_name: "auto_increment_increment",
        value: RelationalDatabaseParameterValue::EngineDefault,
        allowed_values: Some("1-65535"),
        apply_type: PARAMETER_APPLY_TYPE_DYNAMIC,
        data_type: PARAMETER_DATA_TYPE_INTEGER,
        description: "Controls the increment between successive AUTO_INCREMENT values",
        is_modifiable: true,
    },
    RelationalDatabaseParameterSpec {
        parameter_name: "autocommit",
        value: RelationalDatabaseParameterValue::EngineDefault,
        allowed_values: Some("0,1"),
        apply_type: PARAMETER_APPLY_TYPE_DYNAMIC,
        data_type: PARAMETER_DATA_TYPE_BOOLEAN,
        description: "Sets the autocommit mode, in which every statement is committed as a transaction of its own",
        is_modifiable: true,
    },
    RelationalDatabaseParameterSpec {
        parameter_name: "innodb_buffer_pool_size",
        // RDS: `{DBInstanceClassMemory*3/4}` — three quarters of the hardware's
        // memory, in bytes.
        value: RelationalDatabaseParameterValue::FromMemory {
            numerator: 3,
            denominator: 4,
            cap: None,
        },
        allowed_values: Some("5242880-9223372036854775807"),
        apply_type: PARAMETER_APPLY_TYPE_DYNAMIC,
        data_type: PARAMETER_DATA_TYPE_INTEGER,
        description: "The size in bytes of the buffer pool, the memory area where InnoDB caches table and index data",
        is_modifiable: true,
    },
    RelationalDatabaseParameterSpec {
        parameter_name: "long_query_time",
        value: RelationalDatabaseParameterValue::EngineDefault,
        allowed_values: Some("0-31536000"),
        apply_type: PARAMETER_APPLY_TYPE_DYNAMIC,
        data_type: PARAMETER_DATA_TYPE_FLOAT,
        description: "The minimum execution time in seconds for a statement to be logged as slow",
        is_modifiable: true,
    },
    RelationalDatabaseParameterSpec {
        parameter_name: "max_allowed_packet",
        value: RelationalDatabaseParameterValue::EngineDefault,
        allowed_values: Some("1024-1073741824"),
        apply_type: PARAMETER_APPLY_TYPE_DYNAMIC,
        data_type: PARAMETER_DATA_TYPE_INTEGER,
        description: "The maximum size in bytes of one packet, or of any generated or intermediate string",
        is_modifiable: true,
    },
    RelationalDatabaseParameterSpec {
        parameter_name: "max_connections",
        // RDS: `{DBInstanceClassMemory/12582880}` — one connection per 12 MB of
        // the hardware's memory.
        value: RelationalDatabaseParameterValue::FromMemory {
            numerator: 1,
            denominator: 12_582_880,
            cap: None,
        },
        allowed_values: Some("1-100000"),
        apply_type: PARAMETER_APPLY_TYPE_DYNAMIC,
        data_type: PARAMETER_DATA_TYPE_INTEGER,
        description: "The maximum permitted number of simultaneous client connections",
        is_modifiable: true,
    },
    RelationalDatabaseParameterSpec {
        parameter_name: "slow_query_log",
        value: RelationalDatabaseParameterValue::EngineDefault,
        allowed_values: Some("0,1"),
        apply_type: PARAMETER_APPLY_TYPE_DYNAMIC,
        data_type: PARAMETER_DATA_TYPE_BOOLEAN,
        description: "Enables or disables the slow query log",
        is_modifiable: true,
    },
    RelationalDatabaseParameterSpec {
        parameter_name: "time_zone",
        // The managed service runs a database in UTC unless the customer
        // changes it.
        value: RelationalDatabaseParameterValue::Fixed("UTC"),
        // A time zone name, not a range: the engine publishes none.
        allowed_values: None,
        apply_type: PARAMETER_APPLY_TYPE_DYNAMIC,
        data_type: PARAMETER_DATA_TYPE_STRING,
        description: "The time zone the server interprets and reports date and time values in",
        is_modifiable: true,
    },
    RelationalDatabaseParameterSpec {
        parameter_name: "wait_timeout",
        value: RelationalDatabaseParameterValue::EngineDefault,
        allowed_values: Some("1-31536000"),
        apply_type: PARAMETER_APPLY_TYPE_DYNAMIC,
        data_type: PARAMETER_DATA_TYPE_INTEGER,
        description: "The number of seconds the server waits for activity on a noninteractive connection before closing it",
        is_modifiable: true,
    },
];

/// The runtime parameters of the PostgreSQL engine, as a Lightsail PostgreSQL
/// database reports them.
///
/// SOURCE: each entry's range, unit and reset-required (static) nature is the
/// one the PostgreSQL documentation's "Server Configuration" publishes for that
/// parameter, and the description is that parameter's own; the two
/// memory-derived values and the pinned time zone are what the managed service's
/// parameter group sets on top of them — RDS publishes `shared_buffers` as
/// `{DBInstanceClassMemory/32768}` (its unit is an 8 kB page, so that is a
/// quarter of the hardware's memory) and `max_connections` as
/// `LEAST({DBInstanceClassMemory/9531392},5000)` in its PostgreSQL default
/// parameter group, and documents that a DB instance's time zone is UTC unless
/// the customer changes it.
///
/// The ORDER is this slice's own, by parameter name, for the same reason as the
/// MySQL catalog's.
pub const POSTGRES_RELATIONAL_DATABASE_PARAMETERS: &[RelationalDatabaseParameterSpec] = &[
    RelationalDatabaseParameterSpec {
        parameter_name: "autovacuum",
        value: RelationalDatabaseParameterValue::EngineDefault,
        allowed_values: Some("0,1"),
        apply_type: PARAMETER_APPLY_TYPE_DYNAMIC,
        data_type: PARAMETER_DATA_TYPE_BOOLEAN,
        description: "Controls whether the server runs the autovacuum launcher daemon",
        is_modifiable: true,
    },
    RelationalDatabaseParameterSpec {
        parameter_name: "client_encoding",
        value: RelationalDatabaseParameterValue::EngineDefault,
        // A character set name, not a range: the parameter admits every
        // encoding the server was built with.
        allowed_values: None,
        apply_type: PARAMETER_APPLY_TYPE_DYNAMIC,
        data_type: PARAMETER_DATA_TYPE_STRING,
        description: "Sets the client-side character set encoding",
        is_modifiable: true,
    },
    RelationalDatabaseParameterSpec {
        parameter_name: "log_min_duration_statement",
        value: RelationalDatabaseParameterValue::EngineDefault,
        allowed_values: Some("-1-2147483647"),
        apply_type: PARAMETER_APPLY_TYPE_DYNAMIC,
        data_type: PARAMETER_DATA_TYPE_INTEGER,
        description: "Sets the minimum execution time in milliseconds above which a statement is logged; -1 disables the logging",
        is_modifiable: true,
    },
    RelationalDatabaseParameterSpec {
        parameter_name: "maintenance_work_mem",
        value: RelationalDatabaseParameterValue::EngineDefault,
        allowed_values: Some("1024-2147483647"),
        apply_type: PARAMETER_APPLY_TYPE_DYNAMIC,
        data_type: PARAMETER_DATA_TYPE_INTEGER,
        description: "Sets the maximum memory in kilobytes a maintenance operation such as VACUUM or CREATE INDEX may use",
        is_modifiable: true,
    },
    RelationalDatabaseParameterSpec {
        parameter_name: "max_connections",
        // RDS: `LEAST({DBInstanceClassMemory/9531392},5000)` — one connection
        // per about 9 MB of the hardware's memory, never more than 5000.
        value: RelationalDatabaseParameterValue::FromMemory {
            numerator: 1,
            denominator: 9_531_392,
            cap: Some(5000),
        },
        allowed_values: Some("1-262143"),
        // PostgreSQL reads `max_connections` only at startup.
        apply_type: PARAMETER_APPLY_TYPE_STATIC,
        data_type: PARAMETER_DATA_TYPE_INTEGER,
        description: "Sets the maximum number of concurrent connections to the database server",
        is_modifiable: true,
    },
    RelationalDatabaseParameterSpec {
        parameter_name: "shared_buffers",
        // RDS: `{DBInstanceClassMemory/32768}`, in the parameter's own unit of
        // 8 kB pages.
        value: RelationalDatabaseParameterValue::FromMemory {
            numerator: 1,
            denominator: 32_768,
            cap: None,
        },
        allowed_values: Some("16-1073741823"),
        apply_type: PARAMETER_APPLY_TYPE_STATIC,
        data_type: PARAMETER_DATA_TYPE_INTEGER,
        description: "Sets the number of 8 kB shared memory buffers the server uses",
        is_modifiable: true,
    },
    RelationalDatabaseParameterSpec {
        parameter_name: "statement_timeout",
        value: RelationalDatabaseParameterValue::EngineDefault,
        allowed_values: Some("0-2147483647"),
        apply_type: PARAMETER_APPLY_TYPE_DYNAMIC,
        data_type: PARAMETER_DATA_TYPE_INTEGER,
        description: "Sets the maximum allowed duration in milliseconds of any statement; 0 disables the timeout",
        is_modifiable: true,
    },
    RelationalDatabaseParameterSpec {
        parameter_name: "timezone",
        value: RelationalDatabaseParameterValue::Fixed("UTC"),
        allowed_values: None,
        apply_type: PARAMETER_APPLY_TYPE_DYNAMIC,
        data_type: PARAMETER_DATA_TYPE_STRING,
        description: "Sets the time zone the server displays and interprets time stamps in",
        is_modifiable: true,
    },
    RelationalDatabaseParameterSpec {
        parameter_name: "work_mem",
        value: RelationalDatabaseParameterValue::EngineDefault,
        allowed_values: Some("64-2147483647"),
        apply_type: PARAMETER_APPLY_TYPE_DYNAMIC,
        data_type: PARAMETER_DATA_TYPE_INTEGER,
        description: "Sets the maximum memory in kilobytes a query operation such as a sort or a hash table may use before writing to disk",
        is_modifiable: true,
    },
];

/// How many parameters one `GetRelationalDatabaseParameters` page carries.
///
/// CHOSEN: Lightsail publishes no page size for this operation, only that the
/// response carries a `nextPageToken` when more parameters remain. It is above
/// either engine's parameter set, so a database's whole set is answered in one
/// page, and the paging walk still holds for a set that outgrows it.
pub const RELATIONAL_DATABASE_PARAMETER_PAGE_SIZE: usize = 50;

/// The runtime parameters the database engine `engine` offers: the catalog of
/// the engine the database's blueprint names.
///
/// The engine is matched the way [`relational_database_port`] matches it — on
/// the engine FAMILY — so a blueprint of any PostgreSQL major version resolves
/// to the PostgreSQL set and everything else to the MySQL one, which is the
/// engine the modelled `RelationalDatabaseEngine` enum names.
pub fn relational_database_parameter_specs(
    engine: &str,
) -> &'static [RelationalDatabaseParameterSpec] {
    if is_postgres_engine(engine) {
        POSTGRES_RELATIONAL_DATABASE_PARAMETERS
    } else {
        MYSQL_RELATIONAL_DATABASE_PARAMETERS
    }
}

/// One page of a database's runtime parameters: the parameters it carries and
/// the token that opens the page after it.
#[derive(Debug, Clone)]
pub struct RelationalDatabaseParameterPage {
    /// The page's parameters, as the modelled view.
    pub parameters: Vec<RelationalDatabaseParameter>,
    /// The name of the first parameter of the NEXT page, present only while
    /// parameters remain.
    pub next_page_token: Option<String>,
}

/// One page of `specs` as a database with `ram_size_in_gb` gibibytes of RAM
/// reports them: the slice from the parameter `page_token` names onward, at most
/// `page_size` entries, with `next_page_token` naming the first parameter of the
/// page after it.
///
/// `None` when `page_token` names no parameter of the set — the caller refuses it
/// with the modelled `InvalidInputException`. A token names a PARAMETER rather
/// than an index, so a set that gained or lost an entry between two pages cannot
/// make the walk skip or repeat another one. An absent — or empty — token asks
/// for the first page.
pub fn relational_database_parameter_page(
    specs: &[RelationalDatabaseParameterSpec],
    ram_size_in_gb: f32,
    page_token: Option<&str>,
    page_size: usize,
) -> Option<RelationalDatabaseParameterPage> {
    // A page carries at least one entry: a zero-length page would name its own
    // first entry as the next token and never advance.
    let page_size = page_size.max(1);

    let start = match page_token.filter(|token| !token.is_empty()) {
        None => 0,
        Some(token) => specs
            .iter()
            .position(|spec| spec.parameter_name == token)?,
    };

    let mut parameters = Vec::new();
    let mut next_page_token = None;
    for spec in &specs[start..] {
        if parameters.len() == page_size {
            next_page_token = Some(spec.parameter_name.to_string());
            break;
        }
        parameters.push(spec.view(ram_size_in_gb));
    }

    Some(RelationalDatabaseParameterPage {
        parameters,
        next_page_token,
    })
}

/// The parameter names a `GetRelationalDatabaseParameters` page token may name
/// out of `specs`, for a refusal that has to tell the caller what to send
/// instead.
pub fn relational_database_parameter_names(specs: &[RelationalDatabaseParameterSpec]) -> String {
    specs
        .iter()
        .map(|spec| spec.parameter_name)
        .collect::<Vec<_>>()
        .join(", ")
}

/// The parameter `parameter_name` names in the set the engine `engine` offers,
/// or `None` when that engine offers no such parameter — a parameter of ANOTHER
/// engine included, since a database has only its own engine's set.
pub fn relational_database_parameter_spec(
    engine: &str,
    parameter_name: &str,
) -> Option<RelationalDatabaseParameterSpec> {
    relational_database_parameter_specs(engine)
        .iter()
        .find(|spec| spec.parameter_name == parameter_name)
        .copied()
}

/// The most parameters one `UpdateRelationalDatabaseParameters` request may
/// carry.
///
/// A Lightsail managed database is an RDS DB instance and this call is RDS's
/// `ModifyDBParameterGroup`, which documents "A maximum of 20 parameters can be
/// modified in a single request" — so this is AWS's own bound on the
/// caller-sized list, not an emulator guard.
pub const MAX_RELATIONAL_DATABASE_PARAMETER_UPDATES: usize = 20;

/// The `parameterApplyStatus` of a database holding a parameter value the
/// RUNNING engine has not loaded yet, beside
/// [`PARAMETER_APPLY_STATUS_IN_SYNC`].
///
/// A Lightsail managed database is an RDS DB instance and the member carries
/// RDS's own parameter group status. `RebootRelationalDatabase` is what clears
/// it: "Parameters marked with a pending-reboot apply type are applied only
/// after the database is rebooted using the reboot relational database
/// operation" (the modelled `UpdateRelationalDatabaseParameters`
/// documentation).
pub const PARAMETER_APPLY_STATUS_PENDING_REBOOT: &str = "pending-reboot";

/// A parameter value a database's parameter group carries because an
/// `UpdateRelationalDatabaseParameters` call set it.
///
/// Only a parameter of the database's own engine set can be set, so the map of
/// these a database holds is bounded by that set however many updates a caller
/// makes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredParameterOverride {
    /// The value the update set, which every later read of the parameter
    /// reports: the modification reaches the parameter group at once.
    pub value: String,
    /// The apply method the update was made with, which the parameter reports
    /// as its `applyMethod` from then on.
    pub apply_method: &'static str,
    /// Whether the RUNNING engine is still waiting for a reboot to load this
    /// value — true for a value applied `pending-reboot`. It is what puts the
    /// database in [`PARAMETER_APPLY_STATUS_PENDING_REBOOT`], and the engine
    /// coming back up from a `RebootRelationalDatabase` restart is what clears
    /// it ([`StoredRelationalDatabase::load_pending_parameters`]).
    pub pending_reboot: bool,
}

/// One resolved parameter modification: which parameter of the engine's set an
/// `UpdateRelationalDatabaseParameters` entry named, the value it sets and the
/// method it is applied with.
///
/// An update carries only these three: every other member of the modelled
/// `RelationalDatabaseParameter` DESCRIBES the parameter (its type, its allowed
/// values, its description, whether it is modifiable) and is the engine's to
/// report, so a request that spells one differently cannot change what the
/// service reports about the parameter.
#[derive(Debug, Clone)]
pub struct RelationalDatabaseParameterUpdate {
    /// The parameter the update names.
    pub spec: RelationalDatabaseParameterSpec,
    /// The value to set, already checked against that parameter.
    pub value: String,
    /// [`PARAMETER_APPLY_METHOD_IMMEDIATE`] or
    /// [`PARAMETER_APPLY_METHOD_PENDING_REBOOT`].
    pub apply_method: &'static str,
}

/// The modification `requested` makes to a parameter of `engine`, or a refusal
/// naming the member that is wrong and the constraint it broke.
///
/// `member` is how the request spells the entry under check (`parameters[0]`),
/// so a refusal points the caller at the entry it must fix.
pub fn relational_database_parameter_update(
    engine: &str,
    member: &str,
    requested: &RelationalDatabaseParameter,
) -> Result<RelationalDatabaseParameterUpdate, String> {
    let parameter_name = requested
        .parameter_name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .ok_or_else(|| {
            format!("{member}.parameterName is required: it names the parameter to update")
        })?;
    let spec = relational_database_parameter_spec(engine, parameter_name).ok_or_else(|| {
        format!(
            "{member}.parameterName '{parameter_name}' is not a parameter the {engine} engine \
             offers; use GetRelationalDatabaseParameters to list this database's parameters \
             ([{}])",
            relational_database_parameter_names(relational_database_parameter_specs(engine))
        )
    })?;
    if !spec.is_modifiable {
        return Err(format!(
            "{member}.parameterName '{parameter_name}' is not modifiable: the {engine} engine \
             reports isModifiable false for it"
        ));
    }

    let value = requested.parameter_value.as_deref().ok_or_else(|| {
        format!(
            "{member}.parameterValue is required: it carries the value to set on parameter \
             '{parameter_name}'"
        )
    })?;
    validate_relational_database_parameter_value(
        &spec,
        &format!("{member}.parameterValue"),
        value,
    )?;

    // "Indicates when parameter updates are applied. Can be `immediate` or
    // `pending-reboot`": an update that names neither is applied the way the
    // parameter's own apply type admits, and one that names `immediate` for a
    // parameter the engine reads only at startup is refused rather than
    // silently held back.
    let apply_method = match requested.apply_method.as_deref() {
        None => spec.apply_method(),
        Some(PARAMETER_APPLY_METHOD_PENDING_REBOOT) => PARAMETER_APPLY_METHOD_PENDING_REBOOT,
        Some(PARAMETER_APPLY_METHOD_IMMEDIATE) => {
            if spec.apply_type != PARAMETER_APPLY_TYPE_DYNAMIC {
                return Err(format!(
                    "{member}.applyMethod '{PARAMETER_APPLY_METHOD_IMMEDIATE}' cannot apply \
                     parameter '{parameter_name}', whose applyType is '{}': a change to it takes \
                     effect only when the database is rebooted, so apply it with \
                     '{PARAMETER_APPLY_METHOD_PENDING_REBOOT}'",
                    spec.apply_type
                ));
            }
            PARAMETER_APPLY_METHOD_IMMEDIATE
        }
        Some(other) => {
            return Err(format!(
                "{member}.applyMethod '{other}' is not an apply method: it must be \
                 '{PARAMETER_APPLY_METHOD_IMMEDIATE}' or \
                 '{PARAMETER_APPLY_METHOD_PENDING_REBOOT}'"
            ))
        }
    };

    Ok(RelationalDatabaseParameterUpdate {
        spec,
        value: value.to_string(),
        apply_method,
    })
}

/// `value` checked against `spec`'s own data type and published value set, or a
/// refusal naming the constraint it broke.
///
/// `member` is how the request spells the member carrying the value, so the
/// refusal points at it.
pub fn validate_relational_database_parameter_value(
    spec: &RelationalDatabaseParameterSpec,
    member: &str,
    value: &str,
) -> Result<(), String> {
    if value.is_empty() {
        return Err(format!(
            "{member} is empty: parameter '{}' takes a {} value",
            spec.parameter_name, spec.data_type
        ));
    }
    // A parameter's `dataType` is "the valid data type for the parameter", so a
    // value that is not of that type is refused before its range is looked at.
    let typed = match spec.data_type {
        PARAMETER_DATA_TYPE_INTEGER | PARAMETER_DATA_TYPE_BOOLEAN => {
            let number: i64 = value.parse().map_err(|_| {
                format!(
                    "{member} '{value}' is not a whole number: parameter '{}' has dataType {}",
                    spec.parameter_name, spec.data_type
                )
            })?;
            number as f64
        }
        PARAMETER_DATA_TYPE_FLOAT => value.parse::<f64>().map_err(|_| {
            format!(
                "{member} '{value}' is not a number: parameter '{}' has dataType {}",
                spec.parameter_name, spec.data_type
            )
        })?,
        // Free text: there is no type to fail, only the value set below.
        _ => f64::NAN,
    };

    // `allowedValues` is "the valid range of values for the parameter", absent
    // where the engine publishes none — and then any value of the data type is
    // accepted.
    let Some(allowed_values) = spec.allowed_values else {
        return Ok(());
    };
    let admitted = allowed_values.split(',').any(|allowed| {
        let allowed = allowed.trim();
        // An entry is either a value the parameter takes verbatim or a
        // `low-high` range a numeric value may fall inside.
        allowed == value
            || parse_allowed_range(allowed).is_some_and(|(low, high)| (low..=high).contains(&typed))
    });
    if admitted {
        return Ok(());
    }
    Err(format!(
        "{member} '{value}' is outside the allowedValues of parameter '{}' ({allowed_values})",
        spec.parameter_name
    ))
}

/// The `low-high` range `allowed` names, or `None` when it names a single value.
///
/// A lower bound may itself be negative (`-1-2147483647`), so the separator is
/// the first `-` that is not the sign at the front.
pub(super) fn parse_allowed_range(allowed: &str) -> Option<(f64, f64)> {
    let separator = allowed
        .char_indices()
        .skip(1)
        .find(|(_, character)| *character == '-')
        .map(|(index, _)| index)?;
    let low = allowed[..separator].parse::<f64>().ok()?;
    let high = allowed[separator + 1..].parse::<f64>().ok()?;
    Some((low, high))
}
