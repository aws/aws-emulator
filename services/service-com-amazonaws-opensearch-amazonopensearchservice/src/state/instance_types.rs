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

/// The Elasticsearch release the UltraWarm instance family is offered from: the warm
/// tier's own hardware arrived with it, so no earlier release publishes a type of that
/// family at all.
pub(super) const ULTRAWARM_SINCE_ELASTICSEARCH: (u32, u32) = (6, 8);

/// The Elasticsearch release the Graviton instance families are offered from.
pub(super) const GRAVITON_SINCE_ELASTICSEARCH: (u32, u32) = (7, 9);

/// The Elasticsearch release the FOURTH generation of the storage-optimized line is
/// offered from — the NVMe-backed families of that generation, whose Graviton half
/// (`im4gn`) and Intel half (`i4i`) the service offers alongside each other, so one gate
/// states the generation rather than one per half.
///
/// Its value is the release the Graviton families are gated on
/// ([`GRAVITON_SINCE_ELASTICSEARCH`]) and is stated apart from it because the generation,
/// not the processor, is what it gates. Like every other generation gate here the release
/// itself is the emulator's own statement of published content; what it fixes is that a
/// release predating the generation offers neither half of it.
pub(super) const NVME_STORAGE_GEN4_SINCE_ELASTICSEARCH: (u32, u32) = GRAVITON_SINCE_ELASTICSEARCH;

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

/// Whether the instance family of `instance_type` is one both encryption features are
/// available on at all — the hardware half of what
/// `InstanceTypeDetails$EncryptionEnabled` reports ("whether encryption at rest and
/// node-to-node encryption are supported for the instance type").
///
/// Read off the family ([`UNENCRYPTED_FAMILIES`]) and off nothing else, so the answer is
/// the same for every size of a family. [`offers_encryption_on`] is the whole answer: the
/// features also need an engine that has them.
pub fn offers_encryption(instance_type: &str) -> bool {
    let family = instance_family(instance_type).unwrap_or(instance_type);
    !UNENCRYPTED_FAMILIES.contains(&family)
}

/// The Elasticsearch release BOTH features `InstanceTypeDetails$EncryptionEnabled`
/// reports are available from.
///
/// AWS documents encryption at rest as requiring OpenSearch or Elasticsearch 5.1 or
/// later and node-to-node encryption as requiring 6.0 or later; the member reports the
/// two TOGETHER, and a release has the pair only once it has each of them, so the gate
/// is the later of the two — node-to-node encryption's.
pub(super) const ENCRYPTION_SINCE_ELASTICSEARCH: (u32, u32) = (6, 0);

/// Whether `version` offers both encryption features on `instance_type` — the whole of
/// what `InstanceTypeDetails$EncryptionEnabled` reports.
///
/// Encryption at rest and node-to-node encryption each need BOTH halves of the pairing a
/// listing is about: hardware that provides them ([`offers_encryption`]) and an engine
/// release that has them ([`ENCRYPTION_SINCE_ELASTICSEARCH`]). A release older than the
/// one a feature arrived in cannot serve it on any instance type, so the catalog of such
/// a release reports the pair unsupported — which is what the recorded listing of an old
/// Elasticsearch release carries for families a current release reports it on. The ONE
/// derivation of that capability, as [`offers_warm_storage`] is of the warm tier's.
pub fn offers_encryption_on(version: EngineVersion, instance_type: &str) -> bool {
    offers_encryption(instance_type) && version.since_elasticsearch(ENCRYPTION_SINCE_ELASTICSEARCH)
}

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

/// The roles a family published for the DATA TIER alone serves: its instances hold the
/// domain's data and are never master-eligible, so neither of the other hot-tier roles is
/// available on them.
///
/// The OpenSearch-optimized families are published that way, and so are the
/// storage-optimized generations that arrived after the engines forked (`i7i`, `i8g`
/// beside `i8ge`), whose recorded catalog entries carry the data role alone. The earlier
/// storage-optimized generations stay master-eligible ([`ROLES_DATA_AND_MASTER`]), which
/// is what their own entries report.
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
    /// Every ELASTICSEARCH release, and no OpenSearch release: a family the service
    /// carried on the legacy engine line and did not carry over to the OpenSearch one,
    /// so a domain of an OpenSearch release cannot be provisioned on it and there are
    /// no limits to publish for the pairing.
    ElasticsearchOnly,
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
            OfferedReleases::ElasticsearchOnly => !version.is_opensearch(),
        }
    }

    /// The instance types of this family the service PUBLISHES, smallest size first.
    ///
    /// The sizes a family is offered in are the catalog's own
    /// ([`Self::instance_types`]); which of them a caller may NAME is the model's
    /// (`OpenSearchPartitionInstanceType`), and a value outside that set never reaches a
    /// handler at all — the request boundary refuses it against the modelled value set
    /// before any operation runs. What is published is therefore the INTERSECTION of the
    /// two ([`nameable_instance_type`]): a listing that offered a caller a size the
    /// boundary then refuses would publish a type no request can use, and a family left
    /// with no nameable size is published nowhere.
    pub(super) fn published_types(&self) -> impl Iterator<Item = &'static str> + use<'_> {
        self.instance_types
            .iter()
            .copied()
            .filter(|instance_type| nameable_instance_type(instance_type))
    }

    /// Whether this family publishes `instance_type` — a size it is offered in that a
    /// request may name ([`Self::published_types`]).
    pub(super) fn publishes(&self, instance_type: &str) -> bool {
        self.instance_types.contains(&instance_type) && nameable_instance_type(instance_type)
    }
}

