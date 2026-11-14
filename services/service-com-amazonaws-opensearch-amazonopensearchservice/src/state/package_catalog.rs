// SPDX-License-Identifier: CC0-1.0

//! The plugin packages the SERVICE publishes, which every account sees beside the
//! packages it created itself.
//!
//! `DescribePackages` "describes all packages available to Amazon OpenSearch Service"
//! — not "the packages this account created" — so the packages the service publishes
//! are part of the set every package READ answers from: a client discovers an
//! installable plugin by listing it here, reads its versions with
//! `GetPackageVersionHistory`, and then associates one with a domain.
//!
//! A published package is the SERVICE's resource, not the account's, so the rows below
//! are read-only reference data held as a table rather than inserted into the store:
//! every answer about one is [`PublishedPackage::record`] of the same row, which is
//! what makes two reads of a published package agree and makes the catalog identical
//! in every Region and on every call. Nothing here is written, so no read of it can
//! write.
//!
//! A ZIP plugin package carries ONE plugin built against ONE engine release, so the
//! catalog publishes a package per (plugin, release) pair — the pairs each row
//! declares — and derives its `PackageID` from both ([`published_package_id`]), since
//! `PackageID` is "the unique identifier of the package" and so is the service's to
//! mint rather than something a row lists.
//!
//! The ROW SET itself — which plugins the table carries, the releases each is
//! published for, the wording of a description, a plugin's class name and uncompressed
//! size, and the publication instants — is this emulator's own statement of published
//! content, exactly as the price table in [`crate::state`] is: AWS publishes no
//! machine-readable record of its package catalog, and the Smithy model constrains
//! only the shape of these answers. What the operations guarantee is the shape and the
//! resolution rules above, so a row may be added or reworded without changing any of
//! them.
//!
//! The SIBLING statement of the same content is
//! `service-com-amazonaws-elasticsearchservice-amazonelasticsearchservice2015`'s own
//! `state/package_catalog.rs`: the legacy `es` API version of this service publishes
//! the same plugin catalog through its own, smaller `PackageDetails` shape (no engine
//! version, no plugin properties, no package configuration), so the two tables are two
//! projections of one catalog and an edit to the plugins one of them names belongs in
//! both.

use super::*;

/// `PackageDetails$PackageOwner` for a plugin of the engine's own distribution: the
/// publisher of a package the SERVICE vends, which no account of this emulator created.
pub const PACKAGE_PUBLISHER_ENGINE: &str = "amazon";

/// `PackageDetails$PackageOwner` for the separately licensed third-party analyzer the
/// catalog carries: its publisher is the vendor, not the engine distribution.
pub const PACKAGE_PUBLISHER_BABEL_STREET: &str = "babel-street";

/// `RequirementLevel`'s `NONE`: the package needs nothing of this kind.
pub const REQUIREMENT_LEVEL_NONE: &str = "NONE";

/// `RequirementLevel`'s `OPTIONAL`: the package accepts one but runs without it.
pub const REQUIREMENT_LEVEL_OPTIONAL: &str = "OPTIONAL";

/// `RequirementLevel`'s `REQUIRED`: the package cannot be used without one.
pub const REQUIREMENT_LEVEL_REQUIRED: &str = "REQUIRED";

/// How long after a plugin's first publication the catalog publishes it for the NEXT
/// engine release it supports: a plugin is rebuilt for each release it is published
/// for, so the package of a later release was published later, and no two packages of
/// one plugin carry the same instants.
const RELEASE_PUBLICATION_SPACING: Duration = Duration::from_secs(120 * 24 * 60 * 60);

/// How long after one numbered version of a package's content the catalog publishes the
/// next, so the entries of one history carry ascending, distinct instants.
const REVISION_PUBLICATION_SPACING: Duration = Duration::from_secs(30 * 24 * 60 * 60);

/// Installing or removing a plugin replaces the nodes of the domain it runs on, so a
/// configuration update a plugin package is a prerequisite for needs a blue/green
/// deployment (`PackageConfiguration$RequiresRestartForConfigurationUpdate`).
const PLUGIN_REQUIRES_RESTART: bool = true;

