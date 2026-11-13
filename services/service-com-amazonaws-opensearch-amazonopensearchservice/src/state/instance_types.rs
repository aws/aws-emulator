// SPDX-License-Identifier: CC0-1.0

//! The instance-type catalog, split out of [`crate::state`] unchanged: which instance
//! types an engine version offers, which node roles each of them can take, what each
//! can be scaled to, and the domain configurations that catalog refuses.

use super::*;

/// `InstanceTypeDetails$InstanceRole` for an instance that holds the domain's data,
/// from that member's own documentation: "whether the instance acts as a data node, a
/// dedicated master node, or an UltraWarm node".
pub const INSTANCE_ROLE_DATA: &str = "data";

/// `InstanceTypeDetails$InstanceRole` for an instance that can serve as the cluster's
/// dedicated master node.
pub const INSTANCE_ROLE_MASTER: &str = "master";

/// `InstanceTypeDetails$InstanceRole` for an instance that serves the UltraWarm
/// storage tier.
pub const INSTANCE_ROLE_ULTRAWARM: &str = "ultrawarm";

/// `InstanceTypeDetails$InstanceRole` for an instance that can serve as a dedicated
/// coordinator node: one that routes and merges a request across the cluster and holds
/// none of its data. A family whose instances can be provisioned in that role reports
/// it beside the roles it serves, which is what tells a caller the role is available on
/// the type at all.
pub const INSTANCE_ROLE_COORDINATOR: &str = "coordinator";

/// An `EngineVersion` read apart: the engine it names and its `X.Y` release numbers,
/// which is what every version gate in this section is expressed against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EngineVersion {
    /// The `EngineType` the version names: [`ENGINE_TYPE_OPENSEARCH`] or
    /// [`ENGINE_TYPE_ELASTICSEARCH`], as [`engine_type_of`] derives it.
    pub engine: &'static str,
    /// The major release number (the `X` of `X.Y`).
    pub major: u32,
    /// The minor release number (the `Y` of `X.Y`).
    pub minor: u32,
}

impl EngineVersion {
    /// Whether this version runs the OpenSearch engine rather than the legacy
    /// Elasticsearch one.
    pub fn is_opensearch(self) -> bool {
        self.engine == ENGINE_TYPE_OPENSEARCH
    }

    /// Whether this version is at or past an `X.Y` release OF THE ENGINE IT NAMES.
    pub fn at_least(self, (major, minor): (u32, u32)) -> bool {
        (self.major, self.minor) >= (major, minor)
    }

    /// Whether a capability introduced in an Elasticsearch release is available here.
    ///
    /// OpenSearch 1.0 forked from Elasticsearch 7.10, so an OpenSearch version
    /// carries every capability an Elasticsearch release up to that fork introduced —
    /// which each gate in this section is — and an Elasticsearch version carries it
    /// from the release it arrived in.
    pub fn since_elasticsearch(self, release: (u32, u32)) -> bool {
        self.is_opensearch() || self.at_least(release)
    }

    /// Whether this version is an OpenSearch release at or past `X.Y` — the gate for
    /// a capability that never existed on Elasticsearch.
    pub fn opensearch_at_least(self, release: (u32, u32)) -> bool {
        self.is_opensearch() && self.at_least(release)
    }
}

/// An `EngineVersion` read apart, or `None` for a value that is not
/// `Elasticsearch_X.Y` or `OpenSearch_X.Y`.
///
/// `VersionString`'s pattern
/// `^Elasticsearch_[0-9]{1}\.[0-9]{1,2}$|^OpenSearch_[0-9]{1,2}\.[0-9]{1,2}$` allows
/// nothing else, but the request boundary enforces only that shape's LENGTH bound, so
/// an operation that reads a version apart checks the form itself rather than
/// treating an unreadable value as a version.
pub fn parse_engine_version(engine_version: &str) -> Option<EngineVersion> {
    let engine = engine_type_of(engine_version)?;
    let (_, release) = engine_version.split_once('_')?;
    let (major, minor) = release.split_once('.')?;
    Some(EngineVersion {
        engine,
        major: major.parse().ok()?,
        minor: minor.parse().ok()?,
    })
}

/// The Elasticsearch release both encryption features `InstanceTypeDetails$Encryption\
/// Enabled` covers are available from. That member is "whether encryption at rest AND
/// node-to-node encryption are supported for the instance type", so the gate is the
/// LATER of the two: node-to-node encryption arrived in Elasticsearch 6.0, encryption
/// at rest before it.
pub(super) const ENCRYPTION_SINCE_ELASTICSEARCH: (u32, u32) = (6, 0);

/// The Elasticsearch release Amazon Cognito authentication for Dashboards is
/// available from (`InstanceTypeDetails$CognitoEnabled`).
pub(super) const COGNITO_SINCE_ELASTICSEARCH: (u32, u32) = (5, 1);

/// The Elasticsearch release publishing application logs to CloudWatch Logs is
/// available from (`InstanceTypeDetails$AppLogsEnabled`), which is also the release
/// the `LogPublishingOptions` container appeared with.
pub(super) const APP_LOGS_SINCE_ELASTICSEARCH: (u32, u32) = (5, 1);

/// The Elasticsearch release fine-grained access control is available from
/// (`InstanceTypeDetails$AdvancedSecurityEnabled`).
pub(super) const ADVANCED_SECURITY_SINCE_ELASTICSEARCH: (u32, u32) = (6, 7);

/// The Elasticsearch release UltraWarm storage is available from
/// (`InstanceTypeDetails$WarmEnabled`), which is also the release the UltraWarm
/// instance family was introduced for.
pub(super) const ULTRAWARM_SINCE_ELASTICSEARCH: (u32, u32) = (6, 8);

/// The Elasticsearch release the Graviton instance families are offered from.
pub(super) const GRAVITON_SINCE_ELASTICSEARCH: (u32, u32) = (7, 9);

/// The OpenSearch release the OpenSearch-optimized instance family is offered from.
/// It is an OpenSearch-only family, so this gate is not an Elasticsearch one.
pub(super) const OPENSEARCH_OPTIMIZED_SINCE: (u32, u32) = (2, 11);

/// The OpenSearch release the SECOND OpenSearch-optimized generation is offered from:
/// later families of the same line, so they are offered from a later release than
/// [`OPENSEARCH_OPTIMIZED_SINCE`] and, like it, on no Elasticsearch release. It is the
/// generation the recorded catalogs of two OpenSearch releases differ by, which is what
/// fixes the gate at a release rather than at the engine.
pub(super) const OPENSEARCH_OPTIMIZED_GEN2_SINCE: (u32, u32) = (2, 17);

/// The OpenSearch release the seventh-generation Intel families are offered from.
///
/// That generation arrived after the engines forked, so no Elasticsearch release offers
/// it and the gate is an OpenSearch one. AWS publishes no machine-readable boundary for
/// an instance family, so the release itself is the emulator's own statement of
/// published content, like the figures in the price and limit tables above: what these
/// gates fix is that a release predating a family does not offer it, that an
/// Elasticsearch release offers none of the generations that postdate the fork, and
/// that those generations are offered in the order they arrived — this one, then the
/// Graviton4 compute family ([`GRAVITON4_SINCE_OPENSEARCH`]), then the second
/// OpenSearch-optimized generation, which is the one ordering the recorded catalogs of
/// two OpenSearch releases show.
pub(super) const SEVENTH_GENERATION_INTEL_SINCE_OPENSEARCH: (u32, u32) = (2, 13);

/// The OpenSearch release the Graviton4 families are offered from — see
/// [`SEVENTH_GENERATION_INTEL_SINCE_OPENSEARCH`] for what this gate fixes and what in
/// it is the emulator's own statement.
pub(super) const GRAVITON4_SINCE_OPENSEARCH: (u32, u32) = (2, 15);

/// The instance families neither encryption at rest nor node-to-node encryption is
/// available on, whatever the engine version
/// (`InstanceTypeDetails$EncryptionEnabled`). Fine-grained access control requires
/// both, so it is unavailable on them too.
pub(super) const UNENCRYPTED_FAMILIES: [&str; 2] = ["m3", "t2"];

/// The burstable instance families. UltraWarm cannot be attached to a cluster whose
/// hot tier runs on one of them, so `InstanceTypeDetails$WarmEnabled` is off for
/// them.
pub(super) const BURSTABLE_FAMILIES: [&str; 3] = ["t2", "t3", "t4g"];

/// The instance family that serves the UltraWarm tier: its instances act as UltraWarm
/// nodes rather than as data or master nodes, which is the `InstanceRole` they carry.
pub(super) const ULTRAWARM_FAMILY: &str = "ultrawarm1";

/// The node roles a general-purpose, compute-optimized or memory-optimized family
/// serves: a node of one holds the domain's data, is master-eligible, and can be
/// provisioned as a dedicated coordinator node.
pub(super) const ROLES_WITH_COORDINATOR: &[&str] = &[
    INSTANCE_ROLE_DATA,
    INSTANCE_ROLE_MASTER,
    INSTANCE_ROLE_COORDINATOR,
];

/// The roles a family that is master-eligible but runs no dedicated coordinator node
/// serves — a coordinator node holds none of the domain's data, so a storage-optimized
/// family, a burstable one and a previous generation publish these two.
pub(super) const ROLES_DATA_AND_MASTER: &[&str] = &[INSTANCE_ROLE_DATA, INSTANCE_ROLE_MASTER];

