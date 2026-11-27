// SPDX-License-Identifier: CC0-1.0

//! The Apache Kafka version catalog this service publishes, and every answer
//! derived from it: the ROWS `ListKafkaVersions` pages, the de-duplicated version
//! set the create/update gates and their refusals read, the per-source upgrade
//! graph `GetCompatibleKafkaVersions` projects, and the paging of the row
//! sequence.
//!
//! One catalog, two readers: a PUBLISHING reader (the listing) serves the rows as
//! published, and a QUESTION-ANSWERING reader ([`catalogued_kafka_versions`],
//! [`kafka_version_status`]) answers about a version rather than about a row.

use super::*;

/// A `KafkaVersionStatus` value: a version Amazon MSK still offers for new
/// clusters and for upgrades.
pub const KAFKA_VERSION_ACTIVE: &str = "ACTIVE";
/// A `KafkaVersionStatus` value: a version Amazon MSK has deprecated. Existing
/// clusters keep running it, but it is not an upgrade target.
pub const KAFKA_VERSION_DEPRECATED: &str = "DEPRECATED";

/// One Apache Kafka version Amazon MSK knows about, with the lifecycle status it
/// publishes for that version.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KafkaVersionEntry {
    /// The version string as MSK names it, e.g. a three-component release, a
    /// `.x` name (MSK patches the trailing component for you) or the
    /// `2.8.2.tiered` variant.
    pub version: &'static str,
    /// [`KAFKA_VERSION_ACTIVE`] or [`KAFKA_VERSION_DEPRECATED`] — one of the
    /// model's `KafkaVersionStatus` values
    /// ([`crate::ops::types::KAFKA_VERSION_STATUS_VALUES`]).
    pub status: &'static str,
}

/// The variant name of the `.kraft` lines: the versions whose cluster metadata is
/// held by a KRaft controller quorum instead of Apache ZooKeeper.
pub const KAFKA_VERSION_VARIANT_KRAFT: &str = "kraft";

/// The variant name of the `.tiered` spelling: a ZooKeeper-mode version with
/// tiered storage, which MSK offers as a storage option of that same build (every
/// release from 3.6.0 on carries it without a spelling of its own).
pub const KAFKA_VERSION_VARIANT_TIERED: &str = "tiered";