/// One plugin of the published catalog: what it is, which engine releases the service
/// publishes it for, and when each of its content versions was published.
#[derive(Debug, Clone, Copy)]
pub struct PublishedPackage {
    /// `PackageDetails$PackageName` — the plugin's name in the engine distribution,
    /// which is what a ZIP plugin package installs. It satisfies that member's own
    /// constraints (`@length(3, 256)`, `@pattern("^[a-z][a-z0-9\\-]+$")`).
    pub name: &'static str,
    /// `PackageDetails$PackageDescription` and `PluginProperties$Description` — what
    /// the plugin does.
    pub description: &'static str,
    /// `PluginProperties$ClassName` — "the name of the class to load", the plugin's
    /// entry point in the engine.
    pub class_name: &'static str,
    /// `PluginProperties$UncompressedSizeInBytes` — how large the plugin's content is
    /// once unpacked.
    pub uncompressed_size_in_bytes: i64,
    /// `PackageDetails$PackageOwner` — who publishes the plugin.
    pub publisher: &'static str,
    /// `PackageConfiguration$LicenseFilepath` — the relative path of the license a
    /// separately licensed plugin needs, and `None` for one that needs no license of
    /// its own. Whether a license is REQUIRED is read off this, so the two cannot
    /// disagree.
    pub license_filepath: Option<&'static str>,
    /// `PackageConfiguration$ConfigurationRequirement` — whether the plugin needs
    /// configuration of its own, as a `RequirementLevel`.
    pub configuration_requirement: &'static str,
    /// The engine releases the service publishes this plugin for, oldest first. Each
    /// one is a package of its own, because a plugin is built against one release.
    /// Every entry is a release [`SUPPORTED_ENGINE_VERSIONS`] publishes, which
    /// [`catalog_publishes_published_releases`] holds.
    pub engine_versions: &'static [&'static str],
    /// The instant, in seconds since the epoch, the FIRST numbered version of this
    /// plugin's content was published for the FIRST release it is published for. Every
    /// other instant of this plugin's packages is that one plus a whole number of
    /// [`RELEASE_PUBLICATION_SPACING`] and [`REVISION_PUBLICATION_SPACING`] steps, so
    /// the stamps a read reports are stable across reads and across processes rather
    /// than taken from the clock of the read.
    pub first_published_at: u64,
    /// How many numbered versions of its content the catalog offers for this plugin:
    /// its version history is that many entries long, and the newest of them is the
    /// version an association would put on a domain.
    pub revisions: u32,
}