/// The roles an OpenSearch-optimized family serves: its instances hold the domain's
/// data and are never master-eligible.
pub(super) const ROLES_DATA_ONLY: &[&str] = &[INSTANCE_ROLE_DATA];

/// The roles the UltraWarm family serves: the warm tier, and nothing else.
pub(super) const ROLES_ULTRAWARM_ONLY: &[&str] = &[INSTANCE_ROLE_ULTRAWARM];

/// Which engine releases offer one instance family.
#[derive(Debug, Clone, Copy)]
pub(super) enum OfferedReleases {
    /// Every release the service publishes — a family with no published engine
    /// prerequisite, which is most of the catalog: the recorded catalog of a current
    /// OpenSearch release carries previous-generation entries too, so the catalog is
    /// not filtered by instance generation.
    Every,
    /// From an Elasticsearch release onwards — and so on every OpenSearch release,
    /// which continues the Elasticsearch 7 line.
    SinceElasticsearch((u32, u32)),
    /// From an OpenSearch release onwards, and on no Elasticsearch release: a family
    /// the service publishes for the OpenSearch engine alone.
    SinceOpenSearch((u32, u32)),
}

/// One family of the published instance-type catalog: the instance types the service
/// offers it as, the node roles an instance of it serves, and the releases that offer
/// it.
#[derive(Debug, Clone, Copy)]
pub(super) struct CatalogFamily {
    /// The family token — the `<family>` of `<family>.<size>.search`.
    pub(super) family: &'static str,
    /// The instance types the family is published as, smallest size first. Each one's
    /// size is a token of [`SIZE_NORMALIZATION_FACTORS`], so every published type has a
    /// size the rest of this crate can scale a per-size quantity against.
    pub(super) instance_types: &'static [&'static str],
    /// The roles an instance of the family serves, in the order they are reported.
    pub(super) roles: &'static [&'static str],
    /// The releases that offer the family.
    pub(super) releases: OfferedReleases,
}

impl CatalogFamily {
    /// Whether `version` offers this family.
    pub(super) fn offered_on(&self, version: EngineVersion) -> bool {
        match self.releases {
            OfferedReleases::Every => true,
            OfferedReleases::SinceElasticsearch(release) => version.since_elasticsearch(release),
            OfferedReleases::SinceOpenSearch(release) => version.opensearch_at_least(release),
        }
    }
}

/// The instance families AWS publishes as PREVIOUS generation for this service — the
/// generation before the current one, which it still offers. The published catalog
/// lists them after the current generations, which is what [`catalog_rank`] uses them
/// for. [`PREVIOUS_GENERATION_EBS_FAMILIES`] is the subset of them the magnetic volume
/// sub-type is published for.
pub(super) const PREVIOUS_GENERATION_FAMILIES: [&str; 8] = ["c4", "d2", "i2", "m3", "m4", "r3", "r4", "t2"];

/// Where one family sits in the published catalog: the current generations first, then
/// the UltraWarm tier — which holds no hot-tier node and is published apart from them —
/// then the previous generations, and inside a group by family name.
///
/// The GROUPING is the one AWS's supported-instance-types documentation for this
/// service publishes: the current generations, the warm tier and the previous
/// generations, in that order. The order of the families WITHIN a group is by name,
/// which is the emulator's own statement — that page groups a generation's families by
/// instance class rather than naming an order among them, and the service's own
/// ordering is not recoverable from anything it publishes. Computing the rank here
/// rather than writing a sequence out is what makes the published order a property of
/// the CATALOG: adding a family puts it where the grouping puts it, not where it was
/// appended.
pub(super) fn catalog_rank(entry: &CatalogFamily) -> (u8, &'static str) {
    let group = if entry.family == ULTRAWARM_FAMILY {
        1
    } else if PREVIOUS_GENERATION_FAMILIES.contains(&entry.family) {
        2
    } else {
        0
    };
    (group, entry.family)
}