/// Every ROW Amazon MSK publishes in its Apache Kafka version catalog, in the
/// order it publishes them: ascending by release, with a release's base version
/// ahead of the variant lines built on it.
///
/// Source: the Amazon MSK Developer Guide, "Supported Apache Kafka versions"
/// (<https://docs.aws.amazon.com/msk/latest/developerguide/supported-kafka-versions.html>),
/// which lists the versions MSK offers, the variants it offers them in (the
/// `.kraft` metadata-mode lines, the `.tiered` storage line, the `link` line) and
/// the ones it has deprecated, together with the catalog the service itself
/// publishes, which this table reproduces ROW FOR ROW. A version the catalog
/// publishes a deprecated row for is neither an upgrade TARGET nor an upgrade
/// SOURCE: the service publishes no path off a version it has stopped offering
/// ([`compatible_target_versions`]), while a cluster already running it goes on
/// running it.
///
/// Nothing outside that catalog belongs here: a version string the service does
/// not publish names no broker software, so this table holds EXACTLY the versions
/// MSK publishes and no spelling of its own. Nothing is MISSING from it either: a
/// row the service publishes and this table drops is a version a client cannot
/// discover, and the rows are what `ListKafkaVersions` and the published
/// compatibility matrix serve.
///
/// Publishing a row is not the same fact as taking a version for a NEW resource:
/// a broker category's new-cluster offering ([`new_cluster_kafka_versions`]) is
/// narrower than this catalog, which goes on publishing a version after the
/// service has stopped building clusters on it, so a row added here is published
/// without becoming a version a new cluster may name, and no offering ever names
/// a version this table does not publish.
///
/// A CONFIGURATION is different: it declares which versions a stored
/// `server.properties` body may be APPLIED to — clusters still running an older
/// version included — so every version this catalog publishes is one a
/// configuration may name ([`is_configuration_kafka_version`]), whatever status
/// it is published under. The valid-value list that create's refusal advertises
/// ([`valid_kafka_versions`]) is the service's own recorded sentence and a
/// separate fact from that gate.
///
/// A row is a version together with ONE status the service publishes it under,
/// and a version may be published on MORE THAN ONE row: the guide lists a
/// version among the ones MSK offers and among the ones it has deprecated while
/// clusters are still being moved off it, and the catalog publishes a row per
/// such listing. The published shape is therefore a SEQUENCE OF ROWS rather than
/// a version -> status map, and a walk of the listing's pages reproduces it row
/// for row — a version's second, other-status row included.
///
/// Those rows are published in ONE order: the LAST row published for a version
/// carries the status the service offers that version under
/// ([`kafka_version_status`]), so the catalog's final word about a version agrees
/// with the gates that accept it — a client reducing the published rows to one
/// status per version (a version has one lifecycle status) reads the version the
/// way a create naming it is answered. A version whose offering row came last
/// here would otherwise be published as deprecated while every create naming it
/// is accepted.
///
/// Everything that asks a QUESTION about a version — rather than publishing the
/// catalog — reads these rows through [`kafka_version_status`] (the status a
/// version is offered under: a version the catalog publishes an `ACTIVE` row for
/// is one the service still offers) and [`catalogued_kafka_versions`] (each
/// published version once, in published order), so publishing a second row for a
/// version never changes which versions the service offers.
///
/// This is the one catalog: `GetCompatibleKafkaVersions` derives its matrix from
/// it, so nothing has to keep a second copy of the version list in step.
pub const KAFKA_VERSION_CATALOG: &[KafkaVersionEntry] = &[
    KafkaVersionEntry { version: "1.1.1", status: KAFKA_VERSION_DEPRECATED },
    KafkaVersionEntry { version: "2.1.0", status: KAFKA_VERSION_DEPRECATED },
    KafkaVersionEntry { version: "2.2.1", status: KAFKA_VERSION_DEPRECATED },
    KafkaVersionEntry { version: "2.3.1", status: KAFKA_VERSION_DEPRECATED },
    KafkaVersionEntry { version: "2.4.1", status: KAFKA_VERSION_DEPRECATED },
    KafkaVersionEntry { version: "2.4.1.1", status: KAFKA_VERSION_DEPRECATED },
    KafkaVersionEntry { version: "2.5.1", status: KAFKA_VERSION_DEPRECATED },
    KafkaVersionEntry { version: "2.6.0", status: KAFKA_VERSION_DEPRECATED },
    KafkaVersionEntry { version: "2.6.1", status: KAFKA_VERSION_DEPRECATED },
    KafkaVersionEntry { version: "2.6.2", status: KAFKA_VERSION_DEPRECATED },
    KafkaVersionEntry { version: "2.6.3", status: KAFKA_VERSION_DEPRECATED },
    KafkaVersionEntry { version: "2.7.0", status: KAFKA_VERSION_DEPRECATED },
    KafkaVersionEntry { version: "2.7.1", status: KAFKA_VERSION_DEPRECATED },
    KafkaVersionEntry { version: "2.7.2", status: KAFKA_VERSION_DEPRECATED },
    KafkaVersionEntry { version: "2.8.0", status: KAFKA_VERSION_DEPRECATED },
    KafkaVersionEntry { version: "2.8.1", status: KAFKA_VERSION_DEPRECATED },
    KafkaVersionEntry { version: "2.8.2.tiered", status: KAFKA_VERSION_DEPRECATED },
    KafkaVersionEntry { version: "3.1.1", status: KAFKA_VERSION_DEPRECATED },
    KafkaVersionEntry { version: "3.2.0", status: KAFKA_VERSION_DEPRECATED },
    KafkaVersionEntry { version: "3.3.1", status: KAFKA_VERSION_DEPRECATED },
    KafkaVersionEntry { version: "3.3.2", status: KAFKA_VERSION_DEPRECATED },
    KafkaVersionEntry { version: "3.4.0", status: KAFKA_VERSION_DEPRECATED },
    KafkaVersionEntry { version: "3.5.1", status: KAFKA_VERSION_DEPRECATED },
    KafkaVersionEntry { version: "3.6.0", status: KAFKA_VERSION_ACTIVE },
    KafkaVersionEntry { version: "3.6.0.1", status: KAFKA_VERSION_DEPRECATED },
    KafkaVersionEntry { version: "3.7.x", status: KAFKA_VERSION_ACTIVE },
    KafkaVersionEntry { version: "3.7.x.kraft", status: KAFKA_VERSION_ACTIVE },
    KafkaVersionEntry { version: "3.8.x", status: KAFKA_VERSION_ACTIVE },
    KafkaVersionEntry { version: "3.8.x.kraft", status: KAFKA_VERSION_ACTIVE },
    // The `link` line of that same release: a spelling the service publishes a
    // row for and has deprecated, on no upgradable line of its own
    // ([`KafkaVersionLine::Other`]). No NEW CLUSTER is built on it and no upgrade
    // targets it, while a configuration may still declare itself valid for it
    // ([`is_configuration_kafka_version`]), because the service publishes it and
    // a cluster may be running it.
    KafkaVersionEntry { version: "3.8.link", status: KAFKA_VERSION_DEPRECATED },
    // The deprecation row of a version the catalog also publishes as offered,
    // ahead of the row offering it and beside it: the LAST row the catalog
    // publishes for a version carries the status the service offers that version
    // under, and the rows of one version stay together, so the published order is
    // still ascending by release with a release's base version ahead of the
    // variant lines built on it.
    KafkaVersionEntry { version: "3.9.x", status: KAFKA_VERSION_DEPRECATED },
    KafkaVersionEntry { version: "3.9.x", status: KAFKA_VERSION_ACTIVE },
    KafkaVersionEntry { version: "3.9.x.kraft", status: KAFKA_VERSION_ACTIVE },
    KafkaVersionEntry { version: "4.0.x.kraft", status: KAFKA_VERSION_ACTIVE },
    KafkaVersionEntry { version: "4.1.x.kraft", status: KAFKA_VERSION_ACTIVE },
    // The same for the newest release.
    KafkaVersionEntry { version: "4.2.x.kraft", status: KAFKA_VERSION_DEPRECATED },
    KafkaVersionEntry { version: "4.2.x.kraft", status: KAFKA_VERSION_ACTIVE },
];