/// The plugins this service publishes, in name order.
///
/// Each row is named for the plugin it installs: the `analysis-*` tokenizers and token
/// filters a non-English index needs, the `mapper-*` field types, the ingest
/// processors, and the separately licensed third-party analyzer. The members every row
/// projects are `PackageDetails`'s and `PluginProperties`'s own:
///
/// ```text
/// smithy_query(query="shape",shape="PackageDetails,PluginProperties",model_path="services/service-com-amazonaws-opensearch-amazonopensearchservice/smithy-model/com-amazonaws-opensearch-amazonopensearchservice.json")
/// ```
pub const PUBLISHED_PACKAGES: &[PublishedPackage] = &[
    PublishedPackage {
        name: "analysis-icu",
        description: "ICU analysis plugin: Unicode-aware tokenization, normalization, collation \
                      and transliteration from the International Components for Unicode library.",
        class_name: "org.opensearch.plugin.analysis.icu.AnalysisICUPlugin",
        uncompressed_size_in_bytes: 13_421_772,
        publisher: PACKAGE_PUBLISHER_ENGINE,
        license_filepath: None,
        configuration_requirement: REQUIREMENT_LEVEL_NONE,
        engine_versions: &[
            "Elasticsearch_7.10",
            "OpenSearch_1.3",
            "OpenSearch_2.11",
            "OpenSearch_2.19",
            "OpenSearch_3.1",
        ],
        first_published_at: 1_600_000_000,
        revisions: 2,
    },
    PublishedPackage {
        name: "analysis-kuromoji",
        description: "Kuromoji analysis plugin: Japanese morphological tokenization, \
                      part-of-speech filtering and reading-form conversion.",
        class_name: "org.opensearch.plugin.analysis.kuromoji.AnalysisKuromojiPlugin",
        uncompressed_size_in_bytes: 4_404_019,
        publisher: PACKAGE_PUBLISHER_ENGINE,
        license_filepath: None,
        configuration_requirement: REQUIREMENT_LEVEL_NONE,
        engine_versions: &[
            "Elasticsearch_7.10",
            "OpenSearch_1.3",
            "OpenSearch_2.11",
            "OpenSearch_2.19",
            "OpenSearch_3.1",
        ],
        first_published_at: 1_602_000_000,
        revisions: 1,
    },
    PublishedPackage {
        name: "analysis-nori",
        description: "Nori analysis plugin: Korean morphological tokenization backed by the \
                      mecab-ko dictionary.",
        class_name: "org.opensearch.plugin.analysis.nori.AnalysisNoriPlugin",
        uncompressed_size_in_bytes: 22_020_096,
        publisher: PACKAGE_PUBLISHER_ENGINE,
        license_filepath: None,
        configuration_requirement: REQUIREMENT_LEVEL_NONE,
        engine_versions: &["OpenSearch_1.3", "OpenSearch_2.11", "OpenSearch_3.1"],
        first_published_at: 1_626_048_000,
        revisions: 2,
    },
    PublishedPackage {
        name: "analysis-phonetic",
        description: "Phonetic analysis plugin: Soundex, Metaphone, Caverphone and Beider-Morse \
                      token filters for matching names that sound alike.",
        class_name: "org.opensearch.plugin.analysis.phonetic.AnalysisPhoneticPlugin",
        uncompressed_size_in_bytes: 1_258_291,
        publisher: PACKAGE_PUBLISHER_ENGINE,
        license_filepath: None,
        configuration_requirement: REQUIREMENT_LEVEL_NONE,
        engine_versions: &[
            "Elasticsearch_7.10",
            "OpenSearch_1.3",
            "OpenSearch_2.11",
            "OpenSearch_3.1",
        ],
        first_published_at: 1_604_000_000,
        revisions: 2,
    },
    PublishedPackage {
        name: "analysis-seunjeon",
        description: "Seunjeon analysis plugin: Korean tokenization with a user-extensible \
                      dictionary.",
        class_name: "org.bitbucket.eunjeon.seunjeon.opensearch.SeunjeonPlugin",
        uncompressed_size_in_bytes: 31_457_280,
        publisher: PACKAGE_PUBLISHER_ENGINE,
        license_filepath: None,
        configuration_requirement: REQUIREMENT_LEVEL_OPTIONAL,
        engine_versions: &["Elasticsearch_7.10", "OpenSearch_1.3"],
        first_published_at: 1_606_000_000,
        revisions: 1,
    },
    PublishedPackage {
        name: "analysis-smartcn",
        description: "SmartCN analysis plugin: probabilistic sentence and word segmentation for \
                      Simplified Chinese.",
        class_name: "org.opensearch.plugin.analysis.smartcn.AnalysisSmartChinesePlugin",
        uncompressed_size_in_bytes: 6_291_456,
        publisher: PACKAGE_PUBLISHER_ENGINE,
        license_filepath: None,
        configuration_requirement: REQUIREMENT_LEVEL_NONE,
        engine_versions: &["OpenSearch_1.3", "OpenSearch_2.11", "OpenSearch_3.1"],
        first_published_at: 1_608_000_000,
        revisions: 1,
    },
    PublishedPackage {
        name: "analysis-stempel",
        description: "Stempel analysis plugin: algorithmic stemming for Polish.",
        class_name: "org.opensearch.plugin.analysis.stempel.AnalysisStempelPlugin",
        uncompressed_size_in_bytes: 1_048_576,
        publisher: PACKAGE_PUBLISHER_ENGINE,
        license_filepath: None,
        configuration_requirement: REQUIREMENT_LEVEL_NONE,
        engine_versions: &["OpenSearch_1.3", "OpenSearch_2.19"],
        first_published_at: 1_610_000_000,
        revisions: 1,
    },
    PublishedPackage {
        name: "analysis-ukrainian",
        description: "Ukrainian analysis plugin: lemmatization through the Morfologik dictionary.",
        class_name: "org.opensearch.plugin.analysis.ukrainian.AnalysisUkrainianPlugin",
        uncompressed_size_in_bytes: 3_145_728,
        publisher: PACKAGE_PUBLISHER_ENGINE,
        license_filepath: None,
        configuration_requirement: REQUIREMENT_LEVEL_NONE,
        engine_versions: &["OpenSearch_2.11", "OpenSearch_3.1"],
        first_published_at: 1_626_048_000,
        revisions: 1,
    },
    PublishedPackage {
        name: "babel-street-match",
        description: "Babel Street Match analysis plugin: multilingual name matching and entity \
                      extraction, licensed separately from the service.",
        class_name: "com.babelstreet.match.opensearch.MatchPlugin",
        uncompressed_size_in_bytes: 268_435_456,
        publisher: PACKAGE_PUBLISHER_BABEL_STREET,
        license_filepath: Some("license/babel-street-match.lic"),
        configuration_requirement: REQUIREMENT_LEVEL_REQUIRED,
        engine_versions: &["OpenSearch_2.11", "OpenSearch_2.19", "OpenSearch_3.1"],
        first_published_at: 1_700_000_000,
        revisions: 2,
    },
    PublishedPackage {
        name: "ingest-attachment",
        description: "Ingest attachment processor: extracts text and metadata from binary \
                      attachments with Apache Tika.",
        class_name: "org.opensearch.ingest.attachment.IngestAttachmentPlugin",
        uncompressed_size_in_bytes: 41_943_040,
        publisher: PACKAGE_PUBLISHER_ENGINE,
        license_filepath: None,
        configuration_requirement: REQUIREMENT_LEVEL_OPTIONAL,
        engine_versions: &[
            "Elasticsearch_7.10",
            "OpenSearch_1.3",
            "OpenSearch_2.11",
            "OpenSearch_2.19",
            "OpenSearch_3.1",
        ],
        first_published_at: 1_650_000_000,
        revisions: 2,
    },
    PublishedPackage {
        name: "mapper-annotated-text",
        description: "Annotated-text mapper plugin: a text field type that carries inline entity \
                      annotations alongside the text it indexes.",
        class_name: "org.opensearch.plugin.mapper.AnnotatedTextPlugin",
        uncompressed_size_in_bytes: 524_288,
        publisher: PACKAGE_PUBLISHER_ENGINE,
        license_filepath: None,
        configuration_requirement: REQUIREMENT_LEVEL_NONE,
        engine_versions: &["OpenSearch_2.11", "OpenSearch_3.1"],
        first_published_at: 1_672_531_200,
        revisions: 1,
    },
    PublishedPackage {
        name: "mapper-murmur3",
        description: "Murmur3 mapper plugin: stores the hash of a field's values at index time, \
                      for cheaper cardinality aggregations.",
        class_name: "org.opensearch.plugin.mapper.MapperMurmur3Plugin",
        uncompressed_size_in_bytes: 262_144,
        publisher: PACKAGE_PUBLISHER_ENGINE,
        license_filepath: None,
        configuration_requirement: REQUIREMENT_LEVEL_NONE,
        engine_versions: &["Elasticsearch_7.10", "OpenSearch_1.3", "OpenSearch_2.11"],
        first_published_at: 1_612_000_000,
        revisions: 1,
    },
    PublishedPackage {
        name: "mapper-size",
        description: "Size mapper plugin: indexes the byte size of each document's _source in \
                      the _size field.",
        class_name: "org.opensearch.plugin.mapper.MapperSizePlugin",
        uncompressed_size_in_bytes: 196_608,
        publisher: PACKAGE_PUBLISHER_ENGINE,
        license_filepath: None,
        configuration_requirement: REQUIREMENT_LEVEL_NONE,
        engine_versions: &["Elasticsearch_7.10", "OpenSearch_1.3", "OpenSearch_2.11"],
        first_published_at: 1_614_000_000,
        revisions: 1,
    },
];