/// The instance-type catalog the service publishes, one entry per instance FAMILY.
///
/// It is published in [`catalog_rank`]'s order — the generation grouping AWS's
/// supported-instance-types documentation for this service publishes, by family name
/// within a group — and each family's instance types smallest size first, so the order
/// of the rows below is not itself the order of a listing.
///
/// This is the service's own published content, so it is written out here rather than
/// derived from `OpenSearchPartitionInstanceType`: the service offers families that
/// enum does not list at all, and a modelled value set carries neither the engine
/// releases that offer a family nor the node roles an instance of it serves. It
/// carries every value of the modelled enum and the current generations the service's
/// answers add to it.
///
/// The current generations are the ones the service's OWN accepted-value list reports —
/// the list its refusal of an unknown instance type enumerates, which is the service's
/// record of the types it accepts and carries families the checked-in enum does not:
/// the seventh-generation Intel families, the Graviton3 and Graviton4 families, the
/// dense-storage ones, and the second OpenSearch-optimized generation. A family is in
/// this table because that list reports it, never because a release's listing happened
/// to show it.
///
/// What is NOT published anywhere machine-readable, and is therefore the emulator's own
/// statement of published content exactly as [`CLASS_HOURLY_RATE_PER_UNIT`] and
/// [`MAXIMUM_DATA_NODE_COUNT`] are: the sizes each family is offered in, and the engine
/// release a family carries no documented prerequisite for (such a family is offered on
/// every release the service publishes). The service's own internal ordering of its
/// catalog is not recoverable from anything it publishes either, so the documented
/// order above is what the catalog is published in, and it is stable: element N of a
/// release's listing is the same instance type on every call.
///
/// Every other reader of an instance type — its roles, its limits, the validation of a
/// cluster configuration — reads this one table, so no two of them can disagree about
/// which instance types exist. Every value of the modelled enum has to be one of them,
/// which [`publishes_every_modelled_instance_type`] holds.
pub(super) const INSTANCE_TYPE_CATALOG: &[CatalogFamily] = &[
    // General purpose.
    CatalogFamily {
        family: "m8g",
        instance_types: &[
            "m8g.medium.search",
            "m8g.large.search",
            "m8g.xlarge.search",
            "m8g.2xlarge.search",
            "m8g.4xlarge.search",
            "m8g.8xlarge.search",
            "m8g.12xlarge.search",
            "m8g.16xlarge.search",
            "m8g.24xlarge.search",
        ],
        roles: ROLES_WITH_COORDINATOR,
        releases: OfferedReleases::SinceOpenSearch(GRAVITON4_SINCE_OPENSEARCH),
    },
    CatalogFamily {
        family: "m7i",
        instance_types: &[
            "m7i.large.search",
            "m7i.xlarge.search",
            "m7i.2xlarge.search",
            "m7i.4xlarge.search",
            "m7i.8xlarge.search",
            "m7i.12xlarge.search",
            "m7i.16xlarge.search",
            "m7i.24xlarge.search",
        ],
        roles: ROLES_WITH_COORDINATOR,
        releases: OfferedReleases::SinceOpenSearch(SEVENTH_GENERATION_INTEL_SINCE_OPENSEARCH),
    },
    CatalogFamily {
        family: "m6g",
        instance_types: &[
            "m6g.large.search",
            "m6g.xlarge.search",
            "m6g.2xlarge.search",
            "m6g.4xlarge.search",
            "m6g.8xlarge.search",
            "m6g.12xlarge.search",
        ],
        roles: ROLES_WITH_COORDINATOR,
        releases: OfferedReleases::SinceElasticsearch(GRAVITON_SINCE_ELASTICSEARCH),
    },
    CatalogFamily {
        family: "m5",
        instance_types: &[
            "m5.large.search",
            "m5.xlarge.search",
            "m5.2xlarge.search",
            "m5.4xlarge.search",
            "m5.12xlarge.search",
            "m5.24xlarge.search",
        ],
        roles: ROLES_WITH_COORDINATOR,
        releases: OfferedReleases::Every,
    },
    CatalogFamily {
        family: "m4",
        instance_types: &[
            "m4.large.search",
            "m4.xlarge.search",
            "m4.2xlarge.search",
            "m4.4xlarge.search",
            "m4.10xlarge.search",
        ],
        roles: ROLES_DATA_AND_MASTER,
        releases: OfferedReleases::Every,
    },
    CatalogFamily {
        family: "m3",
        instance_types: &[
            "m3.medium.search",
            "m3.large.search",
            "m3.xlarge.search",
            "m3.2xlarge.search",
        ],
        roles: ROLES_DATA_AND_MASTER,
        releases: OfferedReleases::Every,
    },
    // Compute optimized.
    CatalogFamily {
        family: "c7i",
        instance_types: &[
            "c7i.large.search",
            "c7i.xlarge.search",
            "c7i.2xlarge.search",
            "c7i.4xlarge.search",
            "c7i.8xlarge.search",
            "c7i.12xlarge.search",
            "c7i.16xlarge.search",
            "c7i.24xlarge.search",
        ],
        roles: ROLES_WITH_COORDINATOR,
        releases: OfferedReleases::SinceOpenSearch(SEVENTH_GENERATION_INTEL_SINCE_OPENSEARCH),
    },
    CatalogFamily {
        family: "c8g",
        instance_types: &[
            "c8g.large.search",
            "c8g.xlarge.search",
            "c8g.2xlarge.search",
            "c8g.4xlarge.search",
            "c8g.8xlarge.search",
            "c8g.12xlarge.search",
            "c8g.16xlarge.search",
            "c8g.24xlarge.search",
        ],
        roles: ROLES_WITH_COORDINATOR,
        releases: OfferedReleases::SinceOpenSearch(GRAVITON4_SINCE_OPENSEARCH),
    },
    CatalogFamily {
        family: "c6g",
        instance_types: &[
            "c6g.large.search",
            "c6g.xlarge.search",
            "c6g.2xlarge.search",
            "c6g.4xlarge.search",
            "c6g.8xlarge.search",
            "c6g.12xlarge.search",
        ],
        roles: ROLES_WITH_COORDINATOR,
        releases: OfferedReleases::SinceElasticsearch(GRAVITON_SINCE_ELASTICSEARCH),
    },
    CatalogFamily {
        family: "c5",
        instance_types: &[
            "c5.large.search",
            "c5.xlarge.search",
            "c5.2xlarge.search",
            "c5.4xlarge.search",
            "c5.9xlarge.search",
            "c5.18xlarge.search",
        ],
        roles: ROLES_WITH_COORDINATOR,
        releases: OfferedReleases::Every,
    },
    CatalogFamily {
        family: "c4",
        instance_types: &[
            "c4.large.search",
            "c4.xlarge.search",
            "c4.2xlarge.search",
            "c4.4xlarge.search",
            "c4.8xlarge.search",
        ],
        roles: ROLES_DATA_AND_MASTER,
        releases: OfferedReleases::Every,
    },
    // Memory optimized. A `d` suffix names the sub-family whose data sits on storage
    // built into the instance ([`INSTANCE_STORAGE_FAMILIES`]) rather than on an
    // attached volume; it serves the same roles as the family it belongs to.
    CatalogFamily {
        family: "r8g",
        instance_types: &[
            "r8g.medium.search",
            "r8g.large.search",
            "r8g.xlarge.search",
            "r8g.2xlarge.search",
            "r8g.4xlarge.search",
            "r8g.8xlarge.search",
            "r8g.12xlarge.search",
            "r8g.16xlarge.search",
            "r8g.24xlarge.search",
        ],
        roles: ROLES_WITH_COORDINATOR,
        releases: OfferedReleases::SinceOpenSearch(GRAVITON4_SINCE_OPENSEARCH),
    },
    CatalogFamily {
        family: "r8gd",
        instance_types: &[
            "r8gd.medium.search",
            "r8gd.large.search",
            "r8gd.xlarge.search",
            "r8gd.2xlarge.search",
            "r8gd.4xlarge.search",
            "r8gd.8xlarge.search",
            "r8gd.12xlarge.search",
            "r8gd.16xlarge.search",
            "r8gd.24xlarge.search",
        ],
        roles: ROLES_WITH_COORDINATOR,
        releases: OfferedReleases::SinceOpenSearch(GRAVITON4_SINCE_OPENSEARCH),
    },
    CatalogFamily {
        family: "r7gd",
        instance_types: &[
            "r7gd.medium.search",
            "r7gd.large.search",
            "r7gd.xlarge.search",
            "r7gd.2xlarge.search",
            "r7gd.4xlarge.search",
            "r7gd.8xlarge.search",
            "r7gd.12xlarge.search",
            "r7gd.16xlarge.search",
        ],
        roles: ROLES_WITH_COORDINATOR,
        releases: OfferedReleases::SinceElasticsearch(GRAVITON_SINCE_ELASTICSEARCH),
    },
    CatalogFamily {
        family: "r7i",
        instance_types: &[
            "r7i.large.search",
            "r7i.xlarge.search",
            "r7i.2xlarge.search",
            "r7i.4xlarge.search",
            "r7i.8xlarge.search",
            "r7i.12xlarge.search",
            "r7i.16xlarge.search",
            "r7i.24xlarge.search",
        ],
        roles: ROLES_WITH_COORDINATOR,
        releases: OfferedReleases::SinceOpenSearch(SEVENTH_GENERATION_INTEL_SINCE_OPENSEARCH),
    },
    CatalogFamily {
        family: "r6gd",
        instance_types: &[
            "r6gd.large.search",
            "r6gd.xlarge.search",
            "r6gd.2xlarge.search",
            "r6gd.4xlarge.search",
            "r6gd.8xlarge.search",
            "r6gd.12xlarge.search",
            "r6gd.16xlarge.search",
        ],
        roles: ROLES_WITH_COORDINATOR,
        releases: OfferedReleases::SinceElasticsearch(GRAVITON_SINCE_ELASTICSEARCH),
    },
    CatalogFamily {
        family: "r6g",
        instance_types: &[
            "r6g.large.search",
            "r6g.xlarge.search",
            "r6g.2xlarge.search",
            "r6g.4xlarge.search",
            "r6g.8xlarge.search",
            "r6g.12xlarge.search",
        ],
        roles: ROLES_WITH_COORDINATOR,
        releases: OfferedReleases::SinceElasticsearch(GRAVITON_SINCE_ELASTICSEARCH),
    },
    CatalogFamily {
        family: "r5",
        instance_types: &[
            "r5.large.search",
            "r5.xlarge.search",
            "r5.2xlarge.search",
            "r5.4xlarge.search",
            "r5.12xlarge.search",
            "r5.24xlarge.search",
        ],
        roles: ROLES_WITH_COORDINATOR,
        releases: OfferedReleases::Every,
    },
    CatalogFamily {
        family: "r4",
        instance_types: &[
            "r4.large.search",
            "r4.xlarge.search",
            "r4.2xlarge.search",
            "r4.4xlarge.search",
            "r4.8xlarge.search",
            "r4.16xlarge.search",
        ],
        roles: ROLES_DATA_AND_MASTER,
        releases: OfferedReleases::Every,
    },
    CatalogFamily {
        family: "r3",
        instance_types: &[
            "r3.large.search",
            "r3.xlarge.search",
            "r3.2xlarge.search",
            "r3.4xlarge.search",
            "r3.8xlarge.search",
        ],
        roles: ROLES_DATA_AND_MASTER,
        releases: OfferedReleases::Every,
    },
    // Storage optimized: an instance of one holds the domain's data on storage built
    // into it, and a coordinator node holds none, so it is not provisioned on one.
    CatalogFamily {
        family: "i8ge",
        instance_types: &[
            "i8ge.large.search",
            "i8ge.xlarge.search",
            "i8ge.2xlarge.search",
            "i8ge.3xlarge.search",
            "i8ge.6xlarge.search",
            "i8ge.12xlarge.search",
            "i8ge.18xlarge.search",
            "i8ge.24xlarge.search",
        ],
        roles: ROLES_DATA_AND_MASTER,
        releases: OfferedReleases::SinceOpenSearch(GRAVITON4_SINCE_OPENSEARCH),
    },
    CatalogFamily {
        family: "im4gn",
        instance_types: &[
            "im4gn.large.search",
            "im4gn.xlarge.search",
            "im4gn.2xlarge.search",
            "im4gn.4xlarge.search",
            "im4gn.8xlarge.search",
            "im4gn.16xlarge.search",
        ],
        roles: ROLES_DATA_AND_MASTER,
        releases: OfferedReleases::SinceElasticsearch(GRAVITON_SINCE_ELASTICSEARCH),
    },
    CatalogFamily {
        family: "i3",
        instance_types: &[
            "i3.large.search",
            "i3.xlarge.search",
            "i3.2xlarge.search",
            "i3.4xlarge.search",
            "i3.8xlarge.search",
            "i3.16xlarge.search",
        ],
        roles: ROLES_DATA_AND_MASTER,
        releases: OfferedReleases::Every,
    },
    CatalogFamily {
        family: "i2",
        instance_types: &["i2.xlarge.search", "i2.2xlarge.search"],
        roles: ROLES_DATA_AND_MASTER,
        releases: OfferedReleases::Every,
    },
    CatalogFamily {
        family: "d2",
        instance_types: &[
            "d2.xlarge.search",
            "d2.2xlarge.search",
            "d2.4xlarge.search",
            "d2.8xlarge.search",
        ],
        roles: ROLES_DATA_AND_MASTER,
        releases: OfferedReleases::Every,
    },
    // OpenSearch optimized.
    CatalogFamily {
        family: "om2",
        instance_types: &[
            "om2.medium.search",
            "om2.large.search",
            "om2.xlarge.search",
            "om2.2xlarge.search",
            "om2.4xlarge.search",
            "om2.8xlarge.search",
            "om2.12xlarge.search",
            "om2.16xlarge.search",
        ],
        roles: ROLES_DATA_ONLY,
        releases: OfferedReleases::SinceOpenSearch(OPENSEARCH_OPTIMIZED_GEN2_SINCE),
    },
    CatalogFamily {
        family: "or2",
        instance_types: &[
            "or2.medium.search",
            "or2.large.search",
            "or2.xlarge.search",
            "or2.2xlarge.search",
            "or2.4xlarge.search",
            "or2.8xlarge.search",
            "or2.12xlarge.search",
            "or2.16xlarge.search",
        ],
        roles: ROLES_DATA_ONLY,
        releases: OfferedReleases::SinceOpenSearch(OPENSEARCH_OPTIMIZED_GEN2_SINCE),
    },
    CatalogFamily {
        family: "oi2",
        instance_types: &[
            "oi2.medium.search",
            "oi2.large.search",
            "oi2.xlarge.search",
            "oi2.2xlarge.search",
            "oi2.4xlarge.search",
            "oi2.8xlarge.search",
            "oi2.12xlarge.search",
            "oi2.16xlarge.search",
        ],
        roles: ROLES_DATA_ONLY,
        releases: OfferedReleases::SinceOpenSearch(OPENSEARCH_OPTIMIZED_GEN2_SINCE),
    },
    CatalogFamily {
        family: "or1",
        instance_types: &[
            "or1.medium.search",
            "or1.large.search",
            "or1.xlarge.search",
            "or1.2xlarge.search",
            "or1.4xlarge.search",
            "or1.8xlarge.search",
            "or1.12xlarge.search",
            "or1.16xlarge.search",
        ],
        roles: ROLES_DATA_ONLY,
        releases: OfferedReleases::SinceOpenSearch(OPENSEARCH_OPTIMIZED_SINCE),
    },
    // Burstable.
    CatalogFamily {
        family: "t4g",
        instance_types: &["t4g.small.search", "t4g.medium.search"],
        roles: ROLES_DATA_AND_MASTER,
        releases: OfferedReleases::SinceElasticsearch(GRAVITON_SINCE_ELASTICSEARCH),
    },
    CatalogFamily {
        family: "t3",
        instance_types: &[
            "t3.nano.search",
            "t3.micro.search",
            "t3.small.search",
            "t3.medium.search",
            "t3.large.search",
            "t3.xlarge.search",
            "t3.2xlarge.search",
        ],
        roles: ROLES_DATA_AND_MASTER,
        releases: OfferedReleases::Every,
    },
    CatalogFamily {
        family: "t2",
        instance_types: &["t2.micro.search", "t2.small.search", "t2.medium.search"],
        roles: ROLES_DATA_AND_MASTER,
        releases: OfferedReleases::Every,
    },
    // The UltraWarm tier.
    CatalogFamily {
        family: ULTRAWARM_FAMILY,
        instance_types: &[
            "ultrawarm1.medium.search",
            "ultrawarm1.large.search",
            "ultrawarm1.xlarge.search",
        ],
        roles: ROLES_ULTRAWARM_ONLY,
        releases: OfferedReleases::SinceElasticsearch(ULTRAWARM_SINCE_ELASTICSEARCH),
    },
];