/// Every version the catalog publishes, each ONCE, in the order its first
/// published row appears.
///
/// This is the version SET every version-aware answer is computed over — the
/// compatibility matrix's targets, the create and update gates and the
/// valid-value lists their refusals carry — because a question about a version is
/// about the version, not about the row it was read from
/// ([`KAFKA_VERSION_CATALOG`]). The SOURCES the matrix publishes a row for are
/// the catalog's rows instead ([`published_kafka_versions`]), because that matrix
/// is a published table rather than an answer about one version.
///
/// A derived CONSTANT of that catalog, so it is computed once rather than on
/// every call: the matrix answers one row per source and each row reads it.
pub fn catalogued_kafka_versions() -> &'static [&'static str] {
    static CATALOGUED: std::sync::LazyLock<Vec<&'static str>> = std::sync::LazyLock::new(|| {
        let mut versions: Vec<&'static str> = Vec::with_capacity(KAFKA_VERSION_CATALOG.len());
        for entry in KAFKA_VERSION_CATALOG {
            if !versions.contains(&entry.version) {
                versions.push(entry.version);
            }
        }
        versions
    });
    &CATALOGUED
}

/// The version of every ROW the catalog publishes, in published order — a version
/// published on more than one row appearing once PER ROW.
///
/// This is the sequence the published upgrade-compatibility matrix is a table of
/// (`GetCompatibleKafkaVersions` with no `ClusterArn`): that answer publishes one
/// row per (source version -> target versions) pair the service publishes, so a
/// version the catalog publishes twice is a source twice, exactly as
/// `ListKafkaVersions` pages that version's two rows. It is NOT the version set
/// ([`catalogued_kafka_versions`]), which is what every answer ABOUT a version
/// reads.
///
/// Derived from the catalog once, like [`catalogued_kafka_versions`]: both
/// factors are constants.
pub fn published_kafka_versions() -> &'static [&'static str] {
    static PUBLISHED: std::sync::LazyLock<Vec<&'static str>> = std::sync::LazyLock::new(|| {
        KAFKA_VERSION_CATALOG
            .iter()
            .map(|entry| entry.version)
            .collect()
    });
    &PUBLISHED
}

/// The release of the FINAL ZooKeeper-mode version the service still offers: the
/// newest release it publishes an offered ZooKeeper-mode version of.
///
/// That one release is where the KRaft migration is offered
/// ([`compatible_target_versions`]): Apache Kafka removed ZooKeeper, so the last
/// release the service publishes in BOTH metadata modes is the release at which a
/// ZooKeeper-mode cluster crosses over to the KRaft line, and an older release's
/// cluster climbs its own line to that release first.
///
/// Derived from the catalog once, like [`catalogued_kafka_versions`].
pub fn final_zookeeper_release() -> &'static [u32] {
    static FINAL_RELEASE: std::sync::LazyLock<Vec<u32>> = std::sync::LazyLock::new(|| {
        catalogued_kafka_versions()
            .iter()
            .filter(|version| is_offered_kafka_version(version))
            .filter(|version| kafka_version_line(version) == KafkaVersionLine::ZooKeeper)
            .map(|version| kafka_version_release(version))
            .max()
            .unwrap_or_default()
    });
    &FINAL_RELEASE
}

/// The Apache Kafka versions this service would INSTALL on a cluster that already
/// exists: the versions a new cluster of the standard brokers may be BUILT on
/// ([`new_cluster_kafka_versions`]), each once, in published order.
///
/// An in-place version update installs software the service still supports, which
/// is what the catalog's `ACTIVE` status says it does ([`kafka_version_status`]) —
/// the same question a create's admission gate asks, and a WIDER set than the one a
/// create's refusal ADVERTISES ([`advertised_new_cluster_kafka_versions`]): MSK goes
/// on upgrading a cluster onto a version it has stopped steering NEW clusters onto,
/// which is what keeps a cluster on an older line able to climb it.
///
/// The broker CATEGORY is not a question an upgrade asks — the cluster already has
/// its brokers — so this is ONE set: the standard category's, which is the software
/// the service installs on brokers of either category (a line it offers the Express
/// brokers alone is not software it installs on a running cluster).
///
/// Derived once, like [`catalogued_kafka_versions`]: both factors are constants.
pub fn upgrade_target_kafka_versions() -> &'static [&'static str] {
    static TARGETS: std::sync::LazyLock<Vec<&'static str>> = std::sync::LazyLock::new(|| {
        let shipped = new_cluster_offering(BROKER_CATEGORY_STANDARD);
        catalogued_kafka_versions()
            .iter()
            .filter(|version| shipped.contains(*version))
            .copied()
            .collect()
    });
    &TARGETS
}

/// The versions a cluster running `source` can be upgraded to: that ONE source's
/// own upgrade ladder, in published order — never a list shared by several
/// sources.
///
/// Amazon MSK upgrades a cluster's Apache Kafka version IN PLACE, never
/// downgrades, and installs only software it still ships
/// ([`upgrade_target_kafka_versions`]) — and an upgrade stays inside the source's
/// own LINE ([`kafka_version_line`]), because a line is a different build of the
/// broker rather than a later release of the same one. So a target must be a
/// version the service still offers a new cluster, sit on the SAME line as the
/// source and name a strictly newer release.
///
/// The ladder is offered only OFF a version the service still offers
/// ([`is_offered_kafka_version`]): once it has deprecated a version it publishes no
/// upgrade path off it, so a deprecated source is terminal whatever its line holds
/// above it, and so is a version the catalog publishes no row for at all.
///
/// The one cross-line move MSK offers is the KRaft migration, to the `.kraft`
/// version of the source's own release, and it is offered at ONE release: the
/// final ZooKeeper-mode release the service still offers
/// ([`final_zookeeper_release`]). A cluster on an older ZooKeeper-mode release
/// climbs its own line to that release first, and is offered no `.kraft` target
/// before it gets there. A version with no newer shipped member of its line, and
/// no KRaft version to cross over to, is terminal and reports an EMPTY list.
///
/// This is the one derivation behind all three answers about an upgrade: the ladder
/// the published matrix carries under this source, the ladder that matrix reports
/// for a cluster running it, and the set an update of such a cluster is admitted
/// on.
pub fn compatible_target_versions(source: &str) -> Vec<String> {
    if !is_offered_kafka_version(source) {
        return Vec::new();
    }
    let release = kafka_version_release(source);
    let line = kafka_version_line(source);
    let migration_release = final_zookeeper_release();
    upgrade_target_kafka_versions()
        .iter()
        .filter(|version| {
            let target_release = kafka_version_release(version);
            match (line, kafka_version_line(version)) {
                // The same line, further along it.
                (KafkaVersionLine::ZooKeeper, KafkaVersionLine::ZooKeeper)
                | (KafkaVersionLine::Kraft, KafkaVersionLine::Kraft) => target_release > release,
                // The KRaft migration: the same release, in KRaft metadata mode,
                // at the one release the service offers the crossing at.
                (KafkaVersionLine::ZooKeeper, KafkaVersionLine::Kraft) => {
                    target_release == release && release == migration_release
                }
                _ => false,
            }
        })
        .map(ToString::to_string)
        .collect()
}

