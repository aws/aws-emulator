// SPDX-License-Identifier: CC0-1.0

//! The plugin packages the SERVICE publishes, which every account of a Region
//! sees beside the packages it created itself.
//!
//! `DescribePackages`'s own documentation is "Describes all packages available
//! to Amazon ES" — not "the packages this account created" — and the plugin
//! packages the service publishes are part of that set: a client discovers an
//! installable plugin by listing it here and then reads its versions with
//! `GetPackageVersionHistory` before associating one with a domain.
//!
//! The entries are READ-ONLY reference data, held as a table rather than
//! inserted into the store, so a read of them can never write: every answer
//! about a published package is [`PublishedPackage::record`] of the same table
//! row, which makes two reads of one agree and makes the whole catalog
//! identical in every Region.
//!
//! Each row's `PackageName` is the plugin's own name in the engine
//! distribution, which is what a ZIP plugin package installs; the ids are
//! derived (see [`published_package_id`]) rather than listed, since `PackageID`
//! is "internal ID of the package" (its model documentation) and so is the
//! service's to mint.
//!
//! The ROW SET itself — which plugins the table carries, the wording of each
//! description and the publication instants — is this emulator's own stand-in
//! for the catalogue a Region serves, chosen so that the catalogue is populated,
//! plausible and stable; it is not a transcription of what the service offers
//! today. What the operations guarantee is the shape and the resolution rule
//! above, so a row may be added or reworded without changing any of it.
//!
//! The SIBLING statement of the same content is
//! `service-com-amazonaws-opensearch-amazonopensearchservice`'s own
//! `state/package_catalog.rs`: the current API version of this service publishes the
//! same plugin catalog through its larger `PackageDetails` shape (per engine release,
//! with the plugin's properties and the package's configuration), so the two tables
//! are two projections of one catalog and an edit to the plugins one of them names
//! belongs in both.

use std::time::{Duration, SystemTime};

use super::*;

/// `PackageType` for a package whose content is a plugin archive.
///
/// The model's `PackageType` enum carries only `TXT-DICTIONARY` — the type a
/// CALLER may create here, and what `CreatePackage`'s own documentation says it
/// "currently supports" — while the plugin packages the service publishes are
/// reported with the plugin type, which is the value the service answers for
/// them.
pub const PACKAGE_TYPE_ZIP_PLUGIN: &str = "ZIP-PLUGIN";

/// One row of the catalog the service publishes.
#[derive(Debug, Clone, Copy)]
pub struct PublishedPackage {
    /// `PackageName` — the plugin's name in the engine distribution. It
    /// satisfies the member's own constraints (`@length(3, 28)`,
    /// `@pattern("^[a-z][a-z0-9\\-]+$")`).
    pub name: &'static str,
    /// `PackageDescription` — what the plugin does.
    pub description: &'static str,
    /// The instant, in seconds since the epoch, at which the service published
    /// each revision of this package's content, OLDEST FIRST. One entry per
    /// published version, so this sequence is the package's version history and
    /// its length is the number of versions the catalog offers.
    ///
    /// Fixed values, so the stamps a read reports are stable across reads and
    /// across processes rather than taken from the clock of the read.
    pub published_at: &'static [u64],
}