/// The family token of an instance type, which is `<family>.<size>.search`; `None` for
/// a value carrying no family at all.
pub fn instance_family(instance_type: &str) -> Option<&str> {
    instance_type
        .split('.')
        .next()
        .filter(|family| !family.is_empty())
}

/// The catalog entry `instance_type` belongs to, and `None` for a value the service
/// publishes no instance type for — a family the catalog does not carry, a size that
/// family is not offered in, or a value that is not `<family>.<size>.search` at all.
pub(super) fn catalog_family(instance_type: &str) -> Option<&'static CatalogFamily> {
    let family = instance_family(instance_type)?;
    INSTANCE_TYPE_CATALOG
        .iter()
        .find(|entry| entry.family == family && entry.instance_types.contains(&instance_type))
}

/// Whether the service publishes `instance_type` at all, whatever the release: the
/// catalog is the record of which instance types exist, so a value it does not carry
/// names no instance type.
pub fn publishes_instance_type(instance_type: &str) -> bool {
    catalog_family(instance_type).is_some()
}

/// Whether `version` offers `instance_type`.
///
/// An instance type is offered when the catalog publishes it and the release asked
/// about is one its family is offered on, so a family the service introduced for a
/// later release is not offered on an earlier one.
pub fn offers_instance_type(version: EngineVersion, instance_type: &str) -> bool {
    catalog_family(instance_type).is_some_and(|entry| entry.offered_on(version))
}

/// Every instance type `version` offers, in the order the catalog is published:
/// family by family in [`catalog_rank`]'s order, each family's instance types smallest
/// size first, less the families this release does not offer.
///
/// The order is the catalog's and is stable, so a paged read resumes where the previous
/// page stopped and element N of the listing is the same instance type on every call.
pub fn offered_instance_types(version: EngineVersion) -> Vec<&'static str> {
    let mut families: Vec<&'static CatalogFamily> = INSTANCE_TYPE_CATALOG
        .iter()
        .filter(|entry| entry.offered_on(version))
        .collect();
    families.sort_by_key(|entry| catalog_rank(entry));
    families
        .into_iter()
        .flat_map(|entry| entry.instance_types.iter().copied())
        .collect()
}

/// Every instance type the service PUBLISHES, whatever the release, in the order the
/// catalog is published: family by family in [`catalog_rank`]'s order, each family's
/// instance types smallest size first.
///
/// The service's own accepted-value list — a value outside it names no instance type at
/// all ([`publishes_instance_type`]) — so it is what a refusal of an unknown instance
/// type enumerates, and it carries the current-generation families the checked-in
/// `OpenSearchPartitionInstanceType` value set (an older snapshot of the same catalog)
/// does not list. [`offered_instance_types`] is the same catalog narrowed to one release;
/// this is the set membership itself, which no release decides.
pub fn published_instance_types() -> Vec<&'static str> {
    let mut families: Vec<&'static CatalogFamily> = INSTANCE_TYPE_CATALOG.iter().collect();
    families.sort_by_key(|entry| catalog_rank(entry));
    families
        .into_iter()
        .flat_map(|entry| entry.instance_types.iter().copied())
        .collect()
}

/// Whether the catalog publishes every value of `OpenSearchPartitionInstanceType`.
///
/// The modelled value set is the model's own record of which instance types exist, so
/// the service's catalog carries all of it and the families the service added since: a
/// value the catalog did not carry would name no instance type at all, and the
/// operations that resolve one against the catalog would refuse it. Held by this
/// service's `instance_type_catalog` suite.
pub fn publishes_every_modelled_instance_type() -> bool {
    OPEN_SEARCH_PARTITION_INSTANCE_TYPE_VALUES
        .iter()
        .all(|instance_type| publishes_instance_type(instance_type))
}

/// The roles an instance of `instance_type` can serve, in the order the catalog
/// publishes them, and NO role for a type the catalog does not publish — which names
/// no instance type, so there is no instance to serve a role.
///
/// The roles are the catalog entry's: the UltraWarm family serves the warm tier, an
/// OpenSearch-optimized instance is a data node only, a general-purpose, compute- or
/// memory-optimized instance can also be a dedicated coordinator node, and every other
/// family is a data node and master-eligible. This is the ONE derivation of them, so
/// the roles the instance-type catalog reports for a type are the roles the limit
/// catalog below publishes limits for.
pub fn instance_roles(instance_type: &str) -> Vec<&'static str> {
    catalog_family(instance_type).map_or_else(Vec::new, |entry| entry.roles.to_vec())
}

/// What `instance_type` supports on `version`, as `InstanceTypeDetails`.
///
/// Each feature member is a conjunction of two things: the engine release the feature
/// is available from, and whether the instance family can serve it at all. The roles
/// are the ones the catalog publishes for the type ([`instance_roles`]), so the entry
/// reports the roles an instance of it can actually be provisioned in.
///
/// `availability_zones` is reported only when the caller asked for it, so a request
/// that did not carries no `AvailabilityZones` member rather than an empty list.
pub fn instance_type_details(
    version: EngineVersion,
    instance_type: &str,
    availability_zones: Option<Vec<String>>,
) -> InstanceTypeDetails {
    let family = instance_family(instance_type).unwrap_or(instance_type);
    let encryption = !UNENCRYPTED_FAMILIES.contains(&family)
        && version.since_elasticsearch(ENCRYPTION_SINCE_ELASTICSEARCH);
    let instance_role = instance_roles(instance_type)
        .into_iter()
        .map(str::to_string)
        .collect();
    InstanceTypeDetails {
        instance_type: Some(instance_type.to_string()),
        encryption_enabled: Some(encryption),
        cognito_enabled: Some(version.since_elasticsearch(COGNITO_SINCE_ELASTICSEARCH)),
        app_logs_enabled: Some(version.since_elasticsearch(APP_LOGS_SINCE_ELASTICSEARCH)),
        // Fine-grained access control requires both encryption features, so it is
        // available only where they are.
        advanced_security_enabled: Some(
            encryption && version.since_elasticsearch(ADVANCED_SECURITY_SINCE_ELASTICSEARCH),
        ),
        warm_enabled: Some(
            version.since_elasticsearch(ULTRAWARM_SINCE_ELASTICSEARCH)
                && !BURSTABLE_FAMILIES.contains(&family),
        ),
        instance_role: Some(instance_role),
        availability_zones,
    }
}

// ---------------------------------------------------------------------------
// The instance-type LIMIT catalog
//
// What an instance type can be scaled to — how many of it a domain holds in each
// role, and what storage attaches to it — is published BY the service alongside
// the instance-type catalog above, so it is DERIVED from the instance type and
// the role rather than stored per account: two reads of one instance type report
// the same limits, and the roles it is reported for are the roles
// [`instance_roles`] gives that type.
// ---------------------------------------------------------------------------

/// `StorageTypeName`'s `ebs`: Elastic Block Store storage attached to the instance. It
/// is the storage type that carries a sub-type and volume limits.
pub const STORAGE_TYPE_EBS: &str = "ebs";

/// `StorageTypeName`'s `instance`: "built-in storage available for the instance", which
/// has no sub-type and no volume limits, because there is no volume to size.
pub const STORAGE_TYPE_INSTANCE: &str = "instance";

/// The key `LimitsByRole` publishes the UltraWarm tier's limits under. That map's own
/// documentation names its three roles `data`, `master` and `ultra_warm`, so the warm
/// key here is NOT [`INSTANCE_ROLE_ULTRAWARM`], which is the value
/// `InstanceTypeDetails$InstanceRole` reports.
pub const LIMITS_ROLE_ULTRAWARM: &str = "ultra_warm";