/// Which build of the broker a catalogued version is: the thing an upgrade stays
/// inside, per [`compatible_target_versions`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KafkaVersionLine {
    /// A version whose cluster metadata is held by Apache ZooKeeper — the plain
    /// releases, and the `.tiered` spelling of one, since tiered storage is a
    /// storage option of that same build rather than a build of its own.
    ZooKeeper,
    /// A `.kraft` version: cluster metadata held by a KRaft controller quorum.
    Kraft,
    /// Any other spelling MSK publishes a release under. It names a build of its
    /// own, with no upgrade path off it.
    Other,
}

/// The line `version` belongs to, read from the trailing non-numeric component
/// MSK spells the line with ([`KAFKA_VERSION_VARIANT_KRAFT`],
/// [`KAFKA_VERSION_VARIANT_TIERED`]).
///
/// `x` is not a line: it is MSK's name for "the newest patch of this minor", so
/// it belongs to the release ([`kafka_version_release`] reads it).
pub fn kafka_version_line(version: &str) -> KafkaVersionLine {
    match kafka_version_variant(version) {
        KAFKA_VERSION_VARIANT_KRAFT => KafkaVersionLine::Kraft,
        "" | KAFKA_VERSION_VARIANT_TIERED => KafkaVersionLine::ZooKeeper,
        _ => KafkaVersionLine::Other,
    }
}

/// The variant line `version` belongs to: `""` for a plain ZooKeeper-mode
/// release, and otherwise the trailing non-numeric component MSK names the line
/// with (`kraft`, `tiered`, `link`).
///
/// `x` is not a variant: it is MSK's name for "the newest patch of this minor",
/// so it belongs to the release ([`kafka_version_release`] reads it).
pub fn kafka_version_variant(version: &str) -> &str {
    version
        .rsplit('.')
        .next()
        .filter(|last| *last != "x" && last.parse::<u32>().is_err())
        .unwrap_or_default()
}

/// The numeric release `version` names, as the components that order one release
/// against another: `3.6.0` as `[3, 6, 0]`, `2.4.1.1` as `[2, 4, 1, 1]`, and a
/// `.x` component — "the newest patch of this minor" — as the largest patch there
/// can be, so `3.7.x` orders above every numbered `3.7` patch.
///
/// The variant component is NOT part of the release, so a line's base version and
/// its variants name the same release (`3.9.x` and `3.9.x.kraft` are both release
/// `3.9.x`), which is what makes the KRaft migration a move between lines at ONE
/// release. `Vec<u32>` compares lexicographically, so a release that is a prefix
/// of another (`2.4.1` of `2.4.1.1`) orders below it.
pub fn kafka_version_release(version: &str) -> Vec<u32> {
    version
        .split('.')
        .filter_map(|component| match component.parse::<u32>() {
            Ok(number) => Some(number),
            // "the latest patch of this minor": above every numbered patch.
            Err(_) if component == "x" => Some(u32::MAX),
            // The variant component, which names a line rather than a release.
            Err(_) => None,
        })
        .collect()
}

/// Where `version` is FIRST published in [`KAFKA_VERSION_CATALOG`], or `None`
/// when MSK knows no such version — the PUBLICATION lookup, which is about the
/// version and not about any one of its rows.
///
/// Publication is not admission everywhere: whether a NEW CLUSTER may be built on
/// a version is that broker category's own recorded offering
/// ([`new_cluster_kafka_versions`]), which is narrower. This answers whether the
/// service publishes the version at all — which is what narrows that offering to
/// versions a reader of this service knows, and what a configuration's gate reads
/// ([`is_configuration_kafka_version`]).
///
/// A pagination token does NOT resolve through this: a version string names a
/// version and not a position, and a page resumes after a row
/// ([`decode_kafka_version_page_token`]).
pub fn kafka_version_index(version: &str) -> Option<usize> {
    KAFKA_VERSION_CATALOG
        .iter()
        .position(|entry| entry.version == version)
}