/// The id the package of one plugin and one engine release is addressed by.
///
/// `PackageID` is `^([FG][0-9]+)$|^(pkg-[a-f0-9]+)$`, so what matters is that the id is
/// opaque, stable and that no two packages share one: it is the leading 40 hex digits
/// of a SHA-256 over the plugin's name and the release it is built for, behind the
/// prefix naming the resource it identifies. Deriving it from what the package IS —
/// rather than from a counter or a fresh UUID — is what makes a published package
/// resolvable by the same id in every Region and in every run, and the digest's length
/// keeps these ids apart from the ones [`new_package_id`] mints for a caller's own
/// packages.
pub fn published_package_id(name: &str, engine_version: &str) -> String {
    use ::sha2::Digest;
    let digest = ::sha2::Sha256::digest(format!("{name}/{engine_version}").as_bytes());
    let hex = ::hex::encode(digest);
    format!("pkg-{token}", token = &hex[..40])
}

impl PublishedPackage {
    /// Which numbered version of this plugin's content, for `release_index`, was
    /// published when: version `revision` counting from [`FIRST_PACKAGE_VERSION`].
    fn published_at(&self, release_index: u32, revision: u32) -> SystemTime {
        let first = SystemTime::UNIX_EPOCH + Duration::from_secs(self.first_published_at);
        first
            + RELEASE_PUBLICATION_SPACING * release_index
            + REVISION_PUBLICATION_SPACING * revision.saturating_sub(FIRST_PACKAGE_VERSION)
    }
    /// The configuration this plugin's packages are published with.
    ///
    /// A plugin that carries a license file of its own cannot be used without it, so
    /// the license is `REQUIRED` exactly where there is a license path to report; one
    /// without needs none.
    fn configuration(&self) -> PackageConfiguration {
        PackageConfiguration {
            license_requirement: match self.license_filepath {
                Some(_) => REQUIREMENT_LEVEL_REQUIRED.to_string(),
                None => REQUIREMENT_LEVEL_NONE.to_string(),
            },
            license_filepath: self.license_filepath.map(str::to_string),
            configuration_requirement: self.configuration_requirement.to_string(),
            requires_restart_for_configuration_update: Some(PLUGIN_REQUIRES_RESTART),
        }
    }