/// `VolumeType`'s `standard`: a magnetic volume, the one sub-type with its own size cap
/// ([`MAXIMUM_STANDARD_VOLUME_SIZE_GIB`]).
pub const VOLUME_TYPE_STANDARD: &str = "standard";

/// `VolumeType`'s `gp2`: a general-purpose SSD volume whose performance is not
/// provisioned per volume.
pub const VOLUME_TYPE_GP2: &str = "gp2";

/// `VolumeType`'s `gp3`: the general-purpose SSD volume whose IOPS and throughput are
/// provisioned (`EBSOptions$Iops`, `EBSOptions$Throughput`).
pub const VOLUME_TYPE_GP3: &str = "gp3";

/// `VolumeType`'s `io1`: a provisioned-IOPS SSD volume.
pub const VOLUME_TYPE_IO1: &str = "io1";

/// `StorageTypeLimit$LimitName`'s `MinimumVolumeSize`: "minimum volume size that is
/// available for the given storage type".
pub const LIMIT_MINIMUM_VOLUME_SIZE: &str = "MinimumVolumeSize";

/// `StorageTypeLimit$LimitName`'s `MaximumVolumeSize`: "maximum volume size that is
/// available for the given storage type".
pub const LIMIT_MAXIMUM_VOLUME_SIZE: &str = "MaximumVolumeSize";

/// `StorageTypeLimit$LimitName`'s `MinimumIops`.
pub const LIMIT_MINIMUM_IOPS: &str = "MinimumIops";

/// `StorageTypeLimit$LimitName`'s `MaximumIops`.
pub const LIMIT_MAXIMUM_IOPS: &str = "MaximumIops";

/// `StorageTypeLimit$LimitName`'s `MinimumThroughput`.
pub const LIMIT_MINIMUM_THROUGHPUT: &str = "MinimumThroughput";

/// `StorageTypeLimit$LimitName`'s `MaximumThroughput`.
pub const LIMIT_MAXIMUM_THROUGHPUT: &str = "MaximumThroughput";

/// `AdditionalLimit$LimitName`'s `MaximumNumberOfDataNodesSupported`, which "only
/// applies to master nodes and specifies the maximum number of data nodes of a given
/// instance type a master node can support".
pub const LIMIT_DATA_NODES_SUPPORTED: &str = "MaximumNumberOfDataNodesSupported";

/// `AdditionalLimit$LimitName`'s `MaximumNumberOfDataNodesWithoutMasterNode`, which
/// "only applies to data nodes and specifies the maximum number of data nodes of a
/// given instance type can exist without a master node governing them".
pub const LIMIT_DATA_NODES_WITHOUT_MASTER: &str = "MaximumNumberOfDataNodesWithoutMasterNode";

/// The instance families whose data sits on storage built into the instance rather than
/// on an attached volume, so the only storage type they publish is
/// [`STORAGE_TYPE_INSTANCE`].
pub(super) const INSTANCE_STORAGE_FAMILIES: [&str; 9] = [
    "d2", "i2", "i3", "i8ge", "im4gn", "r3", "r6gd", "r7gd", "r8gd",
];

/// The smallest EBS volume, in GiB, every sub-type publishes — the floor a volume size
/// is below rather than simply smaller than, whatever the instance type.
pub const MINIMUM_VOLUME_SIZE_GIB: i64 = 10;

/// The largest magnetic ([`VOLUME_TYPE_STANDARD`]) volume, in GiB: the one sub-type
/// whose maximum is a flat figure rather than one that scales with the instance.
pub const MAXIMUM_STANDARD_VOLUME_SIZE_GIB: i64 = 100;

/// How many GiB of EBS volume ONE instance-size normalization unit carries, which is
/// what makes the maximum volume size scale with the instance: a `large` (four units of
/// [`SIZE_NORMALIZATION_FACTORS`]) publishes 512 GiB, an `xlarge` 1 TiB, and the
/// largest published size, `24xlarge`, 24 TiB.
///
/// Like [`CLASS_HOURLY_RATE_PER_UNIT`] this is the emulator's OWN figure: AWS publishes
/// a table per instance type, which is not part of the API and not in the Smithy model.
/// What it guarantees is the RELATION — the maximum is monotonic in the instance's size
/// and never below [`SMALLEST_MAXIMUM_VOLUME_SIZE_GIB`].
pub(super) const VOLUME_SIZE_GIB_PER_UNIT: f64 = 128.0;

/// The smallest maximum any instance type publishes, in GiB: a volume of this size
/// attaches to every published instance type, whatever its size, and only above it does
/// the scale differentiate one instance from another. It is what keeps the published
/// maximum of a SMALL instance from being the narrower of the two bounds a caller is
/// held to — the service's own maximum for a small type is not below this — so the
/// catalog never reports a volume as unavailable that the service would attach.
pub(super) const SMALLEST_MAXIMUM_VOLUME_SIZE_GIB: i64 = 512;

/// The IOPS range a [`VOLUME_TYPE_GP3`] volume publishes: its 3,000 IOPS baseline is
/// the floor, and 16,000 the most one volume can be provisioned with.
pub(super) const GP3_IOPS_RANGE: (i64, i64) = (3_000, 16_000);

/// The same for a [`VOLUME_TYPE_IO1`] volume, which is provisioned from 1,000 IOPS.
pub(super) const IO1_IOPS_RANGE: (i64, i64) = (1_000, 16_000);

/// The throughput range, in MiB/s, a [`VOLUME_TYPE_GP3`] volume publishes: a 125 MiB/s
/// baseline up to 1,000 MiB/s.
pub(super) const GP3_THROUGHPUT_RANGE_MIB: (i64, i64) = (125, 1_000);

/// The fewest data nodes a domain runs: a single node is a whole domain.
pub const MINIMUM_DATA_NODE_COUNT: i32 = 1;

/// The most data nodes the limit catalog publishes for the data role — the
/// `MaximumInstanceCount` `DescribeInstanceTypeLimits` reports under it.
///
/// Like [`DOMAIN_QUOTA`] this is the emulator's OWN published figure rather than one the
/// model carries, and it is ONE figure for every instance type: the service publishes a
/// per-instance-type table, which is not part of the API and not in the model, so there
/// is nothing to read a per-type ceiling from. What this fixes is that the data-node
/// count is bounded and that the bound is above the master one and below the warm one.
pub const MAXIMUM_DATA_NODE_COUNT: i32 = 20;

/// The dedicated-master count a domain runs: a master quorum needs more than one node,
/// and an odd count above five buys nothing, so five is the top.
pub const MASTER_NODE_COUNT_RANGE: ::std::ops::RangeInclusive<i32> = 2..=5;

/// The UltraWarm node count a domain runs: the warm tier is attached in a pair or more,
/// and like [`MAXIMUM_DATA_NODE_COUNT`] its top is the emulator's own bound.
pub const WARM_NODE_COUNT_RANGE: ::std::ops::RangeInclusive<i32> = 2..=150;

/// How many data nodes a domain runs before a dedicated master node is needed to govern
/// them — `MaximumNumberOfDataNodesWithoutMasterNode`.
pub const DATA_NODES_WITHOUT_MASTER: i32 = 10;

/// How many data nodes a dedicated master supports, by the size of the master instance:
/// each row is the largest [`SIZE_NORMALIZATION_FACTORS`] factor it covers, with the
/// count. A bigger master governs more data nodes, which is the relation this table
/// fixes; the counts themselves are the emulator's own published figures.
pub(super) const DATA_NODES_SUPPORTED_BY_MASTER: [(f64, i32); 4] = [
    // Up to and including `large`.
    (4.0, 10),
    // `xlarge`.
    (8.0, 30),
    // `2xlarge`.
    (16.0, 75),
    // Every larger size.
    (f64::INFINITY, 125),
];

/// What ONE instance's EBS attachment carries: the largest volume that attaches to it,
/// the most IOPS it drives and the most throughput it sustains.
///
/// These are bounds of the INSTANCE, not of the volume, and a limit the catalog publishes
/// for a pairing is the LESSER of the two — a volume sub-type's own ceiling is reported
/// only where the instance it attaches to can reach it.
#[derive(Debug, Clone, Copy)]
pub(super) struct EbsAttachment {
    /// The largest volume, in GiB, one instance of the class takes.
    pub(super) maximum_volume_size_gib: i64,
    /// The most IOPS one instance of the class drives.
    pub(super) maximum_iops: i64,
    /// The most throughput, in MiB/s, one instance of the class sustains.
    pub(super) maximum_throughput_mib: i64,
}

/// The attachment a BURSTABLE instance publishes.
///
/// A burstable instance has no dedicated EBS bandwidth: its attachment bursts from a
/// credit balance, and the figures AWS publishes for that class — a burst of 11,800 IOPS
/// and 260 MiB/s, with a 100 GiB volume ceiling — are every one of them below what the
/// general-purpose volume sub-types publish on their own. That is what makes the
/// burstable instance, rather than the volume, the binding bound, and why it is the one
/// row of this table: every other class reaches the sub-type's own ceilings.
pub(super) const BURSTABLE_EBS_ATTACHMENT: EbsAttachment = EbsAttachment {
    maximum_volume_size_gib: 100,
    maximum_iops: 11_800,
    maximum_throughput_mib: 260,
};

/// The EBS attachment `instance_type` publishes, and `None` for a class whose attachment
/// reaches every ceiling the volume sub-types publish, so nothing of the instance's own
/// narrows them.
pub(super) fn ebs_attachment(instance_type: &str) -> Option<EbsAttachment> {
    let family = instance_family(instance_type).unwrap_or(instance_type);
    BURSTABLE_FAMILIES
        .contains(&family)
        .then_some(BURSTABLE_EBS_ATTACHMENT)
}