/// The support status `version` is OFFERED under, or `None` when MSK knows no
/// such version.
///
/// A version the catalog publishes on a single row is that row's status. A
/// version it publishes on more than one row ([`KAFKA_VERSION_CATALOG`]) is
/// offered under [`KAFKA_VERSION_ACTIVE`] whenever ANY of its rows is `ACTIVE`,
/// whatever order the rows are published in: the deprecation row of a version
/// the catalog still offers is published beside the offering row while clusters
/// are moved off it, and the service goes on accepting the version for as long as
/// it publishes an `ACTIVE` row for it. So this answers the version's lifecycle
/// question — a create may name it and an upgrade may target it — rather than
/// reporting whichever row comes first or last. The catalog publishes the rows so
/// that its LAST row for a version carries exactly this status
/// ([`KAFKA_VERSION_CATALOG`]), which is what makes the published catalog and this
/// answer agree.
pub fn kafka_version_status(version: &str) -> Option<&'static str> {
    let mut published: Option<&'static str> = None;
    for entry in KAFKA_VERSION_CATALOG
        .iter()
        .filter(|entry| entry.version == version)
    {
        if entry.status == KAFKA_VERSION_ACTIVE {
            return Some(KAFKA_VERSION_ACTIVE);
        }
        published = published.or(Some(entry.status));
    }
    published
}

/// The Apache Kafka versions Amazon MSK NAMES as valid when it refuses a
/// configuration naming a version it does not know
/// ([`unsupported_kafka_version_message`]), in the order it names them in.
///
/// The ADVERTISED list of that one sentence, and nothing else: it is not the gate
/// the create is admitted on ([`is_configuration_kafka_version`], which is the
/// published catalog, because a configuration declares the versions it may be
/// APPLIED to and clusters still run versions the service has deprecated). The
/// two are separate recorded facts — the service advertises the versions it names
/// here while admitting every version it publishes — so a version this list omits
/// is still one a configuration may name; it is only one this sentence does not
/// name.
///
/// The sequence is the service's OWN: it is not sorted by release, and it is
/// not the order the catalog publishes its rows in ([`KAFKA_VERSION_CATALOG`],
/// which `ListKafkaVersions` pages and which ascends by release) — it names the
/// newest KRaft releases and the variant lines first and the oldest release
/// last. So the refusal's list has an order of its own, recorded here as the
/// service states it in that message, rather than being assumed to be the
/// catalog's.
///
/// Source: the list the service itself renders in this refusal, which is both
/// the set it advertises and the order it names it in. Only versions the
/// catalog publishes are named ([`valid_kafka_versions`]), so a name here that
/// the catalog does not publish names nothing.
const KAFKA_VERSIONS_FOR_A_CONFIGURATION: &[&str] = &[
    "4.2.x.kraft",
    "4.1.x.kraft",
    "2.8.2.tiered",
    "4.0.x.kraft",
    "2.4.1.1",
    "3.3.2",
    "3.5.1",
    "3.6.0",
    "3.1.1",
    "3.2.0",
    "3.3.1",
    "3.4.0",
    "2.1.0",
    "3.7.x",
    "3.8.x",
    "2.6.2",
    "2.7.1",
    "2.8.0",
    "2.6.3",
    "2.7.2",
    "2.8.1",
    "2.5.1",
    "2.6.0",
    "2.6.1",
    "2.7.0",
    "3.6.0.1",
    "3.9.x",
    "2.3.1",
    "2.4.1",
    "3.7.x.kraft",
    "2.2.1",
    "3.8.x.kraft",
    "3.9.x.kraft",
    "1.1.1",
];

/// The versions the unsupported-version refusal lists as valid: the versions the
/// service names in that sentence ([`KAFKA_VERSIONS_FOR_A_CONFIGURATION`]),
/// narrowed to the ones the catalog really publishes, each named once.
///
/// The valid-value list ONLY. It is contained in the gate that create is admitted
/// on ([`is_configuration_kafka_version`]), so every version a client is told to
/// retry with is one the next request is accepted with — a list wider than the
/// gate would name a value the same request is refused a second time — while the
/// gate itself is the published catalog and so takes versions this list does not
/// name.
///
/// Narrowing to the catalog is what keeps the list from naming a version no
/// reader of this service knows; a row the catalog gains is published, and
/// admitted, with no change to this sentence.
///
/// Derived once, like [`catalogued_kafka_versions`]: both factors are constants.
pub fn valid_kafka_versions() -> &'static [&'static str] {
    static ORDERED: std::sync::LazyLock<Vec<&'static str>> = std::sync::LazyLock::new(|| {
        KAFKA_VERSIONS_FOR_A_CONFIGURATION
            .iter()
            .filter(|version| kafka_version_index(version).is_some())
            .copied()
            .collect::<Vec<&'static str>>()
    });
    &ORDERED
}

/// Whether a configuration may name `version`: whether the service PUBLISHES it
/// ([`kafka_version_index`]), on a row of any status.
///
/// A configuration is not a resource built on a version — it is a
/// `server.properties` body together with the versions it may be APPLIED to, and
/// it is applied to clusters that still run versions the service has deprecated —
/// so the versions a configuration may name are the ones the same service
/// publishes, deprecated rows and variant lines included. A version a client can
/// only have learnt from the published catalog is therefore never refused by the
/// next call, while a version the catalog publishes no row for names no broker
/// software and is refused.
///
/// Not the valid-value list that refusal advertises ([`valid_kafka_versions`]):
/// that list is the service's own recorded sentence, is contained in this gate,
/// and is read only to render the refusal.
pub fn is_configuration_kafka_version(version: &str) -> bool {
    kafka_version_index(version).is_some()
}