    /// The plugin's own properties, as the `PluginProperties` every read of one of its
    /// packages reports.
    ///
    /// `Version` is the plugin's version, which for a plugin of the engine
    /// distribution is the release it is built against, stated as the bare release the
    /// rest of this crate states one as ([`bare_engine_release`]).
    fn plugin_properties(&self, engine_version: &str) -> PluginProperties {
        PluginProperties {
            name: Some(self.name.to_string()),
            description: Some(self.description.to_string()),
            version: Some(bare_engine_release(engine_version).to_string()),
            class_name: Some(self.class_name.to_string()),
            uncompressed_size_in_bytes: Some(self.uncompressed_size_in_bytes),
        }
    }

    /// This plugin's package for `engine_version` as the [`Package`] every package
    /// operation answers from, or `None` for a release the catalog does not publish it
    /// for.
    ///
    /// The content of a published package is held by the service, not in a caller's
    /// bucket, so it names no Amazon S3 source and carries no imported content; it is
    /// published already in place, so no status move is owed it and the status every
    /// read reports is [`PACKAGE_STATUS_AVAILABLE`].
    pub fn record(&self, engine_version: &str) -> Option<Package> {
        let release_index = u32::try_from(
            self.engine_versions
                .iter()
                .position(|release| *release == engine_version)?,
        )
        .ok()?;
        let configuration = self.configuration();
        let plugin_properties = self.plugin_properties(engine_version);
        let versions: Vec<PackageVersionRecord> = (FIRST_PACKAGE_VERSION
            ..FIRST_PACKAGE_VERSION.saturating_add(self.revisions))
            .map(|revision| PackageVersionRecord {
                version: revision,
                // "A message associated with the package version when it was
                // uploaded": nothing uploads a package the service publishes, so
                // there is no message to report for one.
                commit_message: None,
                created_at: self.published_at(release_index, revision),
                configuration: Some(configuration.clone()),
                plugin_properties: Some(plugin_properties.clone()),
                // A ZIP plugin package carries a plugin archive, not the dictionary
                // text an index's analysis chain reads, so there is no content to
                // mount for one.
                content: None,
            })
            .collect();
        let created_at = self.published_at(release_index, FIRST_PACKAGE_VERSION);
        let updated_at = versions
            .last()
            .map_or(created_at, |latest| latest.created_at);
        Some(Package {
            package_id: published_package_id(self.name, engine_version),
            package_name: self.name.to_string(),
            package_type: PACKAGE_TYPE_ZIP_PLUGIN.to_string(),
            description: Some(self.description.to_string()),
            source: PackageSource::default(),
            status: PACKAGE_STATUS_AVAILABLE.to_string(),
            owed: None,
            removal_due: None,
            version: versions.last().map_or(FIRST_PACKAGE_VERSION, |latest| latest.version),
            versions,
            engine_version: Some(engine_version.to_string()),
            configuration: Some(configuration),
            plugin_properties: Some(plugin_properties),
            // "Package Vending Options" and "Encryption options for a package" are a
            // package OWNER's settings, which the service publishes none of for its
            // own packages.
            vending_options: None,
            encryption_options: None,
            // "A list of users who are allowed to view and associate the package. This
            // field is only visible to the owner of a package": the account owns no
            // published package, so there is no scope of one to report.
            allow_listed_users: Vec::new(),
            owner: self.publisher.to_string(),
            created_at,
            updated_at,
        })
    }
}