/// The IOPS range `instance_type` publishes for `sub_type`, and `None` for a sub-type
/// whose performance is not provisioned per volume: `MinimumIops`/`MaximumIops` "can be
/// empty if not applicable", and `EBSOptions$Iops` is "applicable only for the gp3 and
/// provisioned IOPS EBS volume types".
///
/// The ceiling is the lesser of the sub-type's own and what the instance drives; the floor
/// is the sub-type's, and every published attachment drives more than it, so the range
/// never inverts.
pub(super) fn iops_range(instance_type: &str, sub_type: &str) -> Option<(i64, i64)> {
    let (minimum, maximum) = match sub_type {
        VOLUME_TYPE_GP3 => GP3_IOPS_RANGE,
        VOLUME_TYPE_IO1 => IO1_IOPS_RANGE,
        _ => return None,
    };
    let maximum = ebs_attachment(instance_type)
        .map_or(maximum, |attachment| maximum.min(attachment.maximum_iops));
    Some((minimum, maximum.max(minimum)))
}

/// The throughput range `instance_type` publishes for `sub_type`, bounded by the
/// instance's attachment the same way. `EBSOptions$Throughput` is "applicable only for
/// the gp3 volume type", so every other sub-type publishes none.
pub(super) fn throughput_range(instance_type: &str, sub_type: &str) -> Option<(i64, i64)> {
    let (minimum, maximum) = match sub_type {
        VOLUME_TYPE_GP3 => GP3_THROUGHPUT_RANGE_MIB,
        _ => return None,
    };
    let maximum = ebs_attachment(instance_type).map_or(maximum, |attachment| {
        maximum.min(attachment.maximum_throughput_mib)
    });
    Some((minimum, maximum.max(minimum)))
}

/// The largest EBS volume, in GiB, `instance_type` publishes for `sub_type`: the
/// instance's size scaled by [`VOLUME_SIZE_GIB_PER_UNIT`], never below
/// [`SMALLEST_MAXIMUM_VOLUME_SIZE_GIB`], capped by what the instance's own attachment
/// takes ([`ebs_attachment`]), and capped for a magnetic volume at
/// [`MAXIMUM_STANDARD_VOLUME_SIZE_GIB`].
pub fn maximum_volume_size(instance_type: &str, sub_type: &str) -> i64 {
    let scaled = size_normalization_factor(instance_type)
        .map_or(SMALLEST_MAXIMUM_VOLUME_SIZE_GIB, |factor| {
            (factor * VOLUME_SIZE_GIB_PER_UNIT).round() as i64
        })
        .max(SMALLEST_MAXIMUM_VOLUME_SIZE_GIB);
    let scaled = ebs_attachment(instance_type).map_or(scaled, |attachment| {
        scaled.min(attachment.maximum_volume_size_gib)
    });
    if sub_type == VOLUME_TYPE_STANDARD {
        scaled.min(MAXIMUM_STANDARD_VOLUME_SIZE_GIB)
    } else {
        scaled
    }
}

/// One `StorageTypeLimit`. Every limit value is a string on the wire
/// (`StorageTypeLimit$LimitValues`), so a published number is rendered here, once.
pub(super) fn storage_type_limit(name: &str, value: i64) -> StorageTypeLimit {
    StorageTypeLimit {
        limit_name: Some(name.to_string()),
        limit_values: Some(vec![value.to_string()]),
    }
}

/// One `AdditionalLimit`, rendered the same way.
pub(super) fn additional_limit(name: &str, value: i32) -> AdditionalLimit {
    AdditionalLimit {
        limit_name: Some(name.to_string()),
        limit_values: Some(vec![value.to_string()]),
    }
}

/// The `ebs` storage type `instance_type` publishes for one volume sub-type: the size
/// range every sub-type has, plus the IOPS and throughput ranges of the sub-types those
/// are provisioned for.
///
/// Each bound PAIR is published ceiling before floor — `MaximumVolumeSize` before
/// `MinimumVolumeSize`, `MaximumIops` before `MinimumIops`, `MaximumThroughput` before
/// `MinimumThroughput` — the order the service publishes a storage limit in, and the
/// order `StorageTypeLimit$LimitName`'s own documentation lists the IOPS and the
/// throughput pair in. A caller reading the block positionally therefore reads the
/// ceiling first, which is what makes the pair's order part of the answer rather than a
/// detail of how it is assembled.
pub(super) fn ebs_storage_type(instance_type: &str, sub_type: &str) -> StorageType {
    let mut limits = vec![
        storage_type_limit(
            LIMIT_MAXIMUM_VOLUME_SIZE,
            maximum_volume_size(instance_type, sub_type),
        ),
        storage_type_limit(LIMIT_MINIMUM_VOLUME_SIZE, MINIMUM_VOLUME_SIZE_GIB),
    ];
    if let Some((minimum, maximum)) = iops_range(instance_type, sub_type) {
        limits.push(storage_type_limit(LIMIT_MAXIMUM_IOPS, maximum));
        limits.push(storage_type_limit(LIMIT_MINIMUM_IOPS, minimum));
    }
    if let Some((minimum, maximum)) = throughput_range(instance_type, sub_type) {
        limits.push(storage_type_limit(LIMIT_MAXIMUM_THROUGHPUT, maximum));
        limits.push(storage_type_limit(LIMIT_MINIMUM_THROUGHPUT, minimum));
    }
    StorageType {
        storage_type_name: Some(STORAGE_TYPE_EBS.to_string()),
        storage_sub_type_name: Some(sub_type.to_string()),
        storage_type_limits: Some(limits),
    }
}

/// The volume sub-types EVERY instance class with an EBS attachment publishes: the
/// general-purpose SSD types, `VolumeType`'s `gp2` and the `gp3` whose IOPS and
/// throughput are provisioned on it.
pub(super) const GENERAL_PURPOSE_VOLUME_TYPES: [&str; 2] = [VOLUME_TYPE_GP2, VOLUME_TYPE_GP3];

/// The instance families the MAGNETIC sub-type is published for: magnetic storage is
/// offered for the previous-generation instance types alone, so every later family
/// publishes none.
///
/// Which families are previous generation is published BY the service and carried by no
/// part of the model, so this is the emulator's statement of it. The families whose data
/// sits on built-in storage are not among it: they publish no volume at all
/// ([`INSTANCE_STORAGE_FAMILIES`]).
pub(super) const PREVIOUS_GENERATION_EBS_FAMILIES: [&str; 4] = ["c4", "m3", "m4", "r4"];

/// The `VolumeType` sub-types `instance_type` publishes an `ebs` storage type for, in the
/// order the generated enum lists them.
///
/// The general-purpose SSD types attach to every class. A PROVISIONED-IOPS volume
/// provisions performance the instance itself has to carry, so it is published for every
/// class whose attachment provisions its own — which a burstable instance's, bursting
/// from a credit balance ([`BURSTABLE_EBS_ATTACHMENT`]), does not. The magnetic type is
/// published for the previous-generation families alone.
pub(super) fn volume_sub_types(instance_type: &str) -> Vec<&'static str> {
    let family = instance_family(instance_type).unwrap_or(instance_type);
    let burstable = BURSTABLE_FAMILIES.contains(&family);
    let previous_generation = PREVIOUS_GENERATION_EBS_FAMILIES.contains(&family);
    VOLUME_TYPE_VALUES
        .iter()
        .copied()
        .filter(|sub_type| match *sub_type {
            VOLUME_TYPE_STANDARD => previous_generation,
            VOLUME_TYPE_IO1 => !burstable,
            _ => GENERAL_PURPOSE_VOLUME_TYPES.contains(sub_type),
        })
        .collect()
}

/// The storage types `instance_type` publishes for the role that holds the domain's
/// DATA.
///
/// A family whose data sits on storage built into the instance publishes that one type
/// and no volume limits; every other family publishes one `ebs` entry per volume sub-type
/// THAT instance type attaches ([`volume_sub_types`]), in the order the enum lists them.
/// The warm tier's storage is not attached to the node at all, so an UltraWarm instance
/// type publishes no storage type rather than an empty list of them.
pub(super) fn storage_types(instance_type: &str) -> Option<Vec<StorageType>> {
    let family = instance_family(instance_type).unwrap_or(instance_type);
    if family == ULTRAWARM_FAMILY {
        return None;
    }
    if INSTANCE_STORAGE_FAMILIES.contains(&family) {
        return Some(vec![StorageType {
            storage_type_name: Some(STORAGE_TYPE_INSTANCE.to_string()),
            storage_sub_type_name: None,
            storage_type_limits: None,
        }]);
    }
    Some(
        volume_sub_types(instance_type)
            .into_iter()
            .map(|sub_type| ebs_storage_type(instance_type, sub_type))
            .collect(),
    )
}

/// How many data nodes a dedicated master of `instance_type` supports, from
/// [`DATA_NODES_SUPPORTED_BY_MASTER`] — whose last row covers every size, so the
/// fallback is reached only by a value carrying no size token at all.
pub(super) fn data_nodes_supported_by_master(instance_type: &str) -> i32 {
    let factor = size_normalization_factor(instance_type).unwrap_or_default();
    DATA_NODES_SUPPORTED_BY_MASTER
        .iter()
        .find(|(largest, _)| factor <= *largest)
        .map_or(DATA_NODES_WITHOUT_MASTER, |(_, nodes)| *nodes)
}