/// The sentence Amazon MSK answers a request naming an Apache Kafka version it
/// does not offer with: the fixed text `Unsupported KafkaVersion [`, `].
/// Valid values: [`, `]`, around two variable parts — the rejected version,
/// taken verbatim from the request, and the versions that ARE valid, read from
/// [`valid_kafka_versions`] in the order the service publishes that list.
///
/// The listed set is the service's own recorded one, a deprecated version
/// included, since a configuration may name a version existing clusters still
/// run. Every version it names is one the check this refusal comes from admits
/// ([`is_configuration_kafka_version`]), so each is retryable; that gate is the
/// published catalog, so it takes versions this sentence does not name. Each
/// version is named ONCE, however many rows the catalog publishes it under: the
/// list answers which versions the service names here, not how the catalog is
/// published.
///
/// The valid-value list is the actionable half of this refusal, so it is read
/// from that recorded list rather than written out here: a version the list gains
/// or loses is gained or lost in this sentence too.
pub fn unsupported_kafka_version_message(rejected: &str) -> String {
    format!(
        "Unsupported KafkaVersion [{rejected}]. Valid values: [{}]",
        valid_kafka_versions().join(", ")
    )
}

/// Whether the service still OFFERS `version`: whether the catalog publishes it
/// and the status it is offered under ([`kafka_version_status`]) is
/// [`KAFKA_VERSION_ACTIVE`]. This is what makes a version an upgrade SOURCE and
/// what a deprecation check reads; which versions an upgrade may LAND is the
/// narrower offering ([`upgrade_target_kafka_versions`]).
pub fn is_offered_kafka_version(version: &str) -> bool {
    kafka_version_status(version) == Some(KAFKA_VERSION_ACTIVE)
}

/// The input member a create names the Apache Kafka version of its brokers in,
/// as the API documents it on the wire — the camelCase spelling the request
/// itself carries, which is what a refusal of that value reports as the
/// parameter at fault.
pub const KAFKA_VERSION_MEMBER: &str = "kafkaVersion";

/// The prefix of the Express broker instance types (`express.m7g.*`): the
/// broker CATEGORY Amazon MSK offers beside the standard families
/// (`kafka.m5.*`, `kafka.m7g.*`, `kafka.t3.small` — see
/// [`is_broker_instance_type`]).
const EXPRESS_INSTANCE_TYPE_PREFIX: &str = "express.";

/// The broker category of the standard instance families (`kafka.m5.*`,
/// `kafka.m7g.*`, `kafka.t3.small`), as Amazon MSK words it in the version
/// refusal below.
pub const BROKER_CATEGORY_STANDARD: &str = "Standard";

/// The broker category of the Express instance family
/// ([`EXPRESS_INSTANCE_TYPE_PREFIX`]), as Amazon MSK words it in the version
/// refusal below.
pub const BROKER_CATEGORY_EXPRESS: &str = "Express";

/// The category of brokers `instance_type` names, as Amazon MSK words it in the
/// version refusal below: the two categories a provisioned cluster's brokers
/// come in, each with its own set of Apache Kafka versions, so the refusal says
/// which one the request asked for rather than assuming either.
pub fn broker_instance_category(instance_type: &str) -> &'static str {
    match instance_type.starts_with(EXPRESS_INSTANCE_TYPE_PREFIX) {
        true => BROKER_CATEGORY_EXPRESS,
        false => BROKER_CATEGORY_STANDARD,
    }
}

/// The `BrokerNodeType` a broker running on `instance_type` reports itself as:
/// the two values the model's `BrokerNodeType` enum names, chosen by the
/// instance type's own category ([`broker_instance_category`]).
///
/// The same reading of an instance type as the category above, so what a node
/// reports as its broker type and what the version gate calls that node's
/// category cannot disagree — a cluster whose brokers run an `express.*` type
/// runs Express brokers.
pub fn broker_node_type(instance_type: &str) -> &'static str {
    match broker_instance_category(instance_type) {
        BROKER_CATEGORY_EXPRESS => BROKER_NODE_TYPE_EXPRESS,
        _ => BROKER_NODE_TYPE_STANDARD,
    }
}

/// The `BrokerNodeType` value of an Express broker node, as the model's enum
/// spells it.
pub const BROKER_NODE_TYPE_EXPRESS: &str = "EXPRESS";

/// The `BrokerNodeType` value of a Standard broker node, as the model's enum
/// spells it.
pub const BROKER_NODE_TYPE_STANDARD: &str = "STANDARD";

/// The Apache Kafka versions Amazon MSK builds no new STANDARD-broker cluster on
/// although its catalog publishes an `ACTIVE` row for them: the newest KRaft line,
/// which MSK offers the EXPRESS brokers alone.
///
/// Recorded: a provisioned create naming such a line with a `kafka.*` instance
/// type is refused "Standard instance types are not supported for Kafka version
/// 4.2.x.kraft. Valid values: [...]", while a create naming the OLDEST `ACTIVE` row
/// of the same catalog is accepted. So the standard category's offering is the
/// published `ACTIVE` set MINUS these lines, and nothing else narrows it.
const KAFKA_VERSIONS_FOR_EXPRESS_BROKERS_ONLY: &[&str] = &["4.2.x.kraft"];