/// Whether a request may NAME `instance_type`: whether the model declares it as a value of
/// `OpenSearchPartitionInstanceType`, the one record of which instance types a caller can
/// send.
fn nameable_instance_type(instance_type: &str) -> bool {
    OPEN_SEARCH_PARTITION_INSTANCE_TYPE_VALUES.contains(&instance_type)
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
/// derived from `OpenSearchPartitionInstanceType`: a modelled value set carries neither
/// the engine releases that offer a family nor the node roles an instance of it serves,
/// which is most of what a catalog entry IS.
///
/// What a caller may NAME is the model's own record, though, and a request carrying a value
/// outside `OpenSearchPartitionInstanceType` is refused at the request boundary before any
/// operation runs — so what this catalog PUBLISHES is the intersection of the two
/// ([`CatalogFamily::published_types`]): every instance type a listing offers is one the
/// caller may then name, and a family or a size the model does not declare is published
/// nowhere, however the table below spells it. That is what keeps a listing, the limit
/// catalog, the accepted-value set a refusal enumerates and what a create accepts in step.
///
/// The current generations are the ones this service has offered for a hot tier, newest
/// first: the Graviton4 and seventh-generation Intel families, the Graviton3 and Graviton2
/// ones, the dense-storage families and the OpenSearch-optimized generations. A family is
/// in this table because the service offers it, never because a release's listing happened
/// to show it — and whether a caller may name a size of it is the model's answer, stated
/// above.
///
/// A generation is published WHOLE: where the service offers one as more than one family —
/// the EBS-backed and the NVMe-backed half of a memory-optimized generation (`r6g` beside
/// `r6gd`, `r7g` beside `r7gd`), the Graviton and the Intel half of a storage-optimized one
/// (`im4gn` beside `i4i`, `i8g` beside `i7i`) — every half is a row here, because a caller
/// choosing an instance type is offered each of them.
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
        family: "r7g",
        instance_types: &[
            "r7g.medium.search",
            "r7g.large.search",
            "r7g.xlarge.search",
            "r7g.2xlarge.search",
            "r7g.4xlarge.search",
            "r7g.8xlarge.search",
            "r7g.12xlarge.search",
            "r7g.16xlarge.search",
        ],
        roles: ROLES_WITH_COORDINATOR,
        releases: OfferedReleases::SinceElasticsearch(GRAVITON_SINCE_ELASTICSEARCH),
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
    // into it, and a coordinator node holds none, so it is not provisioned on one. The
    // generations that arrived after the engines forked are published for the data tier
    // alone, so they are not master-eligible either ([`ROLES_DATA_ONLY`]).
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
        roles: ROLES_DATA_ONLY,
        releases: OfferedReleases::SinceOpenSearch(GRAVITON4_SINCE_OPENSEARCH),
    },
    CatalogFamily {
        family: "i8g",
        instance_types: &[
            "i8g.large.search",
            "i8g.xlarge.search",
            "i8g.2xlarge.search",
            "i8g.4xlarge.search",
            "i8g.8xlarge.search",
            "i8g.12xlarge.search",
            "i8g.16xlarge.search",
            "i8g.24xlarge.search",
        ],
        roles: ROLES_DATA_ONLY,
        releases: OfferedReleases::SinceOpenSearch(GRAVITON4_SINCE_OPENSEARCH),
    },
    CatalogFamily {
        family: "i7i",
        instance_types: &[
            "i7i.large.search",
            "i7i.xlarge.search",
            "i7i.2xlarge.search",
            "i7i.4xlarge.search",
            "i7i.8xlarge.search",
            "i7i.12xlarge.search",
            "i7i.16xlarge.search",
            "i7i.24xlarge.search",
        ],
        roles: ROLES_DATA_ONLY,
        releases: OfferedReleases::SinceOpenSearch(SEVENTH_GENERATION_INTEL_SINCE_OPENSEARCH),
    },
    CatalogFamily {
        family: "i4i",
        instance_types: &[
            "i4i.large.search",
            "i4i.xlarge.search",
            "i4i.2xlarge.search",
            "i4i.4xlarge.search",
            "i4i.8xlarge.search",
            "i4i.12xlarge.search",
            "i4i.16xlarge.search",
            "i4i.24xlarge.search",
        ],
        roles: ROLES_DATA_AND_MASTER,
        releases: OfferedReleases::SinceElasticsearch(NVME_STORAGE_GEN4_SINCE_ELASTICSEARCH),
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
        releases: OfferedReleases::SinceElasticsearch(NVME_STORAGE_GEN4_SINCE_ELASTICSEARCH),
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
    // The burstable previous generation, which the T3 family above succeeded. It is the one
    // previous generation the service did NOT carry over to the OpenSearch engine line: the
    // T3 family replaced it there, so a domain of an OpenSearch release cannot be provisioned
    // on a T2 instance and the pairing has no limits to publish — while every Elasticsearch
    // release still offers it, as the recorded catalog of those releases reports.
    CatalogFamily {
        family: "t2",
        instance_types: &["t2.micro.search", "t2.small.search", "t2.medium.search"],
        roles: ROLES_DATA_AND_MASTER,
        releases: OfferedReleases::ElasticsearchOnly,
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

/// The trailing token every instance type this service publishes carries: an instance
/// type of this service is `<family>.<size>.search`.
pub(super) const INSTANCE_TYPE_TOKEN: &str = ".search";

/// The trailing token the SAME instance types are spelled with on the legacy
/// Elasticsearch line — `<family>.<size>.elasticsearch`, the spelling the predecessor
/// service's own `ESPartitionInstanceType` value set carries
/// (`service-com-amazonaws-elasticsearchservice-amazonelasticsearchservice2015`).
pub(super) const LEGACY_INSTANCE_TYPE_TOKEN: &str = ".elasticsearch";

/// `instance_type` rendered in the legacy Elasticsearch spelling: the same family and the
/// same size, with this service's trailing token read as the legacy one.
///
/// The two spellings name ONE instance type — the suffix was renamed with the engine, and
/// the two services publish the same hardware under their own token — so a sentence that
/// names a type in the legacy spelling still names the type the request asked for. A
/// value that does not end in this service's token is spelled some other way already and
/// is rendered as written, so nothing is invented for a value naming no instance type at
/// all.
pub fn legacy_instance_type(instance_type: &str) -> String {
    match instance_type.strip_suffix(INSTANCE_TYPE_TOKEN) {
        Some(head) => format!("{head}{LEGACY_INSTANCE_TYPE_TOKEN}"),
        None => instance_type.to_string(),
    }
}

/// The catalog entry `instance_type` belongs to, and `None` for a value the service
/// publishes no instance type for — a family the catalog does not carry, a size that
/// family is not offered in or does not publish ([`CatalogFamily::publishes`]), or a value
/// that is not `<family>.<size>.search` at all.
pub(super) fn catalog_family(instance_type: &str) -> Option<&'static CatalogFamily> {
    let family = instance_family(instance_type)?;
    INSTANCE_TYPE_CATALOG
        .iter()
        .find(|entry| entry.family == family && entry.publishes(instance_type))
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
        .flat_map(CatalogFamily::published_types)
        .collect()
}

/// Every instance type the service PUBLISHES, whatever the release, in the order the
/// catalog is published: family by family in [`catalog_rank`]'s order, each family's
/// instance types smallest size first.
///
/// The service's own accepted-value list — a value outside it names no instance type at
/// all ([`publishes_instance_type`]) — so it is what a refusal of an unknown instance
/// type enumerates, and it is the `OpenSearchPartitionInstanceType` value set the request
/// boundary admits, which carries the same families.
/// [`offered_instance_types`] is the same catalog narrowed to one release;
/// this is the set membership itself, which no release decides.
pub fn published_instance_types() -> Vec<&'static str> {
    let mut families: Vec<&'static CatalogFamily> = INSTANCE_TYPE_CATALOG.iter().collect();
    families.sort_by_key(|entry| catalog_rank(entry));
    families
        .into_iter()
        .flat_map(CatalogFamily::published_types)
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

/// Whether the instance family of `instance_type` offers warm storage at all — what
/// `InstanceTypeDetails$WarmEnabled` reports, which the published catalog reports off
/// this same answer ([`instance_type_details`]).
///
/// UltraWarm cannot be attached to a cluster whose hot tier runs on a burstable family
/// ([`BURSTABLE_FAMILIES`]), so the warm tier a request asks for on one of those is a
/// topology the service does not provision ([`warm_instance_type_refusal`]). The ONE
/// derivation of that capability, so what a create is refused on and what the catalog
/// publishes for the same instance type cannot disagree.
pub fn offers_warm_storage(instance_type: &str) -> bool {
    let family = instance_family(instance_type).unwrap_or(instance_type);
    !BURSTABLE_FAMILIES.contains(&family)
}

/// The roles an instance of `instance_type` can serve, in the order the catalog
/// publishes them, and NO role for a type the catalog does not publish — which names
/// no instance type, so there is no instance to serve a role.
///
/// The roles are the catalog entry's: the UltraWarm family serves the warm tier, a
/// family published for the data tier alone — an OpenSearch-optimized one, or a
/// storage-optimized generation that arrived after the engines forked — is a data node
/// only, a general-purpose, compute- or
/// memory-optimized instance can also be a dedicated coordinator node, and every other
/// family is a data node and master-eligible. This is the ONE derivation of them, so
/// the roles the instance-type catalog reports for a type are the roles the limit
/// catalog below publishes limits for.
pub fn instance_roles(instance_type: &str) -> Vec<&'static str> {
    catalog_family(instance_type).map_or_else(Vec::new, |entry| entry.roles.to_vec())
}