/// The published catalog as package records, derived ONCE and held for the life of the
/// process.
///
/// The rows are immutable reference data, so the records they project are the same on
/// every call: deriving them once is what keeps a package read — which resolves an id
/// against the whole catalog — from re-deriving every id's digest per request, exactly
/// as the published instance-type order is derived once for the offering catalog.
///
/// In `PackageID` order, which is the order the store keys an account's own packages
/// in, so the two sets merge into one order-stable set a pagination token can name a
/// place in.
fn published_catalog() -> &'static [Package] {
    static CATALOG: ::std::sync::OnceLock<Vec<Package>> = ::std::sync::OnceLock::new();
    CATALOG.get_or_init(|| {
        let mut packages: Vec<Package> = PUBLISHED_PACKAGES
            .iter()
            .flat_map(|published| {
                published
                    .engine_versions
                    .iter()
                    .filter_map(|engine_version| published.record(engine_version))
            })
            .collect();
        packages.sort_by(|left, right| left.package_id.cmp(&right.package_id));
        packages
    })
}

/// The whole published catalog as package records, in package-id order.
pub fn published_packages() -> Vec<Package> {
    published_catalog().to_vec()
}

/// The published package carrying `package_id`, if the catalog publishes one.
pub fn published_package(package_id: &str) -> Option<Package> {
    published_catalog()
        .iter()
        .find(|published| published.package_id == package_id)
        .cloned()
}

/// Whether every release the catalog publishes a plugin for is a release the service
/// offers at all ([`offers_engine_version`]): a package built against a release nothing
/// runs could be associated with no domain. Held by this service's package-catalog
/// suite.
pub fn catalog_publishes_published_releases() -> bool {
    PUBLISHED_PACKAGES.iter().all(|published| {
        published
            .engine_versions
            .iter()
            .all(|engine_version| offers_engine_version(engine_version))
    })
}