/// The versions Amazon MSK offers for a NEW cluster, PER BROKER CATEGORY, in the
/// order it names them in when it refuses a version a create may not name
/// ([`create_kafka_version_violation`]).
///
/// A category's offering is the set of versions the catalog publishes an `ACTIVE`
/// row for ([`kafka_version_status`]) — "a version the catalog publishes with
/// Status=ACTIVE is a version MSK still offers for a new cluster" — narrowed, for
/// the standard brokers, by the lines MSK offers the Express brokers alone
/// ([`KAFKA_VERSIONS_FOR_EXPRESS_BROKERS_ONLY`]). It is therefore narrower than the
/// published catalog, which goes on publishing a version — and going on offering it
/// as an upgrade target — after the service has deprecated it for a cluster that
/// does not exist yet.
///
/// Derived from the catalog rather than recorded beside it, so the listing and the
/// create gate cannot contradict each other: a version the listing publishes as
/// offered is one a create of the standard category may name, and a version it
/// publishes as deprecated is one no create may.
fn new_cluster_offering(category: &str) -> Vec<&'static str> {
    catalogued_kafka_versions()
        .iter()
        .filter(|version| kafka_version_status(version) == Some(KAFKA_VERSION_ACTIVE))
        .filter(|version| {
            category == BROKER_CATEGORY_EXPRESS
                || !KAFKA_VERSIONS_FOR_EXPRESS_BROKERS_ONLY.contains(version)
        })
        .copied()
        .collect()
}

/// The versions a create whose brokers are of `instance_type`'s category may
/// name, as the refusal below lists them: that category's offering
/// ([`new_cluster_offering`]), in the order the catalog publishes those versions.
///
/// This is the admission gate of a provisioned create. The valid-value list its
/// refusal ADVERTISES is a narrower, recorded set
/// ([`advertised_new_cluster_kafka_versions`]): MSK goes on admitting a version it
/// no longer advertises for a new cluster.
///
/// Derived once per category, like [`catalogued_kafka_versions`]: both factors
/// are constants.
pub fn new_cluster_kafka_versions(instance_type: &str) -> &'static [&'static str] {
    static OFFERED: std::sync::LazyLock<Vec<(&'static str, Vec<&'static str>)>> =
        std::sync::LazyLock::new(|| {
            [BROKER_CATEGORY_STANDARD, BROKER_CATEGORY_EXPRESS]
                .into_iter()
                .map(|category| (category, new_cluster_offering(category)))
                .collect()
        });
    let category = broker_instance_category(instance_type);
    let offering = OFFERED
        .iter()
        .find(|(offered_for, _)| *offered_for == category);
    // Every category [`broker_instance_category`] can report has a row above, so a
    // miss is a table that lost one rather than a request the service may answer:
    // caught here in a debug build instead of becoming a refusal that offers
    // nothing.
    debug_assert!(
        offering.is_some(),
        "every broker category must have an offering recorded"
    );
    offering.map_or(&[], |(_, versions)| versions.as_slice())
}

/// The Apache Kafka versions Amazon MSK ADVERTISES for a new cluster of the
/// STANDARD brokers, in the order it names them in, which is the order the catalog
/// publishes them in.
///
/// RECORDED beside the refusal rather than derived: a provisioned create naming a
/// version MSK does not offer at all, with a `kafka.*` instance type, is answered
/// "Standard instance types are not supported for Kafka version <v>. Valid values:
/// [3.8.x, 3.8.x.kraft, 3.9.x, 3.9.x.kraft, 4.0.x.kraft, 4.1.x.kraft]". That is
/// NARROWER than the set of versions whose catalog row is `ACTIVE`, which the
/// service keeps publishing for the clusters already running them and as upgrade
/// sources: publication, even publication as offered, is not the same question as
/// which versions MSK advertises to a cluster that does not exist yet.
const ADVERTISED_NEW_CLUSTER_VERSIONS_STANDARD: &[&str] = &[
    "3.8.x",
    "3.8.x.kraft",
    "3.9.x",
    "3.9.x.kraft",
    "4.0.x.kraft",
    "4.1.x.kraft",
];

/// The same advertised offering for the EXPRESS brokers: the standard one plus the
/// lines MSK offers those brokers alone
/// ([`KAFKA_VERSIONS_FOR_EXPRESS_BROKERS_ONLY`]), in published order — the one
/// difference between the two categories this service knows of, so the two lists
/// cannot drift apart over anything else.
fn advertised_new_cluster_versions_express() -> Vec<&'static str> {
    catalogued_kafka_versions()
        .iter()
        .filter(|version| {
            ADVERTISED_NEW_CLUSTER_VERSIONS_STANDARD.contains(*version)
                || KAFKA_VERSIONS_FOR_EXPRESS_BROKERS_ONLY.contains(*version)
        })
        .copied()
        .collect()
}

/// The versions the refusal below ADVERTISES to a create whose brokers are of
/// `instance_type`'s category: that category's advertised new-cluster offering, in
/// the order MSK publishes it.
///
/// The actionable half of the refusal — what a client reads to pick a version it
/// may retry with — so it is MSK's own offering for that broker category rather
/// than a projection of the catalog's `ACTIVE` rows, which is a wider set serving a
/// different question ([`new_cluster_kafka_versions`], the admission gate). Every
/// version advertised here is one that gate admits, so a client retrying with a
/// version the refusal named is not refused a second time.
pub fn advertised_new_cluster_kafka_versions(instance_type: &str) -> &'static [&'static str] {
    static ADVERTISED: std::sync::LazyLock<Vec<(&'static str, Vec<&'static str>)>> =
        std::sync::LazyLock::new(|| {
            vec![
                (
                    BROKER_CATEGORY_STANDARD,
                    ADVERTISED_NEW_CLUSTER_VERSIONS_STANDARD.to_vec(),
                ),
                (
                    BROKER_CATEGORY_EXPRESS,
                    advertised_new_cluster_versions_express(),
                ),
            ]
        });
    let category = broker_instance_category(instance_type);
    let advertised = ADVERTISED
        .iter()
        .find(|(advertised_for, _)| *advertised_for == category);
    // Every category [`broker_instance_category`] can report has a row above, so a
    // miss is a table that lost one rather than a request the service may answer.
    debug_assert!(
        advertised.is_some(),
        "every broker category must have an advertised offering recorded"
    );
    // And nothing advertised may be a version the gate would refuse: the retry the
    // refusal invites has to be admitted.
    debug_assert!(
        advertised.is_none_or(|(_, versions)| versions
            .iter()
            .all(|version| new_cluster_kafka_versions(instance_type).contains(version))),
        "every advertised version must be one the create gate admits"
    );
    advertised.map_or(&[], |(_, versions)| versions.as_slice())
}