/// The plugin packages this service publishes, in name order.
///
/// A ZIP plugin package carries one plugin of the engine distribution, so each
/// row is named for the plugin it installs: the `analysis-*` tokenizers and
/// token filters a non-English index needs, the `mapper-*` field types, the
/// ingest processors, and the separately licensed third-party analyzers. The
/// members every row projects are `PackageDetails`'s own:
///
/// ```text
/// smithy_query(query="shape",shape="PackageDetails",model_path="services/service-com-amazonaws-elasticsearchservice-amazonelasticsearchservice2015/smithy-model/com-amazonaws-elasticsearchservice-amazonelasticsearchservice2015.json")
/// ```
pub const PUBLISHED_PACKAGES: &[PublishedPackage] = &[
    PublishedPackage {
        name: "analysis-icu",
        description: "ICU analysis plugin: Unicode-aware tokenization, normalization, collation \
                      and transliteration from the International Components for Unicode \
                      library.",
        published_at: &[1_600_000_000, 1_700_000_000],
    },
    PublishedPackage {
        name: "analysis-kuromoji",
        description: "Kuromoji analysis plugin: Japanese morphological tokenization, \
                      part-of-speech filtering and reading-form conversion.",
        published_at: &[1_600_000_000],
    },
    PublishedPackage {
        name: "analysis-nori",
        description: "Nori analysis plugin: Korean morphological tokenization backed by the \
                      mecab-ko dictionary.",
        published_at: &[1_626_048_000, 1_722_384_000],
    },
    PublishedPackage {
        name: "analysis-phonetic",
        description: "Phonetic analysis plugin: Soundex, Metaphone, Caverphone and Beider-Morse \
                      token filters for matching names that sound alike.",
        published_at: &[1_600_000_000, 1_672_531_200],
    },
    PublishedPackage {
        name: "analysis-seunjeon",
        description: "Seunjeon analysis plugin: Korean tokenization with a user-extensible \
                      dictionary.",
        published_at: &[1_626_048_000],
    },
    PublishedPackage {
        name: "analysis-smartcn",
        description: "SmartCN analysis plugin: probabilistic sentence and word segmentation for \
                      Simplified Chinese.",
        published_at: &[1_600_000_000],
    },
    PublishedPackage {
        name: "analysis-stempel",
        description: "Stempel analysis plugin: algorithmic stemming for Polish.",
        published_at: &[1_600_000_000],
    },
    PublishedPackage {
        name: "analysis-ukrainian",
        description: "Ukrainian analysis plugin: lemmatization through the Morfologik \
                      dictionary.",
        published_at: &[1_626_048_000],
    },
    PublishedPackage {
        name: "babel-street-match",
        description: "Babel Street Match analysis plugin: multilingual name matching and entity \
                      extraction. Licensed separately from the service.",
        published_at: &[1_700_000_000, 1_740_000_000],
    },
    PublishedPackage {
        name: "ingest-attachment",
        description: "Ingest attachment processor: extracts text and metadata from binary \
                      attachments with Apache Tika.",
        published_at: &[1_650_000_000, 1_740_000_000],
    },
    PublishedPackage {
        name: "mapper-annotated-text",
        description: "Annotated-text mapper plugin: a text field type that carries inline entity \
                      annotations alongside the text it indexes.",
        published_at: &[1_672_531_200],
    },
    PublishedPackage {
        name: "mapper-murmur3",
        description: "Murmur3 mapper plugin: stores the hash of a field's values at index time, \
                      for cheaper cardinality aggregations.",
        published_at: &[1_650_000_000],
    },
    PublishedPackage {
        name: "mapper-size",
        description: "Size mapper plugin: indexes the byte size of each document's _source in \
                      the _size field.",
        published_at: &[1_650_000_000],
    },
];

/// The id a published package is addressed by.
///
/// `PackageID` declares no constraint, so what matters is that the id is
/// opaque, stable and that two packages never share one: it is the leading 40
/// hex digits of a SHA-256 over the package's name, behind the prefix naming
/// the resource it identifies. Deriving it from the name rather than from a
/// counter is what makes a published package resolvable by the same id in every
/// Region and in every run, and the length keeps it apart from the ids
/// [`package_id`] mints for a caller's own packages.
pub fn published_package_id(name: &str) -> String {
    use sha2::Digest;
    let digest = sha2::Sha256::digest(name.as_bytes());
    let hex = hex::encode(digest);
    format!("pkg-{token}", token = &hex[..40])
}

impl PublishedPackage {
    /// This row as the [`Package`] every package operation answers from.
    ///
    /// The content of a published package is held by the service, not in a
    /// caller's bucket, so it names no S3 source; it is published already
    /// copied, so its copy is due at the instant it was published and the
    /// status a read computes is `AVAILABLE`.
    pub fn record(&self) -> Package {
        let versions: Vec<PackageVersionRecord> = self
            .published_at
            .iter()
            .enumerate()
            .map(|(index, at)| PackageVersionRecord {
                version: package_version_label(index + 1),
                // "A message associated with the version" — the service
                // publishes none for its own plugin packages.
                commit_message: None,
                created_at: instant_of(*at),
            })
            .collect();
        let created_at = versions
            .first()
            .map_or_else(|| instant_of(0), |first| first.created_at);
        let last_updated_at = versions
            .last()
            .map_or_else(|| instant_of(0), |last| last.created_at);
        Package {
            package_id: published_package_id(self.name),
            package_name: self.name.to_string(),
            package_type: PACKAGE_TYPE_ZIP_PLUGIN.to_string(),
            package_description: Some(self.description.to_string()),
            source: PackageSource::default(),
            created_at,
            last_updated_at,
            available_at: last_updated_at,
            versions,
        }
    }
}

/// An instant from its seconds since the epoch.
fn instant_of(epoch_seconds: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(epoch_seconds)
}

/// The whole published catalog as package records, in package-id order — the
/// order the store keys the account's own packages in, so the two sets merge
/// into one order-stable set a pagination token can name a place in.
pub fn published_packages() -> Vec<Package> {
    let mut packages: Vec<Package> = PUBLISHED_PACKAGES
        .iter()
        .map(PublishedPackage::record)
        .collect();
    packages.sort_by(|left, right| left.package_id.cmp(&right.package_id));
    packages
}

/// The published package carrying `package_id`, if the catalog holds one.
pub fn published_package(package_id: &str) -> Option<Package> {
    PUBLISHED_PACKAGES
        .iter()
        .find(|published| published_package_id(published.name) == package_id)
        .map(PublishedPackage::record)
}