/// The limits `instance_type` publishes for ONE of its roles: how many instances of it
/// a domain holds in that role, the storage it can attach, and the additional limit
/// that applies to that role.
///
/// Storage is published for the role that HOLDS the domain's data and for no other: an
/// EBS volume "specifies the type of EBS volumes attached to data nodes"
/// (`EBSOptions$VolumeType`), so a dedicated master — which holds no data and whose
/// configuration carries no volume — publishes no storage type rather than a copy of
/// the data role's, exactly as the warm tier publishes none.
pub(super) fn role_limits(instance_type: &str, role: &str) -> Limits {
    let (minimum, maximum, additional) = match role {
        INSTANCE_ROLE_MASTER => (
            *MASTER_NODE_COUNT_RANGE.start(),
            *MASTER_NODE_COUNT_RANGE.end(),
            vec![additional_limit(
                LIMIT_DATA_NODES_SUPPORTED,
                data_nodes_supported_by_master(instance_type),
            )],
        ),
        INSTANCE_ROLE_ULTRAWARM => (
            *WARM_NODE_COUNT_RANGE.start(),
            *WARM_NODE_COUNT_RANGE.end(),
            // Neither published additional limit applies to a warm node: one is a
            // master-node limit and the other a data-node one.
            Vec::new(),
        ),
        _ => (
            MINIMUM_DATA_NODE_COUNT,
            MAXIMUM_DATA_NODE_COUNT,
            vec![additional_limit(
                LIMIT_DATA_NODES_WITHOUT_MASTER,
                DATA_NODES_WITHOUT_MASTER,
            )],
        ),
    };
    Limits {
        // Only the data role attaches storage; every other role publishes none.
        storage_types: match role {
            INSTANCE_ROLE_DATA => storage_types(instance_type),
            _ => None,
        },
        instance_limits: Some(InstanceLimits {
            instance_count_limits: Some(InstanceCountLimits {
                minimum_instance_count: minimum,
                maximum_instance_count: maximum,
            }),
        }),
        // A role no published additional limit applies to carries no `AdditionalLimits`
        // member rather than an empty list.
        additional_limits: (!additional.is_empty()).then_some(additional),
    }
}

/// Every limit `instance_type` publishes, keyed by the role it applies to: the
/// `LimitsByRole` map of `DescribeInstanceTypeLimits`.
///
/// That map's own documentation names the three roles it is keyed by — `data`,
/// `master` and `ultra_warm` — so a role outside them publishes no entry: a dedicated
/// coordinator node holds no data and is scaled with the cluster's request load rather
/// than by a published per-type limit, so the catalog reports the role on the instance
/// type ([`instance_roles`]) and no limit block under it.
pub fn instance_type_limits(
    instance_type: &str,
) -> ::std::collections::HashMap<String, Limits> {
    instance_roles(instance_type)
        .into_iter()
        .filter(|role| *role != INSTANCE_ROLE_COORDINATOR)
        .map(|role| {
            let key = if role == INSTANCE_ROLE_ULTRAWARM {
                LIMITS_ROLE_ULTRAWARM
            } else {
                role
            };
            (key.to_string(), role_limits(instance_type, role))
        })
        .collect()
}

/// Whether `ebs` sizes a volume the instance type publishes, and the refusal naming the
/// bounds it publishes otherwise.
///
/// A volume size is one the SERVICE publishes for that instance type and volume
/// sub-type — [`MINIMUM_VOLUME_SIZE_GIB`] up to [`maximum_volume_size`], the
/// `MinimumVolumeSize`/`MaximumVolumeSize` pair `DescribeInstanceTypeLimits` reports
/// for the pairing — so a size outside it asks for storage the service does not offer
/// for that instance type at all. That is a STORAGE LIMIT of the instance type and not
/// a malformed request, so the refusal is the modelled `LimitExceededException`, which
/// the model classifies `client` with HTTP 409 — the class a caller branches on to tell
/// a capacity condition from a bad request.
///
/// The sentence names the bounds, the instance type the volume would attach to and the
/// engine release the domain runs, each taken from the request (or, on an update, from
/// the merged record): the pairing the bounds are published FOR. Options with no volume
/// (EBS not enabled) and options carrying no size at all size nothing, so there is
/// nothing to check; without an instance type there is no instance to size the volume
/// against, and only the floor every instance type publishes applies, so the sentence
/// names that bound alone.
pub fn ebs_volume_size_refusal(
    ebs: Option<&EBSOptions>,
    instance_type: Option<&str>,
    engine_version: &str,
) -> Option<LimitExceededException> {
    let ebs = ebs?;
    if !ebs.ebs_enabled.unwrap_or(false) {
        return None;
    }
    let size = i64::from(ebs.volume_size?);
    // `EBSOptions$VolumeType` "specifies the type of EBS volumes attached to data
    // nodes"; the general-purpose SSD type is what a request that names none gets, and
    // it is not the magnetic type, whose cap is its own.
    let sub_type = ebs.volume_type.as_deref().unwrap_or(VOLUME_TYPE_GP2);
    let maximum = instance_type.map(|instance_type| maximum_volume_size(instance_type, sub_type));
    if size >= MINIMUM_VOLUME_SIZE_GIB && maximum.is_none_or(|maximum| size <= maximum) {
        return None;
    }
    let message = match (instance_type, maximum) {
        (Some(instance_type), Some(maximum)) => format!(
            "Volume size must be between {MINIMUM_VOLUME_SIZE_GIB} and {maximum} for \
             {instance_type} instance type and version {engine_version}"
        ),
        // A request naming no instance type pairs the volume with no instance, so the
        // only bound published for it is the floor, and the sentence names no instance
        // type it has no value for.
        _ => format!(
            "Volume size must be at least {MINIMUM_VOLUME_SIZE_GIB} for version \
             {engine_version}"
        ),
    };
    Some(LimitExceededException {
        message: Some(message),
    })
}

/// How many Availability Zones of a Region the instance-type catalog reports an
/// instance type as offered in: the most zones a domain can be spread across, which
/// is the top of [`ZONE_COUNT_RANGE`] (`ZoneAwarenessConfig$AvailabilityZoneCount`:
/// "Valid values are `2` and `3`").
pub const CATALOG_AVAILABILITY_ZONES: usize = 3;

/// The Availability Zones of `region` an instance type is offered in: the Region's
/// first [`CATALOG_AVAILABILITY_ZONES`] zones, named by [`zone_name`] so a zone has
/// one name across this service.
pub fn region_availability_zones(region: &str) -> Vec<String> {
    (0..CATALOG_AVAILABILITY_ZONES)
        .filter_map(|index| zone_name(region, index))
        .collect()
}

// ---------------------------------------------------------------------------
// Domain configuration a domain cannot be created or updated with
//
// A configuration the SERVICE does not provision is refused rather than stored:
// each check below reads one documented dependency between members of a domain's
// configuration and states the refusal the create and the update paths share, so
// neither path can accept a configuration the other refuses.
// ---------------------------------------------------------------------------

/// The shortest master password the service takes: `MasterUserPassword`'s own
/// `@length` floor, refused at the boundary before any handler runs.
pub const MASTER_USER_PASSWORD_MINIMUM: usize = 8;

/// The character classes a master password has to draw on, stated as the service
/// states them: the sentence a refusal carries, and the list the checks below walk.
///
/// The LENGTH floor is the model's own (`MasterUserPassword` has `@length` min 8), so
/// it is refused at the boundary before any handler runs and is not restated here.
pub(super) const MASTER_USER_PASSWORD_REQUIREMENT: &str =
    "The master user password must contain at least one uppercase letter, one lowercase letter, \
     one number, and one special character.";

/// Whether `options` carries a master password that meets the service's published
/// complexity requirement, and the refusal stating that requirement otherwise.
///
/// A master user password is the credential of the internal user database, and the
/// service publishes what one has to draw on: an uppercase letter, a lowercase letter,
/// a digit and one character that is none of those — so a password missing any of them
/// is a credential the service would not have accepted, and the request is refused
/// rather than a domain being created or changed around it. A request that carries no
/// password asks for none, so there is nothing to check.
///
/// The refusal states the REQUIREMENT and never the value: `MasterUserPassword` is
/// `@sensitive`, so neither the password nor any part of it is echoed.
pub fn master_user_password_refusal(
    options: Option<&AdvancedSecurityOptionsInput>,
) -> Option<ValidationException> {
    let password = options?
        .master_user_options
        .as_ref()?
        .master_user_password
        .as_deref()?;
    let met = password.chars().any(char::is_uppercase)
        && password.chars().any(char::is_lowercase)
        && password.chars().any(|character| character.is_ascii_digit())
        && password.chars().any(|character| !character.is_alphanumeric());
    if met {
        return None;
    }
    Some(ValidationException {
        message: Some(MASTER_USER_PASSWORD_REQUIREMENT.to_string()),
    })
}