/// Why a provisioned create naming Apache Kafka version `kafka_version` for
/// brokers of `instance_type` describes no cluster the service could run, or
/// `None` when the service takes that version for a new cluster of those
/// brokers' category.
///
/// Amazon MSK runs a new cluster's brokers on a version it still OFFERS that
/// broker category ([`new_cluster_kafka_versions`]) — a narrower set than the
/// catalog `ListKafkaVersions` publishes, which goes on publishing a version, and
/// going on offering it as an upgrade target, after MSK has stopped building new
/// clusters on it. So a value outside that category's offering names no cluster
/// this service would build, whether the catalog publishes it or not, and the
/// create is refused from the REQUEST alone, before anything the request names is
/// resolved against this account's network or registry.
///
/// The sentence is the service's: the broker category the request asked for
/// ([`broker_instance_category`]), the version it named, and the versions MSK
/// ADVERTISES for a NEW cluster of that category
/// ([`advertised_new_cluster_kafka_versions`]) — the actionable half of the
/// refusal, and a subset of what this gate admits, so a client retrying with a
/// version the refusal named is not refused a second time.
pub fn create_kafka_version_violation(kafka_version: &str, instance_type: &str) -> Option<String> {
    let offered = new_cluster_kafka_versions(instance_type);
    if offered.contains(&kafka_version) {
        return None;
    }
    Some(format!(
        "{} instance types are not supported for Kafka version {kafka_version}. \
         Valid values: [{}]",
        broker_instance_category(instance_type),
        advertised_new_cluster_kafka_versions(instance_type).join(", ")
    ))
}

/// Marker that prefixes a version page token's plaintext, so a token minted for
/// some other list cannot be mistaken for one of these.
const KAFKA_VERSION_PAGE_TOKEN_PREFIX: &str = "kafka:kafka-versions:";

/// Encode the catalog ROW a page stopped at — its position in
/// [`KAFKA_VERSION_CATALOG`] — as a token, through the same [`encode_page_token`]
/// every listing here mints its tokens with.
///
/// A page resumes after a row rather than after a version string because the
/// catalog publishes two of its versions under more than one row, so a version
/// string would not name the row a page stopped at. The position inside is only
/// resolved against the public catalog, so a forged resume point can only skip or
/// repeat entries every caller may already list.
pub fn encode_kafka_version_page_token(last_index: usize) -> String {
    encode_page_token(KAFKA_VERSION_PAGE_TOKEN_PREFIX, &last_index.to_string())
}

/// The catalog row a token resumes after, or `None` when the token is not one the
/// version listing minted or does not name a row the catalog holds — which that
/// listing refuses as a bad request rather than serving as a first page.
pub fn decode_kafka_version_page_token(token: &str) -> Option<usize> {
    decode_page_token(KAFKA_VERSION_PAGE_TOKEN_PREFIX, token)?
        .parse::<usize>()
        .ok()
        .filter(|index| *index < KAFKA_VERSION_CATALOG.len())
}

/// One page of the version catalog: the entries it reports, and the token that
/// resumes after them — absent on the last page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KafkaVersionPage {
    pub entries: &'static [KafkaVersionEntry],
    pub next_token: Option<String>,
}

/// Choose one page of [`KAFKA_VERSION_CATALOG`]: the `page_size` rows that
/// follow `resume_after` (a catalog position, as
/// [`decode_kafka_version_page_token`] resolves one), or the first `page_size`
/// rows when a request names no token.
///
/// `page_size` is clamped into the `MaxResults` range the model declares, so no
/// caller of this module can size the page from an unchecked value. Every page is
/// a contiguous run of the catalog in its published order, so the pages of any
/// walk partition the catalog exactly once — row for row, a version the catalog
/// publishes twice included.
///
/// A `resume_after` naming the LAST catalogued row yields an empty final page
/// with no token: the listing never mints a token for the last row (the page
/// that reports it closes the list), so only a forged resume point reaches that
/// case, and an empty page is the truthful answer to "what follows the end".
pub fn kafka_version_page(resume_after: Option<usize>, page_size: usize) -> KafkaVersionPage {
    let page_size = page_size.clamp(1, MAX_RESULTS_LIMIT as usize);
    let start = resume_after
        .map_or(0, |index| index.saturating_add(1))
        .min(KAFKA_VERSION_CATALOG.len());
    let end = start
        .saturating_add(page_size)
        .min(KAFKA_VERSION_CATALOG.len());
    let entries = &KAFKA_VERSION_CATALOG[start..end];
    // The token names the row this page stopped at — the same position the resume
    // lookup resolves — and is absent once the page reaches the end of the
    // catalog, which is what tells a paginating client to stop.
    let next_token = if end < KAFKA_VERSION_CATALOG.len() && !entries.is_empty() {
        Some(encode_kafka_version_page_token(end - 1))
    } else {
        None
    };
    KafkaVersionPage {
        entries,
        next_token,
    }
}