/// What `instance_type` supports on `version`, as `InstanceTypeDetails`.
///
/// Every feature member of this entry is documented "supported for the instance type",
/// so each reports a capability OF THAT TYPE:
///
/// * a capability the instance itself provides — the warm tier
///   ([`offers_warm_storage`]) — is reported off the instance family alone, and is not
///   withheld because the release the listing is for is older than the feature. That is
///   the same answer a create is refused on, so what the catalog publishes for a type and
///   what a configuration naming it is refused on cannot disagree;
/// * a capability the ENGINE provides — Amazon Cognito authentication, application
///   logging and fine-grained access control — is reported off the release that
///   introduced it, because an engine that does not have the feature cannot serve it on
///   any instance type. Fine-grained access control needs both encryption features, so
///   it is reported only where they are;
/// * the two encryption features need BOTH — hardware that provides them and an engine
///   release that has them — so they are reported off the pairing
///   ([`offers_encryption_on`]).
///
/// The roles are the ones the catalog publishes for the type ([`instance_roles`]), so
/// the entry reports the roles an instance of it can actually be provisioned in.
///
/// `availability_zones` is reported only when the caller asked for it, so a request
/// that did not carries no `AvailabilityZones` member rather than an empty list.
pub fn instance_type_details(
    version: EngineVersion,
    instance_type: &str,
    availability_zones: Option<Vec<String>>,
) -> InstanceTypeDetails {
    let encryption = offers_encryption_on(version, instance_type);
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
        warm_enabled: Some(offers_warm_storage(instance_type)),
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
pub(super) const INSTANCE_STORAGE_FAMILIES: [&str; 12] = [
    "d2", "i2", "i3", "i4i", "i7i", "i8g", "i8ge", "im4gn", "r3", "r6gd", "r7gd", "r8gd",
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

/// Whether the hot tier `cluster` runs on is an instance type warm storage is offered
/// for, and the refusal naming the instance type otherwise.
///
/// Which instance families offer warm storage is published per instance type
/// ([`offers_warm_storage`], the capability `InstanceTypeDetails$WarmEnabled` reports), so
/// a cluster that asks for the warm tier on a hot-tier instance type that does not offer
/// it describes a topology the service does not provision: the request is refused rather
/// than stored, and the refusal names the instance type the request carried, which is the
/// value the caller has to change. The link below it is [`warm_storage_refusal`] and the
/// one above it [`cold_storage_refusal`].
///
/// This sentence names that type in the LEGACY Elasticsearch spelling
/// ([`legacy_instance_type`]): the warm tier predates the rename of the instance-type
/// suffix, and the service states this refusal in the spelling of the line it was
/// introduced on, so the family and size are the request's and only the trailing token is
/// the legacy one. Both spellings denote the same instance type, so the sentence still
/// names the value the caller sent.
pub fn warm_instance_type_refusal(cluster: Option<&ClusterConfig>) -> Option<ValidationException> {
    let cluster = cluster?;
    if !cluster.warm_enabled.unwrap_or(false) {
        return None;
    }
    let instance_type = cluster.instance_type.as_deref()?;
    if offers_warm_storage(instance_type) {
        return None;
    }
    let named = legacy_instance_type(instance_type);
    Some(ValidationException {
        message: Some(format!(
            "Warm storage is not available for the {named} instance type. Select a \
             different instance type."
        )),
    })
}

/// Whether the warm nodes `cluster` asks for are a count the service provisions the warm
/// tier in, and the refusal naming the published bounds otherwise.
///
/// `ClusterConfig$WarmCount` is "the number of warm nodes in the cluster" and carries no
/// modelled range, so the bound is the service's own: the warm tier is provisioned in a
/// number of nodes inside the range the service PUBLISHES for the warm role — the very
/// range [`WARM_NODE_COUNT_RANGE`] holds and `DescribeInstanceTypeLimits` reports under
/// that role — so a cluster asking for the tier in a count outside it describes a warm
/// tier the service does not provision and the request is refused rather than stored.
/// Reading the bound off the published range is what keeps the refusal and the published
/// limit one fact.
///
/// `None` for a cluster whose warm tier is not turned on (`WarmEnabled` is "whether to
/// enable warm storage for the cluster") — the count then provisions no node — and for one
/// that names no count, which names no number of warm nodes to judge.
///
/// The sentence states the published BOUNDS and never the value the request carried: what a
/// caller has to know is the range it has to come back inside, and the count is its own.
/// The links below it are [`warm_storage_refusal`] and [`warm_instance_type_refusal`].
pub fn warm_node_count_refusal(cluster: Option<&ClusterConfig>) -> Option<ValidationException> {
    let cluster = cluster?;
    if !cluster.warm_enabled.unwrap_or(false) {
        return None;
    }
    let warm_nodes = cluster.warm_count?;
    if WARM_NODE_COUNT_RANGE.contains(&warm_nodes) {
        return None;
    }
    Some(ValidationException {
        message: Some(format!(
            "Warm storage node count must be between {} and {}.",
            WARM_NODE_COUNT_RANGE.start(),
            WARM_NODE_COUNT_RANGE.end()
        )),
    })
}

/// The fewest data nodes a cluster runs: one. `ClusterConfig$InstanceCount` is the "number
/// of data nodes in the cluster", and a cluster with no data node holds no data and serves
/// no request, so a count below this configures no cluster at all.
const DATA_NODES_MINIMUM: i32 = 1;

/// Whether `cluster` runs any data node at all, and the refusal stating that floor
/// otherwise.
///
/// `InstanceCount` is "number of data nodes in the cluster": a count of zero — or a
/// negative one, which is no count — describes a cluster the service cannot provision,
/// whatever else the configuration asks for, so it is refused before the zone rule below
/// judges how those nodes would be spread. A cluster that names no count asks for the
/// service's own default rather than for none, so there is nothing to refuse.
///
/// The sentence is FIXED and names the member rather than the value: what a caller has to
/// fix is the count, which it sent itself.
pub fn data_node_count_refusal(cluster: Option<&ClusterConfig>) -> Option<ValidationException> {
    let nodes = cluster?.instance_count?;
    if nodes >= DATA_NODES_MINIMUM {
        return None;
    }
    Some(ValidationException {
        message: Some("Instance Count should be greater than 0.".to_string()),
    })
}

/// The numbers of Availability Zones a zone-aware cluster's data nodes are spread across,
/// as words, for the sentences the refusals below state:
/// `ZoneAwarenessConfig$AvailabilityZoneCount` publishes "valid values are `2` and `3`", and
/// the service names the count in words rather than in digits where it says how many nodes
/// such a deployment takes.
///
/// Each entry carries the count, that count as a word, and the STEP the node count of a
/// deployment in that many zones goes up in — the requirement a count BETWEEN two steps
/// breaks ([`zone_awareness_refusal`]), which the service words separately from the FLOOR a
/// count below one node per zone breaks.
const ZONE_COUNT_WORDS: [(i32, &str, &str); 2] =
    [(2, "two", "an even number of"), (3, "three", "a multiple of three")];

/// How many Availability Zones a cluster asking for zone awareness is placed in, for the
/// rules below: the count it names, and the SMALLEST supported count where it names none —
/// `ZoneAwarenessConfig` is "only required if `ZoneAwarenessEnabled` is `true`", so a
/// configuration that leaves it out is placed in the fewest zones multiple zones can mean.
///
/// `None` where the cluster asks for no zone awareness, and where the count it names is
/// outside [`super::ZONE_COUNT_RANGE`] — such a count names no supported number of zones,
/// which [`zone_count_refusal`] refuses before the spread below is judged.
///
/// It is the ONE reading of how many zones a requested cluster is placed in: the spread of
/// its data nodes is judged against it below, and so is the number of subnets a VPC-access
/// placement names ([`super::vpc_network::subnet_count_refusal`]), so no two rules can
/// disagree about how many zones one configuration asks for.
pub fn requested_zone_count(cluster: &ClusterConfig) -> Option<i32> {
    if cluster.zone_awareness_enabled != Some(true) {
        return None;
    }
    let zones = cluster
        .zone_awareness_config
        .as_ref()
        .and_then(|config| config.availability_zone_count)
        .unwrap_or(*super::ZONE_COUNT_RANGE.start());
    super::ZONE_COUNT_RANGE.contains(&zones).then_some(zones)
}

/// Whether the number of Availability Zones `cluster` asks to be placed in is one the
/// service places a cluster in, and the refusal naming the supported counts otherwise.
///
/// `ZoneAwarenessConfig$AvailabilityZoneCount` is "the number of Availability Zones for the
/// domain to use with `ZoneAwarenessEnabled`" and publishes "valid values are `2` and `3`"
/// ([`super::ZONE_COUNT_RANGE`], the one place that set is stated): a zone-aware cluster
/// asking for any other number describes a placement the service does not provision, so the
/// request is refused rather than stored. The member carries no range in the model, so this
/// is the service's own statement of it, in the shape it states an unsupported enumerated
/// value in.
///
/// `None` for a cluster that asks for no zone awareness — the count configures no placement
/// then — and for one that names no count, which asks for the service's own default.
pub fn zone_count_refusal(cluster: Option<&ClusterConfig>) -> Option<ValidationException> {
    let cluster = cluster?;
    if cluster.zone_awareness_enabled != Some(true) {
        return None;
    }
    let zones = cluster
        .zone_awareness_config
        .as_ref()?
        .availability_zone_count?;
    if super::ZONE_COUNT_RANGE.contains(&zones) {
        return None;
    }
    let supported: Vec<String> = super::ZONE_COUNT_RANGE
        .clone()
        .map(|count| count.to_string())
        .collect();
    Some(ValidationException {
        message: Some(format!(
            "Unsupported AvailabilityZoneCount: {zones}, allowed values are: [{}]",
            supported.join(", ")
        )),
    })
}

/// Whether the data nodes `cluster` runs can be spread across the Availability Zones it
/// asks to be placed in, and the refusal stating the requirement it breaks otherwise.
///
/// `ClusterConfig$ZoneAwarenessEnabled` "indicates whether multiple Availability Zones are
/// enabled", and the service spreads the cluster's data nodes EVENLY across those zones
/// ([`super::nodes_per_zone`]) — so a zone-aware cluster runs at least ONE data node in each
/// of its zones and a whole number of them per zone, and one that does not describes a
/// deployment the service cannot provision. The request is refused rather than stored,
/// stating the requirement the count breaks, which the service words as TWO requirements:
/// a count below one node per zone misses the FLOOR such a deployment has (a minimum of as
/// many data nodes as it has zones), and a count at or above that floor which does not
/// divide by the zone count is off the STEP it goes up in ([`ZONE_COUNT_WORDS`]). Either
/// sentence names the zone count in words, as the service states it, and interpolates
/// nothing of the request.
///
/// `InstanceCount` is "number of data nodes in the cluster", so it is the count judged.
/// `None` for a cluster that asks for no zone awareness (one zone holds any number of
/// nodes), for one whose zone count is not a supported one ([`zone_count_refusal`] refuses
/// that count itself), and for one that names no data-node count at all — that names no
/// number of nodes to spread, and the member's absence is not this rule's to refuse. `None`
/// too for a cluster asking for a STANDBY zone: that deployment's own published requirement
/// states the spread ([`multi_az_with_standby_refusal`]), so this rule does not speak over
/// it.
pub fn zone_awareness_refusal(cluster: Option<&ClusterConfig>) -> Option<ValidationException> {
    let cluster = cluster?;
    if cluster.multi_az_with_standby_enabled == Some(true) {
        return None;
    }
    let zones = requested_zone_count(cluster)?;
    let nodes = cluster.instance_count?;
    if nodes > 0 && nodes % zones == 0 {
        return None;
    }
    let (_, zones_word, step) = ZONE_COUNT_WORDS
        .into_iter()
        .find(|(count, _, _)| *count == zones)?;
    // Which of the two requirements this count breaks: the floor, for a deployment asked
    // for with fewer data nodes than it has zones, and otherwise the step.
    let requirement = if nodes < zones {
        format!("a minimum of {zones_word}")
    } else {
        step.to_string()
    };
    Some(ValidationException {
        message: Some(format!(
            "You must choose {requirement} data nodes for a {zones_word} Availability Zone \
             deployment"
        )),
    })
}

/// The fewest dedicated master nodes a cluster that runs them is configured with, and the
/// count between that floor and the next one the service does not configure.
///
/// `ClusterConfig$DedicatedMasterCount` is documented "this number must be greater than 2
/// and not 4, otherwise you receive a validation exception": a master quorum needs an ODD
/// number of voting nodes, so the service configures three or five of them and never an
/// even count that cannot break a tie.
const DEDICATED_MASTER_COUNT_MINIMUM: i32 = 3;
const DEDICATED_MASTER_COUNT_REFUSED: i32 = 4;

/// Whether the dedicated master nodes `cluster` asks for are a quorum the service
/// configures, and the refusal naming the count otherwise.
///
/// The rule is the member's own documentation (see [`DEDICATED_MASTER_COUNT_MINIMUM`]), so
/// a cluster asking for one or two master nodes, or for four, describes a master quorum the
/// service does not provision and the request is refused rather than stored.
///
/// `None` for a cluster whose dedicated master nodes are not turned on
/// (`DedicatedMasterEnabled` "true if the cluster will use a dedicated master node") — the
/// count then configures no node — and for one that names no count, which names no number
/// of master nodes to judge.
pub fn dedicated_master_count_refusal(
    cluster: Option<&ClusterConfig>,
) -> Option<ValidationException> {
    let cluster = cluster?;
    if cluster.dedicated_master_enabled != Some(true) {
        return None;
    }
    let masters = cluster.dedicated_master_count?;
    if masters >= DEDICATED_MASTER_COUNT_MINIMUM && masters != DEDICATED_MASTER_COUNT_REFUSED {
        return None;
    }
    Some(ValidationException {
        message: Some(format!(
            "DedicatedMasterCount must be greater than {} and not {DEDICATED_MASTER_COUNT_REFUSED}: \
             {masters}",
            DEDICATED_MASTER_COUNT_MINIMUM - 1
        )),
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
/// service publishes and does not, naming the member the value was carried in.
///
/// For a member whose value a request RESOLVES an instance type by — a limit read's URI
/// label — a value the catalog carries no entry for names no instance type at all, so
/// there is nothing to report limits for. A member that merely NARROWS a listing is not
/// resolved this way: there the value selects entries, and a read asking for zone
/// information about it is refused as the zones answer it
/// ([`az_unsupported_instance_type`]).
pub fn unknown_instance_type(member: &str, value: &str) -> ValidationException {
    ValidationException {
        message: Some(format!("{member} is not a known instance type: {value}")),
    }
}

/// The FIXED text of the refusal an instance type gets when the request asked the service
/// to resolve it against an Availability Zone and no zone offers it.
///
/// It ends in the space-and-colon the value follows immediately, and it names neither the
/// member the value was carried in nor a zone: the sentence states the zone's answer about
/// the type, and the only variable part is the instance type itself.
const AZ_UNSUPPORTED_INSTANCE_TYPE: &str = "The AZ doesn't support instance type :";

/// The `ValidationException` payload for an instance type that no Availability Zone
/// offers, which is what a read asking for ZONE information about a named type is refused
/// with.
///
/// An instance type the service publishes no catalog entry for
/// ([`publishes_instance_type`]) is hardware no zone has, so there are no zones to report
/// it as offered in and the read cannot be answered. The value is echoed exactly as the
/// request carried it.
pub fn az_unsupported_instance_type(instance_type: &str) -> ValidationException {
    ValidationException {
        message: Some(format!("{AZ_UNSUPPORTED_INSTANCE_TYPE}{instance_type}")),
    }
}

/// The refusal for an instance type whose Availability Zones a request asked for and that
/// no zone offers, or `None` when there is nothing to resolve against a zone.
///
/// `zones` is the zone list the answer would carry — `None` for a read that did NOT ask
/// for zone information, which resolves no instance type against a zone at all and so
/// grades nothing. This is the ONE statement of the rule: the type is graded against the
/// zones exactly when the request asked the service to report them, and a request that
/// named no instance type pairs none with a zone.
pub fn az_unsupported_instance_type_refusal(
    zones: Option<&[String]>,
    instance_type: Option<&str>,
) -> Option<ValidationException> {
    zones?;
    let instance_type = instance_type?;
    (!publishes_instance_type(instance_type)).then(|| az_unsupported_instance_type(instance_type))
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
    provisioned_nodes(cluster)
        .into_iter()
        .map(|(instance_type, _)| instance_type)
        .collect()
}

/// Every node `cluster` provisions, as the instance type it runs on paired with the
/// ROLE that node serves — the one reading of a cluster configuration's tiers, which
/// [`provisioned_instance_types`] is the instance-type half of.
///
/// The role each tier provisions its nodes in is the tier itself:
/// `ClusterConfig$InstanceType` is the "instance type of data nodes in the cluster",
/// `DedicatedMasterType` the type "of the dedicated master nodes in the cluster",
/// `WarmType` the type "for the cluster's warm nodes", and a node option's
/// `NodeConfig$Type` the type "of a particular node within the cluster" for the
/// `NodeOption$NodeType` the option names. A tier that is turned off provisions no node
/// and so contributes none.
fn provisioned_nodes(cluster: &ClusterConfig) -> Vec<(&str, &'static str)> {
    let mut nodes: Vec<(&str, &'static str)> = Vec::new();
    nodes.extend(
        cluster
            .instance_type
            .as_deref()
            .map(|instance_type| (instance_type, INSTANCE_ROLE_DATA)),
    );
    if cluster.dedicated_master_enabled.unwrap_or(false) {
        nodes.extend(
            cluster
                .dedicated_master_type
                .as_deref()
                .map(|instance_type| (instance_type, INSTANCE_ROLE_MASTER)),
        );
    }
    if cluster.warm_enabled.unwrap_or(false) {
        nodes.extend(
            cluster
                .warm_type
                .as_deref()
                .map(|instance_type| (instance_type, INSTANCE_ROLE_ULTRAWARM)),
        );
    }
    for option in cluster.node_options.iter().flatten() {
        if let Some(config) = option.node_config.as_ref() {
            if config.enabled.unwrap_or(false) {
                nodes.extend(config.r#type.as_deref().map(|instance_type| {
                    (instance_type, node_option_role(option.node_type.as_deref()))
                }));
            }
        }
    }
    nodes
}

/// `NodeOptionsNodeType`'s one modelled value
/// (`smithy_query(query="shape",shape="NodeOptionsNodeType")`), which names the
/// dedicated coordinator role.
const NODE_TYPE_COORDINATOR: &str = "coordinator";

/// The role a node option provisions its nodes in: the coordinator role for the one
/// node type the model names, and the data role for an option naming no node type at
/// all — such an option names no role of its own.
fn node_option_role(node_type: Option<&str>) -> &'static str {
    match node_type {
        Some(NODE_TYPE_COORDINATOR) => INSTANCE_ROLE_COORDINATOR,
        _ => INSTANCE_ROLE_DATA,
    }
}

/// The refusal for the first node `cluster` would provision on an instance type that
/// cannot serve that node's ROLE, or `None` when every tier names a type published for
/// the role it provisions.
///
/// Which roles an instance type can be provisioned in is published per instance type
/// ([`instance_roles`], the set `InstanceTypeDetails$InstanceRole` reports — "whether
/// the instance acts as a data node, a dedicated master node, or an UltraWarm node"), so
/// an instance type named for a tier its published roles do not carry describes a node
/// the service cannot provision: the warm-tier family as the cluster's data nodes, a
/// data-only family as the dedicated masters, or a family that serves no coordinator
/// role as a coordinator node option.
///
/// The refusal is the service's one wording for an instance type it cannot provision
/// ([`invalid_instance_type`]), the same text an instance type the engine release does
/// not offer is refused with ([`cluster_instance_type_refusal`]): both say the type named
/// cannot run the node the request asked for.
pub fn instance_role_refusal(cluster: &ClusterConfig) -> Option<InvalidTypeException> {
    provisioned_nodes(cluster)
        .into_iter()
        .find(|(instance_type, role)| {
            // A type the catalog publishes no entry for has no published roles at all;
            // that is the engine-release gate's refusal to state
            // ([`cluster_instance_type_refusal`]), not this rule's.
            publishes_instance_type(instance_type) && !instance_roles(instance_type).contains(role)
        })
        .map(|(instance_type, _)| invalid_instance_type(instance_type))
}

/// How many Availability Zones a cluster turned on with a standby zone is placed in.
///
/// `ClusterConfig$MultiAZWithStandbyEnabled` is "a boolean that indicates whether a
/// multi-AZ domain is turned on with a standby AZ": one of the domain's zones is held in
/// reserve ([`super::STANDBY_ZONES`]) while the others serve traffic, so such a domain is
/// placed in the largest number of zones the service places a cluster in — the top of
/// [`super::ZONE_COUNT_RANGE`] — and a deployment with fewer zones has none to hold back.
fn standby_zone_requirement() -> i32 {
    *super::ZONE_COUNT_RANGE.end()
}

/// The requirement a standby deployment's data-node count is published with: the count has
/// to be a whole number of nodes per Availability Zone the domain is configured for.
///
/// FIXED text, stating the rule rather than the configuration that broke it — the sentence
/// interpolates neither the zone count nor the node count the request carried, unlike the
/// per-zone-count sentences a deployment without a standby is refused with
/// ([`zone_awareness_refusal`]).
const STANDBY_DATA_NODES_PER_ZONE: &str = "The number of data nodes must be a multiple of \
                                           the number of Availability Zones configured for \
                                           the domain.";

/// Whether `cluster` is placed in the Availability Zones a standby zone takes AND runs a
/// data-node count that deployment admits, and the refusal naming the requirement it breaks
/// otherwise.
///
/// A standby zone is one of the domain's own zones held in reserve, so a cluster asking
/// for one has to be zone-aware across [`standby_zone_requirement`] zones: one that asks
/// for a standby without zone awareness at all — a single-zone deployment, whatever its
/// node count — or with fewer zones than that describes a topology the service does not
/// provision, and the request is refused rather than stored.
///
/// Placed in those zones, its data nodes are spread across them the same way every
/// zone-aware cluster's are, and the count is held to the requirement this deployment is
/// published with ([`STANDBY_DATA_NODES_PER_ZONE`]) rather than to the per-zone-count
/// sentences [`zone_awareness_refusal`] states — which is why that rule steps aside for a
/// cluster asking for a standby and the spread of such a cluster is judged here. The create
/// and the update paths apply both alike.
pub fn multi_az_with_standby_refusal(
    cluster: Option<&ClusterConfig>,
) -> Option<ValidationException> {
    let cluster = cluster?;
    if cluster.multi_az_with_standby_enabled != Some(true) {
        return None;
    }
    let required = standby_zone_requirement();
    if requested_zone_count(cluster) != Some(required) {
        return Some(ValidationException {
            message: Some(format!(
                "You must enable zone awareness with {required} Availability Zones to use \
                 Multi-AZ with Standby."
            )),
        });
    }
    // A count the request does not carry asks for the service's own default rather than for
    // a spread to judge, exactly as it does for a cluster without a standby.
    let nodes = cluster.instance_count?;
    if nodes > 0 && nodes % required == 0 {
        return None;
    }
    Some(ValidationException {
        message: Some(STANDBY_DATA_NODES_PER_ZONE.to_string()),
    })
}

/// Whether the per-volume performance figures `ebs` asks for are inside the range the
/// service publishes for the pairing, and the refusal naming the bounds otherwise.
///
/// `EBSOptions$Iops` is "the baseline input/output (I/O) performance of EBS volumes
/// attached to data nodes" and `Throughput` "the throughput (in MiB/s)" of the same
/// volumes; the service publishes a range for each per volume sub-type and instance type
/// ([`iops_range`], [`throughput_range`] — the figures `DescribeInstanceTypeLimits`
/// reports as that pairing's minimum and maximum), so a figure outside it asks for
/// performance the service does not provision for that volume. Like a volume SIZE outside
/// its published range ([`ebs_volume_size_refusal`]) that is a published LIMIT of the
/// pairing rather than a malformed request, so the refusal is the modelled
/// `LimitExceededException` and the sentence is the one that bound is stated in: the
/// bounds, and the pairing they are published for.
///
/// A sub-type the figure does not apply to at all publishes no range
/// ([`crate::service::domain_configuration::ebs_performance_refusal`] refuses the figure
/// itself), options with no volume attach nothing, and a request carrying neither figure
/// asks for the service's own baseline.
pub fn ebs_performance_bounds_refusal(
    ebs: Option<&EBSOptions>,
    instance_type: Option<&str>,
    engine_version: &str,
) -> Option<LimitExceededException> {
    let ebs = ebs?;
    if !ebs.ebs_enabled.unwrap_or(false) {
        return None;
    }
    let sub_type = ebs.volume_type.as_deref().unwrap_or(VOLUME_TYPE_GP2);
    // Without an instance type the volume attaches to nothing, so only the sub-type's own
    // range bounds the figure: no family named is no attachment to narrow it by.
    let paired = instance_type.unwrap_or("");
    for (member, figure, range) in [
        ("Iops", ebs.iops, iops_range(paired, sub_type)),
        (
            "Throughput",
            ebs.throughput,
            throughput_range(paired, sub_type),
        ),
    ] {
        let (Some(figure), Some((minimum, maximum))) = (figure, range) else {
            continue;
        };
        let figure = i64::from(figure);
        if (minimum..=maximum).contains(&figure) {
            continue;
        }
        let message = match instance_type {
            Some(instance_type) => format!(
                "{member} must be between {minimum} and {maximum} for {instance_type} instance \
                 type and version {engine_version}"
            ),
            None => format!(
                "{member} must be between {minimum} and {maximum} for version {engine_version}"
            ),
        };
        return Some(LimitExceededException {
            message: Some(message),
        });
    }
    None
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