/// Whether `cluster` rests its cold tier on a warm one, and the refusal stating that
/// link otherwise.
///
/// The tiers of a domain form a chain, each level resting on the one below it:
/// `ColdStorageOptions$Enabled` is documented "you must enable UltraWarm storage to
/// enable cold storage". A cluster that asks for the cold tier without the warm one
/// describes a tiering the service cannot provision, so it is refused rather than
/// stored, and the refusal names the link that is missing rather than the chain as a
/// whole: a caller fixes the level it actually left out.
///
/// This link is refused with the operation's `BaseException` ("an error occurred while
/// processing the request", HTTP 400) — the error the service answers it with — and
/// the warm link below with the operation's `ValidationException`, so a caller tells
/// the two apart by code as well as by sentence.
pub fn cold_storage_refusal(cluster: Option<&ClusterConfig>) -> Option<BaseException> {
    let cluster = cluster?;
    let cold = cluster
        .cold_storage_options
        .as_ref()
        .is_some_and(|cold| cold.enabled);
    if !cold || cluster.warm_enabled.unwrap_or(false) {
        return None;
    }
    Some(BaseException {
        message: Some("To use cold storage, you must have UltraWarm enabled.".to_string()),
    })
}

/// Whether `cluster` rests its warm tier on the master nodes that manage it, and the
/// refusal stating that link otherwise.
///
/// The warm nodes of a domain are managed by the cluster's master nodes, so the warm
/// tier is offered only on a cluster whose masters are DEDICATED: a cluster that asks
/// for warm storage without them is refused rather than stored. The next link up is
/// [`cold_storage_refusal`].
pub fn warm_storage_refusal(cluster: Option<&ClusterConfig>) -> Option<ValidationException> {
    let cluster = cluster?;
    if !cluster.warm_enabled.unwrap_or(false) || cluster.dedicated_master_enabled.unwrap_or(false) {
        return None;
    }
    Some(ValidationException {
        message: Some(
            "To use warm storage, your domain must have dedicated master nodes.".to_string(),
        ),
    })
}

/// Whether every log-publishing option in `options` names the destination its logs go
/// to, and the refusal naming the first log type that does not otherwise.
///
/// A `LogPublishingOption` is "whether the log should be published" plus "the ARN of
/// the CloudWatch Logs group to publish logs to": an option that is enabled and names
/// no group publishes to nowhere, so it is refused rather than stored as a
/// configuration that silently does nothing. Naming a group means carrying the ARN OF
/// ONE — a value that is not a log group's ARN names no destination either
/// ([`super::log_destinations::names_a_log_group`]). A disabled option publishes
/// nothing and needs no destination.
///
/// Whether the group that ARN names exists is the next link up
/// ([`super::log_destinations::log_publishing_destination_refusal`]).
///
/// The log types are stated in order, so one request with two offending options is
/// refused with the same sentence however the map was iterated.
pub fn log_publishing_refusal<'a>(
    options: impl IntoIterator<Item = (&'a String, &'a LogPublishingOption)>,
) -> Option<ValidationException> {
    let mut offending: Vec<&str> = options
        .into_iter()
        .filter(|(_, option)| {
            option.enabled.unwrap_or(false)
                && option
                    .cloud_watch_logs_log_group_arn
                    .as_deref()
                    .is_none_or(|arn| !super::log_destinations::names_a_log_group(arn))
        })
        .map(|(log_type, _)| log_type.as_str())
        .collect();
    offending.sort_unstable();
    let log_type = offending.first()?;
    Some(ValidationException {
        message: Some(format!(
            "Log publishing option is misconfigured. Enter valid CloudWatch Logs Arn and Enable \
             flag for LogType \"{log_type}\""
        )),
    })
}

/// Whether `options` leaves the domain's off-peak window enabled, and the refusal
/// naming the restriction otherwise.
///
/// `OffPeakWindowOptions$Enabled` is documented "only available when modifying a
/// domain created prior to February 16, 2023, not when creating a new domain. All
/// domains created after this date have the off-peak window enabled by default. You
/// can't disable the off-peak window after it's enabled for a domain." Every domain
/// this emulator holds is created now, so its window is enabled from the create on and
/// a request that asks for it disabled — at create or at update — is refused rather
/// than leaving a domain with no window for the service's mandatory configuration
/// changes.
pub fn off_peak_window_refusal(
    options: Option<&OffPeakWindowOptions>,
) -> Option<ValidationException> {
    if options?.enabled != Some(false) {
        return None;
    }
    Some(ValidationException {
        message: Some(
            "An off-peak window is required for a domain and cannot be disabled.".to_string(),
        ),
    })
}

/// The `ValidationException` payload for a member that must carry an engine version
/// and does not. It names the member and the value that could not be read as one, so
/// the caller can tell which input it was.
pub fn malformed_engine_version(member: &str, value: &str) -> ValidationException {
    ValidationException {
        message: Some(format!(
            "{member} must be Elasticsearch_X.Y or OpenSearch_X.Y: {value}"
        )),
    }
}

/// The `ValidationException` payload for a member naming an engine version the
/// service does not offer: the value exactly as the request carried it, and the
/// catalog it was resolved against — the same set `ListVersions` advertises, so a
/// caller is told which releases it could have asked for and the two catalog answers
/// cannot disagree.
///
/// The catalog is listed in the order it is PUBLISHED in
/// ([`published_engine_versions`], newest release first), which is the order a caller
/// reads it in everywhere else this service hands it over.
pub fn unsupported_engine_version(value: &str) -> ValidationException {
    ValidationException {
        message: Some(format!(
            "Unsupported Version: {value}, allowed Versions are: [{}]",
            published_engine_versions().join(", ")
        )),
    }
}

/// The engine release `value` names, for an operation that reports what the service
/// offers FOR a named release.
///
/// Two things have to hold, and this is the ONE statement of both: the value has to
/// be readable as a version at all ([`parse_engine_version`]), and it has to resolve
/// against the published catalog ([`offers_engine_version`]). Nothing is offered for
/// a release the service does not have, so a value that resolves to none of them is
/// refused rather than answered for an engine nothing runs.
pub fn resolve_engine_version(
    member: &str,
    value: &str,
) -> Result<EngineVersion, ValidationException> {
    let version =
        parse_engine_version(value).ok_or_else(|| malformed_engine_version(member, value))?;
    if !offers_engine_version(value) {
        return Err(unsupported_engine_version(value));
    }
    Ok(version)
}

/// The `ValidationException` payload for a member that must name an instance type the
/// service publishes and does not.
///
/// `InstanceTypeString`'s pattern `^.*\..*\.search$` admits any value of that shape,
/// so a value outside `OpenSearchPartitionInstanceType` — the model's record of which
/// instance types exist — names no instance type at all, which is a malformed request
/// rather than a listing with nothing in it.
pub fn unknown_instance_type(member: &str, value: &str) -> ValidationException {
    ValidationException {
        message: Some(format!("{member} is not a known instance type: {value}")),
    }
}

/// The modelled refusal for an instance type the engine release in play does not
/// offer: `InvalidTypeException`, "an exception for trying to create or access a
/// sub-resource that's either invalid or not supported", which the model classifies
/// `client` with HTTP 409.
///
/// The sentence names the INSTANCE TYPE alone, taken from the request: the engine
/// release is what the pairing is refused for, not part of the text. This is the ONE
/// construction of it, so every operation that refuses such a pairing words the
/// refusal identically.
pub fn invalid_instance_type(instance_type: &str) -> InvalidTypeException {
    InvalidTypeException {
        message: Some(format!("Invalid instance type: {instance_type}")),
    }
}

/// The refusal for `instance_type` when `version` does not offer it, or `None` when
/// the pairing is one the service offers (and for a request naming no type, which
/// pairs nothing with the release).
///
/// An instance family introduced for a later engine release cannot run an earlier one
/// ([`offers_instance_type`]), so the pairing names a cluster the service cannot
/// provision.
pub fn unoffered_instance_type_refusal(
    version: EngineVersion,
    instance_type: Option<&str>,
) -> Option<InvalidTypeException> {
    let instance_type = instance_type?;
    (!offers_instance_type(version, instance_type)).then(|| invalid_instance_type(instance_type))
}

/// Every instance type `cluster` provisions a node on, in the order the cluster
/// configuration declares them: the data nodes' type, the dedicated master type when
/// dedicated masters are enabled, the warm type when warm storage is enabled, and the
/// type of each enabled node option.
///
/// A type named beside a tier that is turned OFF provisions no node, so it is not a
/// type the cluster runs: `DedicatedMasterType` goes with "indicates whether dedicated
/// master nodes are enabled for the cluster", `WarmType` with "whether to enable warm
/// storage for the cluster", and a `NodeConfig`'s type with "whether a specific node
/// type is active or inactive".
pub(super) fn provisioned_instance_types(cluster: &ClusterConfig) -> Vec<&str> {
    let mut types: Vec<&str> = Vec::new();
    types.extend(cluster.instance_type.as_deref());
    if cluster.dedicated_master_enabled.unwrap_or(false) {
        types.extend(cluster.dedicated_master_type.as_deref());
    }
    if cluster.warm_enabled.unwrap_or(false) {
        types.extend(cluster.warm_type.as_deref());
    }
    for option in cluster.node_options.iter().flatten() {
        if let Some(config) = option.node_config.as_ref() {
            if config.enabled.unwrap_or(false) {
                types.extend(config.r#type.as_deref());
            }
        }
    }
    types
}

/// The refusal for the first instance type `cluster` provisions a node on that
/// `version` does not offer, or `None` when the release offers every one of them.
///
/// A domain runs ONE engine release, so every node of its cluster runs on an instance
/// type that release offers — the data nodes' type and the type of every other tier
/// alike.
pub fn cluster_instance_type_refusal(
    version: EngineVersion,
    cluster: &ClusterConfig,
) -> Option<InvalidTypeException> {
    provisioned_instance_types(cluster)
        .into_iter()
        .find(|instance_type| !offers_instance_type(version, instance_type))
        .map(invalid_instance_type)
}
