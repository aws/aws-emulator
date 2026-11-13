// SPDX-License-Identifier: CC0-1.0

//! The two catalogs this service PUBLISHES — the orderable instance types and the
//! orderable hardware configurations — and the instance-type specifications both are
//! read off.
//!
//! All three are REFERENCE data rather than resources a caller creates — no operation
//! mutates any of them — so they live together, away from the store of created
//! resources: the configurations in the hardware catalog are described BY the
//! instance-type specifications (which instance types a configuration runs is those
//! specifications narrowed to the configuration's form factor and generation), and
//! what can be ORDERED is one entry per rack family of the same specifications, so a
//! single file is what keeps the three from drifting apart.

use std::collections::BTreeMap;

use sha2::{Digest, Sha256};

use crate::ops::types::{CatalogItem, DetailedInstanceTypeItem, EC2Capacity, FormFactorConfig};

use super::filter_accepts;

/// How many MiB of memory one GiB is, which turns a family's documented
/// GiB-per-vCPU ratio into the `MemoryInMib` the model reports.
const MIB_PER_GIB: i32 = 1024;

/// One EC2 instance FAMILY this service offers: the sizes it is offered in, the
/// hardware every size carries, and where an Outpost can be ordered with it.
///
/// A family is the unit because EC2 publishes a family's specifications as one rule
/// — every size in it carries the same memory per vCPU and the name is
/// `<family>.<size>` — so one row here expands to every size of that family and no
/// size can drift from its family's ratio. The row feeds BOTH reports this service
/// answers about instance types: its specifications
/// ([`instance_type_specifications`]), which is every size, and the catalog of what
/// can be ordered ([`orderable_instance_type_catalog`]), which is the family's whole
/// host alone.
struct OrderableFamily {
    /// The family prefix an instance type in it carries: `c5` in `c5.xlarge`.
    family: &'static str,
    /// GiB of memory per vCPU every size in this family carries.
    ///
    /// Source: the EC2 instance-type specifications AWS publishes — a
    /// compute-optimized family (`c5`, `c5d`, `c6gd`, `c6id`, `c7i`, `c8i`) carries
    /// 2 GiB per vCPU, a general-purpose one (`m5`, `m5d`, `m7i`, `m8i`) 4, and a
    /// memory- or storage-optimized one (`r5`, `r5d`, `r7i`, `r8i`, `i3en`) 8. A
    /// `d`-suffix family is its base family with local NVMe storage, so it carries
    /// the same vCPU and memory figures.
    memory_gib_per_vcpu: i32,
    /// Every size the family is offered in, as (size suffix, vCPUs, network
    /// performance).
    ///
    /// Source: the same EC2 instance-type specifications — `VCPUs` and
    /// `NetworkPerformance` are per-size figures, so each is spelled out per row
    /// rather than derived.
    sizes: &'static [(&'static str, i32, &'static str)],
    /// The `(FormFactor, OutpostGeneration)` pairs an Outpost carrying this family
    /// can be ordered as — the modelled `FormFactorConfig` list, which is a LIST
    /// because one instance type can be orderable in more than one of them.
    form_factors: &'static [(&'static str, &'static str)],
}

/// The EC2 instance families this service offers, each with every size it offers it
/// in.
///
/// Two different sources meet in each row. The hardware figures (the sizes, their
/// vCPU counts, the memory ratio and the network performance) are the EC2
/// instance-type specifications AWS publishes. WHICH generation and form factor a
/// family can be ordered in is the hardware AWS Outposts is built on: the
/// first-generation rack families are the compute-, general-purpose-, memory- and
/// storage-optimized families AWS Outposts racks have offered since launch, each in
/// its plain and (where it has one) its local-storage (`d`-suffix) form; the
/// second-generation rack families are the later Intel generations those racks are
/// built on, both the seventh (`c7i`, `m7i`, `r7i`) and the eighth (`c8i`, `m8i`,
/// `r8i`) in their compute-, general-purpose- and memory-optimized forms; and an
/// Outposts SERVER is built on the Graviton2 `c6gd` and the Intel `c6id`. A family is
/// published in EVERY (form factor, generation) it is offered in, which is why the
/// pairs are a list. The modelled value sets these name are
/// [`crate::ops::types::FORM_FACTOR_VALUES`] (`RACK` | `SERVER`) and
/// [`crate::ops::types::OUTPOST_GENERATION_VALUES`] (`GENERATION_1` |
/// `GENERATION_2`).
const ORDERABLE_FAMILIES: &[OrderableFamily] = &[
    OrderableFamily {
        family: "c5",
        memory_gib_per_vcpu: 2,
        sizes: &[
            ("large", 2, "Up to 10 Gigabit"),
            ("xlarge", 4, "Up to 10 Gigabit"),
            ("2xlarge", 8, "Up to 10 Gigabit"),
            ("4xlarge", 16, "Up to 10 Gigabit"),
            ("9xlarge", 36, "10 Gigabit"),
            ("12xlarge", 48, "12 Gigabit"),
            ("18xlarge", 72, "25 Gigabit"),
            ("24xlarge", 96, "25 Gigabit"),
        ],
        form_factors: &[(FORM_FACTOR_RACK, OUTPOST_GENERATION_1)],
    },
    OrderableFamily {
        family: "m5",
        memory_gib_per_vcpu: 4,
        sizes: &[
            ("large", 2, "Up to 10 Gigabit"),
            ("xlarge", 4, "Up to 10 Gigabit"),
            ("2xlarge", 8, "Up to 10 Gigabit"),
            ("4xlarge", 16, "Up to 10 Gigabit"),
            ("8xlarge", 32, "10 Gigabit"),
            ("12xlarge", 48, "12 Gigabit"),
            ("16xlarge", 64, "20 Gigabit"),
            ("24xlarge", 96, "25 Gigabit"),
        ],
        form_factors: &[(FORM_FACTOR_RACK, OUTPOST_GENERATION_1)],
    },
    OrderableFamily {
        family: "r5",
        memory_gib_per_vcpu: 8,
        sizes: &[
            ("large", 2, "Up to 10 Gigabit"),
            ("xlarge", 4, "Up to 10 Gigabit"),
            ("2xlarge", 8, "Up to 10 Gigabit"),
            ("4xlarge", 16, "Up to 10 Gigabit"),
            ("8xlarge", 32, "10 Gigabit"),
            ("12xlarge", 48, "12 Gigabit"),
            ("16xlarge", 64, "20 Gigabit"),
            ("24xlarge", 96, "25 Gigabit"),
        ],
        form_factors: &[(FORM_FACTOR_RACK, OUTPOST_GENERATION_1)],
    },
    OrderableFamily {
        family: "c5d",
        memory_gib_per_vcpu: 2,
        sizes: &[
            ("large", 2, "Up to 10 Gigabit"),
            ("xlarge", 4, "Up to 10 Gigabit"),
            ("2xlarge", 8, "Up to 10 Gigabit"),
            ("4xlarge", 16, "Up to 10 Gigabit"),
            ("9xlarge", 36, "10 Gigabit"),
            ("12xlarge", 48, "12 Gigabit"),
            ("18xlarge", 72, "25 Gigabit"),
            ("24xlarge", 96, "25 Gigabit"),
        ],
        form_factors: &[(FORM_FACTOR_RACK, OUTPOST_GENERATION_1)],
    },
    OrderableFamily {
        family: "m5d",
        memory_gib_per_vcpu: 4,
        sizes: &[
            ("large", 2, "Up to 10 Gigabit"),
            ("xlarge", 4, "Up to 10 Gigabit"),
            ("2xlarge", 8, "Up to 10 Gigabit"),
            ("4xlarge", 16, "Up to 10 Gigabit"),
            ("8xlarge", 32, "10 Gigabit"),
            ("12xlarge", 48, "12 Gigabit"),
            ("16xlarge", 64, "20 Gigabit"),
            ("24xlarge", 96, "25 Gigabit"),
        ],
        form_factors: &[(FORM_FACTOR_RACK, OUTPOST_GENERATION_1)],
    },
    OrderableFamily {
        family: "r5d",
        memory_gib_per_vcpu: 8,
        sizes: &[
            ("large", 2, "Up to 10 Gigabit"),
            ("xlarge", 4, "Up to 10 Gigabit"),
            ("2xlarge", 8, "Up to 10 Gigabit"),
            ("4xlarge", 16, "Up to 10 Gigabit"),
            ("8xlarge", 32, "10 Gigabit"),
            ("12xlarge", 48, "12 Gigabit"),
            ("16xlarge", 64, "20 Gigabit"),
            ("24xlarge", 96, "25 Gigabit"),
        ],
        form_factors: &[(FORM_FACTOR_RACK, OUTPOST_GENERATION_1)],
    },
    OrderableFamily {
        family: "i3en",
        memory_gib_per_vcpu: 8,
        sizes: &[
            ("large", 2, "Up to 25 Gigabit"),
            ("xlarge", 4, "Up to 25 Gigabit"),
            ("2xlarge", 8, "Up to 25 Gigabit"),
            ("3xlarge", 12, "Up to 25 Gigabit"),
            ("6xlarge", 24, "25 Gigabit"),
            ("12xlarge", 48, "50 Gigabit"),
            ("24xlarge", 96, "100 Gigabit"),
        ],
        form_factors: &[(FORM_FACTOR_RACK, OUTPOST_GENERATION_1)],
    },
    OrderableFamily {
        family: "c6gd",
        memory_gib_per_vcpu: 2,
        sizes: &[
            ("medium", 1, "Up to 10 Gigabit"),
            ("large", 2, "Up to 10 Gigabit"),
            ("xlarge", 4, "Up to 10 Gigabit"),
            ("2xlarge", 8, "Up to 10 Gigabit"),
            ("4xlarge", 16, "Up to 10 Gigabit"),
            ("8xlarge", 32, "12 Gigabit"),
            ("12xlarge", 48, "20 Gigabit"),
            ("16xlarge", 64, "25 Gigabit"),
        ],
        form_factors: &[(FORM_FACTOR_SERVER, OUTPOST_GENERATION_1)],
    },
    OrderableFamily {
        family: "c6id",
        memory_gib_per_vcpu: 2,
        sizes: &[
            ("large", 2, "Up to 12.5 Gigabit"),
            ("xlarge", 4, "Up to 12.5 Gigabit"),
            ("2xlarge", 8, "Up to 12.5 Gigabit"),
            ("4xlarge", 16, "Up to 12.5 Gigabit"),
            ("8xlarge", 32, "12.5 Gigabit"),
            ("12xlarge", 48, "18.75 Gigabit"),
            ("16xlarge", 64, "25 Gigabit"),
            ("24xlarge", 96, "37.5 Gigabit"),
            ("32xlarge", 128, "50 Gigabit"),
        ],
        form_factors: &[(FORM_FACTOR_SERVER, OUTPOST_GENERATION_1)],
    },
    OrderableFamily {
        family: "c7i",
        memory_gib_per_vcpu: 2,
        sizes: &[
            ("large", 2, "Up to 12.5 Gigabit"),
            ("xlarge", 4, "Up to 12.5 Gigabit"),
            ("2xlarge", 8, "Up to 12.5 Gigabit"),
            ("4xlarge", 16, "Up to 12.5 Gigabit"),
            ("8xlarge", 32, "12.5 Gigabit"),
            ("12xlarge", 48, "18.75 Gigabit"),
            ("16xlarge", 64, "25 Gigabit"),
            ("24xlarge", 96, "37.5 Gigabit"),
            ("48xlarge", 192, "50 Gigabit"),
        ],
        form_factors: &[(FORM_FACTOR_RACK, OUTPOST_GENERATION_2)],
    },
    OrderableFamily {
        family: "m7i",
        memory_gib_per_vcpu: 4,
        sizes: &[
            ("large", 2, "Up to 12.5 Gigabit"),
            ("xlarge", 4, "Up to 12.5 Gigabit"),
            ("2xlarge", 8, "Up to 12.5 Gigabit"),
            ("4xlarge", 16, "Up to 12.5 Gigabit"),
            ("8xlarge", 32, "12.5 Gigabit"),
            ("12xlarge", 48, "18.75 Gigabit"),
            ("16xlarge", 64, "25 Gigabit"),
            ("24xlarge", 96, "37.5 Gigabit"),
            ("48xlarge", 192, "50 Gigabit"),
        ],
        form_factors: &[(FORM_FACTOR_RACK, OUTPOST_GENERATION_2)],
    },
    OrderableFamily {
        family: "r7i",
        memory_gib_per_vcpu: 8,
        sizes: &[
            ("large", 2, "Up to 12.5 Gigabit"),
            ("xlarge", 4, "Up to 12.5 Gigabit"),
            ("2xlarge", 8, "Up to 12.5 Gigabit"),
            ("4xlarge", 16, "Up to 12.5 Gigabit"),
            ("8xlarge", 32, "12.5 Gigabit"),
            ("12xlarge", 48, "18.75 Gigabit"),
            ("16xlarge", 64, "25 Gigabit"),
            ("24xlarge", 96, "37.5 Gigabit"),
            ("48xlarge", 192, "50 Gigabit"),
        ],
        form_factors: &[(FORM_FACTOR_RACK, OUTPOST_GENERATION_2)],
    },
    OrderableFamily {
        family: "c8i",
        memory_gib_per_vcpu: 2,
        sizes: &[
            ("large", 2, "Up to 12.5 Gigabit"),
            ("xlarge", 4, "Up to 12.5 Gigabit"),
            ("2xlarge", 8, "Up to 12.5 Gigabit"),
            ("4xlarge", 16, "Up to 12.5 Gigabit"),
            ("8xlarge", 32, "12.5 Gigabit"),
            ("12xlarge", 48, "18.75 Gigabit"),
            ("16xlarge", 64, "25 Gigabit"),
            ("24xlarge", 96, "37.5 Gigabit"),
            ("32xlarge", 128, "50 Gigabit"),
            ("48xlarge", 192, "75 Gigabit"),
            ("96xlarge", 384, "100 Gigabit"),
        ],
        form_factors: &[(FORM_FACTOR_RACK, OUTPOST_GENERATION_2)],
    },
    OrderableFamily {
        family: "m8i",
        memory_gib_per_vcpu: 4,
        sizes: &[
            ("large", 2, "Up to 12.5 Gigabit"),
            ("xlarge", 4, "Up to 12.5 Gigabit"),
            ("2xlarge", 8, "Up to 12.5 Gigabit"),
            ("4xlarge", 16, "Up to 12.5 Gigabit"),
            ("8xlarge", 32, "12.5 Gigabit"),
            ("12xlarge", 48, "18.75 Gigabit"),
            ("16xlarge", 64, "25 Gigabit"),
            ("24xlarge", 96, "37.5 Gigabit"),
            ("32xlarge", 128, "50 Gigabit"),
            ("48xlarge", 192, "75 Gigabit"),
            ("96xlarge", 384, "100 Gigabit"),
        ],
        form_factors: &[(FORM_FACTOR_RACK, OUTPOST_GENERATION_2)],
    },
    OrderableFamily {
        family: "r8i",
        memory_gib_per_vcpu: 8,
        sizes: &[
            ("large", 2, "Up to 12.5 Gigabit"),
            ("xlarge", 4, "Up to 12.5 Gigabit"),
            ("2xlarge", 8, "Up to 12.5 Gigabit"),
            ("4xlarge", 16, "Up to 12.5 Gigabit"),
            ("8xlarge", 32, "12.5 Gigabit"),
            ("12xlarge", 48, "18.75 Gigabit"),
            ("16xlarge", 64, "25 Gigabit"),
            ("24xlarge", 96, "37.5 Gigabit"),
            ("32xlarge", 128, "50 Gigabit"),
            ("48xlarge", 192, "75 Gigabit"),
            ("96xlarge", 384, "100 Gigabit"),
        ],
        form_factors: &[(FORM_FACTOR_RACK, OUTPOST_GENERATION_2)],
    },
];

/// The `FormFactor` an Outposts RACK carries, from the modelled value set
/// [`crate::ops::types::FORM_FACTOR_VALUES`].
pub const FORM_FACTOR_RACK: &str = "RACK";

/// The `FormFactor` an Outposts SERVER carries. See [`FORM_FACTOR_RACK`].
pub const FORM_FACTOR_SERVER: &str = "SERVER";

/// The first-generation `OutpostGeneration`, from the modelled value set
/// [`crate::ops::types::OUTPOST_GENERATION_VALUES`] — "`GENERATION_1` for
/// first-generation rack deployments", as
/// `ListOrderableInstanceTypes$OutpostGenerationFilter` documents it.
pub const OUTPOST_GENERATION_1: &str = "GENERATION_1";

/// The second-generation `OutpostGeneration`. See [`OUTPOST_GENERATION_1`].
pub const OUTPOST_GENERATION_2: &str = "GENERATION_2";

/// One catalog entry: the modelled [`DetailedInstanceTypeItem`] plus its position in
/// the catalog's stable order.
///
/// The catalog is REFERENCE data, not a resource a caller creates, so the sequence is
/// the entry's position in that order rather than a creation counter — it is what
/// [`super::page`] cuts pages at and what a `NextToken` names, exactly as a created
/// resource's [`super::SiteRecord::sequence`] is.
#[derive(Debug, Clone)]
pub struct OrderableInstanceTypeRecord {
    /// Position in the catalog's stable order. See [`super::SiteRecord::sequence`].
    pub sequence: u64,
    /// The entry as `ListOrderableInstanceTypes` reports it.
    pub instance_type: DetailedInstanceTypeItem,
}

impl OrderableInstanceTypeRecord {
    /// Whether an Outpost of `generation` can be ordered with this instance type:
    /// whether ANY of its `FormFactorConfigs` names that generation.
    ///
    /// `FormFactorConfigs` is a list of the (form factor, generation) combinations
    /// the instance type is offered in, so one entry naming the generation is what
    /// makes the entry orderable in it — the same "matches any of the values" rule
    /// every filter in this API follows.
    pub fn is_orderable_in(&self, generation: &str) -> bool {
        self.instance_type
            .form_factor_configs
            .iter()
            .flatten()
            .any(|config| config.outpost_generation.as_deref() == Some(generation))
    }

    /// Whether this instance type is offered on `form_factor` hardware of
    /// `generation`: whether ANY of its `FormFactorConfigs` names that PAIR.
    ///
    /// The narrower question [`Self::is_orderable_in`] asks of the generation alone,
    /// asked of one whole configuration — which is what tells which instance types a
    /// given piece of hardware runs (see [`catalog_item_catalog`]).
    pub fn is_offered_as(&self, form_factor: &str, generation: &str) -> bool {
        self.instance_type
            .form_factor_configs
            .iter()
            .flatten()
            .any(|config| {
                config.form_factor.as_deref() == Some(form_factor)
                    && config.outpost_generation.as_deref() == Some(generation)
            })
    }
}

/// One SIZE of one family as the modelled [`DetailedInstanceTypeItem`] reports it.
///
/// Projecting a size here is what keeps every entry consistent with its family's
/// published ratio: `MemoryInMib` is the size's vCPU count times the family's GiB per
/// vCPU times [`MIB_PER_GIB`], never a per-size figure that could disagree with it.
fn instance_type_item(
    family: &OrderableFamily,
    size: &str,
    vcpus: i32,
    network_performance: &str,
) -> DetailedInstanceTypeItem {
    DetailedInstanceTypeItem {
        instance_type: Some(format!("{}.{size}", family.family)),
        vcp_us: Some(vcpus),
        memory_in_mib: vcpus * family.memory_gib_per_vcpu * MIB_PER_GIB,
        network_performance: Some(network_performance.to_string()),
        form_factor_configs: Some(
            family
                .form_factors
                .iter()
                .map(|(form_factor, generation)| FormFactorConfig {
                    form_factor: Some((*form_factor).to_string()),
                    outpost_generation: Some((*generation).to_string()),
                })
                .collect(),
        ),
    }
}

/// `items` as the records a read of a catalog pages through: in one order, each
/// numbered by its position in it.
///
/// The order is by instance type name so it is stable across processes — a page token
/// names a position in it, so the order a caller pages through must not depend on how
/// the table above happens to be written.
fn sequenced(mut items: Vec<DetailedInstanceTypeItem>) -> Vec<OrderableInstanceTypeRecord> {
    items.sort_by(|left, right| left.instance_type.cmp(&right.instance_type));
    items
        .into_iter()
        .enumerate()
        .map(|(index, instance_type)| OrderableInstanceTypeRecord {
            sequence: u64::try_from(index).unwrap_or_default() + 1,
            instance_type,
        })
        .collect()
}

/// The WHOLE HOST of a family: the largest size it is offered in, as (size suffix,
/// vCPUs, network performance), or `None` for a family offered in no size at all.
///
/// One instance of that size IS one physical host of the family — every smaller size
/// is a slice of the same host — which is why it is the size an Outpost's EC2
/// capacity is ordered in (see [`orderable_instance_type_catalog`]) and the size the
/// published capacity of a configuration names (`EC2Capacity$MaxSize`, "the maximum
/// size of the EC2 capacity", counted in `Quantity` whole hosts).
fn whole_host(family: &OrderableFamily) -> Option<&(&'static str, i32, &'static str)> {
    family.sizes.iter().max_by_key(|(_, vcpus, _)| *vcpus)
}

/// Every instance type this service knows the hardware specifications of: every size
/// of every family in [`ORDERABLE_FAMILIES`].
///
/// This is the specification table, not the published catalog: a caller configures an
/// Outpost's capacity in any size of a family its hardware runs — a capacity task
/// names an instance type, and what one Outpost SUPPORTS is every size that fits its
/// hardware — so the vCPU count of each size has to be a figure this service holds.
/// What can be ORDERED is the narrower list [`orderable_instance_type_catalog`]
/// publishes.
pub fn instance_type_specifications() -> Vec<OrderableInstanceTypeRecord> {
    // The table may only ever name values the model declares, so it is checked
    // against the GENERATED value sets rather than trusted to agree with them: a
    // model that renames a `FormFactor` or an `OutpostGeneration` trips this in every
    // debug build instead of letting a catalog report an unmodelled member.
    debug_assert!(
        ORDERABLE_FAMILIES
            .iter()
            .flat_map(|family| family.form_factors)
            .all(|(form_factor, generation)| {
                crate::ops::types::FORM_FACTOR_VALUES.contains(form_factor)
                    && crate::ops::types::OUTPOST_GENERATION_VALUES.contains(generation)
            }),
        "every catalog configuration names a modelled FormFactor and OutpostGeneration"
    );
    sequenced(
        ORDERABLE_FAMILIES
            .iter()
            .flat_map(|family| {
                family
                    .sizes
                    .iter()
                    .map(move |(size, vcpus, network_performance)| {
                        instance_type_item(family, size, *vcpus, network_performance)
                    })
            })
            .collect(),
    )
}

/// The orderable-instance-type catalog this service publishes: one entry per RACK
/// family it offers, naming that family's WHOLE HOST ([`whole_host`]).
///
/// EC2 capacity is ordered for an Outpost in whole hosts — a line item of an order
/// adds hosts of a family to the hardware, and the capacity a configuration carries
/// is published as `EC2Capacity$MaxSize` with a `Quantity` of them — so the instance
/// types that can be ordered are the host types, one per family, and the smaller
/// sizes of a family are configured on a host already installed rather than ordered.
/// Those smaller sizes are therefore specifications this service holds
/// ([`instance_type_specifications`]) rather than entries it publishes as orderable.
///
/// What capacity is ordered BY the instance type is rack capacity, which is the
/// question this read answers: its own `OutpostGenerationFilter` selects "the
/// generation of hardware", documented per value as a generation of "rack
/// deployments" ([`OUTPOST_GENERATION_1`]). An Outposts SERVER is one host of its own
/// — it is ordered as a whole unit from the hardware catalog
/// ([`catalog_item_catalog`]), carrying the instance type its hardware comes with —
/// so a SERVER-only family is hardware this service holds specifications for and
/// publishes capacity of ([`ec2_capacities`]) without publishing it as a type
/// capacity can be ordered in.
pub fn orderable_instance_type_catalog() -> Vec<OrderableInstanceTypeRecord> {
    sequenced(
        ORDERABLE_FAMILIES
            .iter()
            .filter(|family| {
                family
                    .form_factors
                    .iter()
                    .any(|(form_factor, _)| *form_factor == FORM_FACTOR_RACK)
            })
            .filter_map(|family| {
                whole_host(family).map(|(size, vcpus, network_performance)| {
                    instance_type_item(family, size, *vcpus, network_performance)
                })
            })
            .collect(),
    )
}

/// The `CatalogItemClass` an Outposts RACK configuration is published as, from the
/// modelled value set [`crate::ops::types::CATALOG_ITEM_CLASS_VALUES`] (`RACK` |
/// `SERVER`).
///
/// A class names the same kind of hardware the modelled `FormFactor` does
/// ([`FORM_FACTOR_RACK`]), which is what lets a configuration's class select the
/// instance types an Outpost of that hardware runs (see [`catalog_item_catalog`]).
pub const CATALOG_ITEM_CLASS_RACK: &str = "RACK";

/// The `CatalogItemClass` an Outposts SERVER configuration is published as. See
/// [`CATALOG_ITEM_CLASS_RACK`].
pub const CATALOG_ITEM_CLASS_SERVER: &str = "SERVER";

/// The `CatalogItemStatus` a configuration that can still be ordered carries, from
/// the modelled value set [`crate::ops::types::CATALOG_ITEM_STATUS_VALUES`]
/// (`AVAILABLE` | `DISCONTINUED`).
pub const CATALOG_ITEM_STATUS_AVAILABLE: &str = "AVAILABLE";

/// The `CatalogItemStatus` a configuration that is no longer offered carries.
///
/// A discontinued item stays IN the catalog — it is still what an Outpost already
/// installed was ordered as — so a read of the catalog reports it; the operation
/// models no status filter to leave it out with. See
/// [`CATALOG_ITEM_STATUS_AVAILABLE`].
pub const CATALOG_ITEM_STATUS_DISCONTINUED: &str = "DISCONTINUED";

/// The Amazon EBS storage option, from the modelled `SupportedStorageEnum` value set
/// [`crate::ops::types::SUPPORTED_STORAGE_ENUM_VALUES`] (`EBS` | `S3`).
pub const SUPPORTED_STORAGE_EBS: &str = "EBS";

/// The Amazon S3 storage option. See [`SUPPORTED_STORAGE_EBS`].
pub const SUPPORTED_STORAGE_S3: &str = "S3";

/// The bytes one terabyte of published S3 on Outposts capacity holds.
///
/// AWS publishes the S3 on Outposts capacity options of a rack in decimal
/// terabytes, so that is the unit [`CatalogConfiguration::s3_capacity_bytes`]
/// converts from.
const PUBLISHED_TERABYTE: i64 = 1_000_000_000_000;

/// The `RackScalingType` of a configuration that scales as one rack, from the
/// modelled value set [`crate::ops::types::RACK_SCALING_TYPE_VALUES`]
/// (`SINGLE_RACK` | `MULTI_RACK`), which `CatalogItem$RackScalingType` documents as
/// its valid values.
pub const RACK_SCALING_TYPE_SINGLE_RACK: &str = "SINGLE_RACK";

/// The `RackScalingType` of a configuration that scales across racks. See
/// [`RACK_SCALING_TYPE_SINGLE_RACK`].
pub const RACK_SCALING_TYPE_MULTI_RACK: &str = "MULTI_RACK";

/// The `RackUnitHeight` an Outposts RACK configuration carries, from the modelled
/// value set [`crate::ops::types::RACK_UNIT_HEIGHT_VALUES`] (`HEIGHT_42U` |
/// `HEIGHT_2U` | `HEIGHT_1U`).
///
/// A rack is the whole rack, so it is the full-height value of that set; the other
/// two are the heights a single server comes in ([`RACK_UNIT_HEIGHT_1U`]).
pub const RACK_UNIT_HEIGHT_42U: &str = "HEIGHT_42U";

/// The `RackUnitHeight` of a one-rack-unit Outposts SERVER configuration, from the
/// same value set. See [`RACK_UNIT_HEIGHT_42U`].
pub const RACK_UNIT_HEIGHT_1U: &str = "HEIGHT_1U";

/// The `RackUnitHeight` of a two-rack-unit Outposts SERVER configuration, from the
/// same value set. See [`RACK_UNIT_HEIGHT_42U`].
pub const RACK_UNIT_HEIGHT_2U: &str = "HEIGHT_2U";

/// The `CurrentType` every configuration this service publishes carries: the
/// electrical current its hardware is fed with.
///
/// Source: the power a SITE declares it can provide at a hardware placement
/// position, which is the modelled `RackPhysicalProperties` — its `PowerConnector`
/// set (`AH530P7W`, `AH532P6W`, `IEC309`, `L6_30P`) and its `PowerPhase` set
/// (`SINGLE_PHASE` | `THREE_PHASE`) are alternating-current connectors and phases
/// throughout, so hardware a site can be prepared for draws alternating current. The
/// published configurations are all such hardware, so each reports this one value
/// rather than a per-row figure that could disagree with the site requirements.
pub const CURRENT_TYPE_AC: &str = "AC";

/// The `Dimensions` an Outposts RACK configuration carries: the physical form a
/// whole rack takes.
///
/// A configuration is either a rack or a single server ([`CATALOG_ITEM_CLASS_RACK`]),
/// and a server's form is the height it occupies in the customer's own rack
/// ([`server_dimensions`]) — so the form follows from the class and the height and is
/// never a third, independent fact that could contradict either.
pub const DIMENSIONS_OUTPOST_RACK: &str = "OUTPOST_RACK";

/// The `Dimensions` prefix an Outposts SERVER configuration carries, completed by the
/// rack units it occupies. See [`DIMENSIONS_OUTPOST_RACK`].
const DIMENSIONS_OUTPOST_SERVER_PREFIX: &str = "OUTPOST_SERVER_";

/// The prefix every modelled `RackUnitHeight` value carries before the rack units it
/// names (`HEIGHT_1U` → `1U`), which is what [`server_dimensions`] reads the units
/// off.
const RACK_UNIT_HEIGHT_PREFIX: &str = "HEIGHT_";

/// The `Dimensions` of a SERVER configuration of `rack_unit_height`: the form of a
/// server is the rack units it occupies, so the two are one fact reported two ways.
fn server_dimensions(rack_unit_height: &str) -> String {
    let units = rack_unit_height
        .strip_prefix(RACK_UNIT_HEIGHT_PREFIX)
        .unwrap_or(rack_unit_height);
    format!("{DIMENSIONS_OUTPOST_SERVER_PREFIX}{units}")
}

/// The literal prefix every `SkuCode` carries, as the modelled pattern
/// `^OR-[A-Z0-9]{7}$` spells it.
const SKU_PREFIX: &str = "OR-";

/// How many characters of [`SKU_ALPHABET`] follow that prefix, from the same pattern.
const SKU_BODY: usize = 7;

/// The alphabet a `SkuCode`'s body is drawn from, from the same pattern's
/// `[A-Z0-9]`.
const SKU_ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";

/// The `SkuCode` a hardware configuration is published under, derived from `seed`:
/// the configuration's own facts.
///
/// A SKU is not a resource a caller creates, so it is DERIVED rather than minted —
/// the same configuration is published under the same code in every process, which
/// is what lets a caller that read a SKU out of the catalog read that item back by
/// it later. The rendering is exactly the modelled `SkuCode` pattern.
fn sku_code(seed: &str) -> String {
    let digest = Sha256::digest(seed.as_bytes());
    let body: String = digest
        .iter()
        .take(SKU_BODY)
        .map(|byte| char::from(SKU_ALPHABET[usize::from(*byte) % SKU_ALPHABET.len()]))
        .collect();
    format!("{SKU_PREFIX}{body}")
}

/// One hardware configuration in the catalog this service publishes: what it is,
/// what it draws and weighs, how it attaches to the Region, which storage it
/// supports and how much EC2 capacity it carries.
struct CatalogConfiguration {
    /// The `CatalogItemClass` the configuration is published as.
    item_class: &'static str,
    /// The `OutpostGeneration` of its hardware, which together with `item_class`
    /// selects the instance types it runs. Not reported: the modelled `CatalogItem`
    /// carries neither the class nor the generation, only what they imply.
    generation: &'static str,
    /// The `CatalogItemStatus` the configuration is published in.
    item_status: &'static str,
    /// `PowerKva`: the power the configuration draws.
    power_kva: f32,
    /// `WeightLbs`: what the configuration weighs.
    weight_lbs: i32,
    /// `SupportedUplinkGbps`: the uplink speeds it supports for the connection to
    /// the Region.
    supported_uplink_gbps: &'static [i32],
    /// `SupportedStorage`: the storage options it supports.
    supported_storage: &'static [&'static str],
    /// How much Amazon S3 storage capacity, in bytes, this configuration carries —
    /// zero for one that supports no S3 storage, which therefore has no S3 capacity
    /// to provision on an Outpost ordered from it.
    ///
    /// Held beside `supported_storage` because that member says WHICH storage
    /// options the hardware supports and never how much of each: the figure is the
    /// one an S3 on Outposts bucket's capacity is drawn from, which no member of this
    /// API reports. The values are AWS's published S3 on Outposts capacity options
    /// (26, 48, 96, 240 and 380 TB), one per S3-capable configuration in order of the
    /// EC2 capacity it carries — this emulator's own catalog assignment, exactly as
    /// which configurations are offered at all is.
    s3_capacity_bytes: i64,
    /// `RackScalingType`, or `None` for a configuration rack scaling does not apply
    /// to — a single server is not a rack, so it scales as neither one rack nor
    /// several, and the member is omitted rather than given a value the hardware has
    /// no answer for.
    rack_scaling_type: Option<&'static str>,
    /// `RackUnitHeight`: how many rack units the configuration's hardware occupies,
    /// from the modelled value set ([`RACK_UNIT_HEIGHT_42U`]).
    ///
    /// Spelled out per row because it is a fact about the hardware rather than one
    /// its class implies: a rack is the whole rack, while the servers come in the two
    /// single-unit heights that set names. `Dimensions` and the `Dimensions` of a
    /// server follow from it ([`server_dimensions`]), so the two never disagree.
    rack_unit_height: &'static str,
    /// How many vCPUs of EC2 capacity the configuration carries, which is what the
    /// `Quantity` of each of its EC2 capacities is computed from (see
    /// [`ec2_capacities`]). Not reported: the modelled `CatalogItem` reports the
    /// capacity per family rather than one total.
    capacity_vcpus: i32,
}

impl CatalogConfiguration {
    /// The seed [`sku_code`] derives this configuration's `CatalogItemId` from:
    /// every fact that distinguishes it from another row, so two different
    /// configurations are never published under one SKU.
    fn sku_seed(&self) -> String {
        format!(
            "{}|{}|{}|{}|{}|{}|{:?}|{:?}|{}",
            self.item_class,
            self.generation,
            self.item_status,
            self.power_kva,
            self.weight_lbs,
            self.capacity_vcpus,
            self.supported_uplink_gbps,
            self.supported_storage,
            self.rack_scaling_type.unwrap_or_default(),
        )
    }

    /// The `Dimensions` this configuration is published with: the physical form its
    /// hardware takes.
    ///
    /// A rack is a whole rack ([`DIMENSIONS_OUTPOST_RACK`]); a server is the rack
    /// units it occupies in the customer's own rack ([`server_dimensions`]), which is
    /// the same fact its `RackUnitHeight` reports.
    fn dimensions(&self) -> String {
        match self.item_class {
            CATALOG_ITEM_CLASS_RACK => DIMENSIONS_OUTPOST_RACK.to_string(),
            _ => server_dimensions(self.rack_unit_height),
        }
    }
}

/// The catalog of orderable hardware configurations this service publishes.
///
/// Three sources meet in these rows. The uplink speeds are the modelled `UplinkGbps`
/// set (`UPLINK_1G` | `UPLINK_10G` | `UPLINK_40G` | `UPLINK_100G`), read as the
/// Gbps figures the member's own `SupportedUplinkGbps` integers report. A rack's
/// power draws are the modelled `PowerDrawKva` set (`POWER_5_KVA` …
/// `POWER_30_KVA`), which is what a site declares it can provide at a rack position,
/// and a rack's weight stays inside the modelled `MaximumSupportedWeightLbs` set
/// (`MAX_1400_LBS` … `MAX_2000_LBS`, above which the model's own `NO_LIMIT` begins)
/// so a published rack is one a site can declare support for. WHICH configurations
/// are offered — their storage options, how much EC2 capacity each carries and which
/// are still available — is this emulator's own catalog assignment, exactly as
/// [`ORDERABLE_FAMILIES`]'s generation assignment is: AWS prices a configuration per
/// quote rather than publishing a machine-readable catalog. A server is not a rack,
/// so its power and weight are the hardware's own rather than a rack position's.
const CATALOG_ITEMS: &[CatalogConfiguration] = &[
    CatalogConfiguration {
        item_class: CATALOG_ITEM_CLASS_RACK,
        generation: OUTPOST_GENERATION_1,
        item_status: CATALOG_ITEM_STATUS_AVAILABLE,
        power_kva: 5.0,
        weight_lbs: 1400,
        supported_uplink_gbps: &[1, 10, 40, 100],
        supported_storage: &[SUPPORTED_STORAGE_EBS, SUPPORTED_STORAGE_S3],
        s3_capacity_bytes: 26 * PUBLISHED_TERABYTE,
        rack_scaling_type: Some(RACK_SCALING_TYPE_SINGLE_RACK),
        rack_unit_height: RACK_UNIT_HEIGHT_42U,
        capacity_vcpus: 480,
    },
    CatalogConfiguration {
        item_class: CATALOG_ITEM_CLASS_RACK,
        generation: OUTPOST_GENERATION_1,
        item_status: CATALOG_ITEM_STATUS_AVAILABLE,
        power_kva: 10.0,
        weight_lbs: 1600,
        supported_uplink_gbps: &[10, 40, 100],
        supported_storage: &[SUPPORTED_STORAGE_EBS, SUPPORTED_STORAGE_S3],
        s3_capacity_bytes: 48 * PUBLISHED_TERABYTE,
        rack_scaling_type: Some(RACK_SCALING_TYPE_SINGLE_RACK),
        rack_unit_height: RACK_UNIT_HEIGHT_42U,
        capacity_vcpus: 960,
    },
    CatalogConfiguration {
        item_class: CATALOG_ITEM_CLASS_RACK,
        generation: OUTPOST_GENERATION_1,
        item_status: CATALOG_ITEM_STATUS_AVAILABLE,
        power_kva: 15.0,
        weight_lbs: 1800,
        supported_uplink_gbps: &[10, 40, 100],
        supported_storage: &[SUPPORTED_STORAGE_EBS, SUPPORTED_STORAGE_S3],
        s3_capacity_bytes: 240 * PUBLISHED_TERABYTE,
        rack_scaling_type: Some(RACK_SCALING_TYPE_MULTI_RACK),
        rack_unit_height: RACK_UNIT_HEIGHT_42U,
        capacity_vcpus: 1440,
    },
    CatalogConfiguration {
        item_class: CATALOG_ITEM_CLASS_RACK,
        generation: OUTPOST_GENERATION_1,
        item_status: CATALOG_ITEM_STATUS_DISCONTINUED,
        power_kva: 5.0,
        weight_lbs: 1400,
        supported_uplink_gbps: &[1, 10],
        supported_storage: &[SUPPORTED_STORAGE_EBS],
        s3_capacity_bytes: 0,
        rack_scaling_type: Some(RACK_SCALING_TYPE_SINGLE_RACK),
        rack_unit_height: RACK_UNIT_HEIGHT_42U,
        capacity_vcpus: 240,
    },
    CatalogConfiguration {
        item_class: CATALOG_ITEM_CLASS_RACK,
        generation: OUTPOST_GENERATION_2,
        item_status: CATALOG_ITEM_STATUS_AVAILABLE,
        power_kva: 10.0,
        weight_lbs: 1800,
        supported_uplink_gbps: &[10, 40, 100],
        supported_storage: &[SUPPORTED_STORAGE_EBS, SUPPORTED_STORAGE_S3],
        s3_capacity_bytes: 96 * PUBLISHED_TERABYTE,
        rack_scaling_type: Some(RACK_SCALING_TYPE_SINGLE_RACK),
        rack_unit_height: RACK_UNIT_HEIGHT_42U,
        capacity_vcpus: 1280,
    },
    CatalogConfiguration {
        item_class: CATALOG_ITEM_CLASS_RACK,
        generation: OUTPOST_GENERATION_2,
        item_status: CATALOG_ITEM_STATUS_AVAILABLE,
        power_kva: 30.0,
        weight_lbs: 2000,
        supported_uplink_gbps: &[40, 100],
        supported_storage: &[SUPPORTED_STORAGE_EBS, SUPPORTED_STORAGE_S3],
        s3_capacity_bytes: 380 * PUBLISHED_TERABYTE,
        rack_scaling_type: Some(RACK_SCALING_TYPE_MULTI_RACK),
        rack_unit_height: RACK_UNIT_HEIGHT_42U,
        capacity_vcpus: 2560,
    },
    CatalogConfiguration {
        item_class: CATALOG_ITEM_CLASS_SERVER,
        generation: OUTPOST_GENERATION_1,
        item_status: CATALOG_ITEM_STATUS_AVAILABLE,
        power_kva: 0.5,
        weight_lbs: 50,
        supported_uplink_gbps: &[1, 10],
        supported_storage: &[SUPPORTED_STORAGE_EBS],
        s3_capacity_bytes: 0,
        rack_scaling_type: None,
        rack_unit_height: RACK_UNIT_HEIGHT_1U,
        capacity_vcpus: 64,
    },
    CatalogConfiguration {
        item_class: CATALOG_ITEM_CLASS_SERVER,
        generation: OUTPOST_GENERATION_1,
        item_status: CATALOG_ITEM_STATUS_AVAILABLE,
        power_kva: 1.0,
        weight_lbs: 70,
        supported_uplink_gbps: &[1, 10],
        supported_storage: &[SUPPORTED_STORAGE_EBS],
        s3_capacity_bytes: 0,
        rack_scaling_type: None,
        rack_unit_height: RACK_UNIT_HEIGHT_2U,
        capacity_vcpus: 128,
    },
];

/// One entry of the catalog of items: the modelled [`CatalogItem`] as a read of the
/// catalog reports it, plus the entry's position in the catalog's stable order and
/// the one fact a filter selects on that the projection does not carry.
///
/// The catalog is REFERENCE data, not a resource a caller creates, so the sequence is
/// the entry's position in that order rather than a creation counter — exactly as
/// [`OrderableInstanceTypeRecord::sequence`] is.
#[derive(Debug, Clone)]
pub struct CatalogItemRecord {
    /// Position in the catalog's stable order. See [`super::SiteRecord::sequence`].
    pub sequence: u64,
    /// How many vCPUs of EC2 capacity this item's hardware carries.
    ///
    /// Held BESIDE the projection because the modelled `CatalogItem` reports the
    /// capacity per instance family rather than one total ([`ec2_capacities`]). It is
    /// the published size of the hardware, which is what a subscription on it is
    /// priced by (see
    /// [`super::ComAmazonawsOutpostsOutpostsolafserviceState::renewal_pricing`]).
    pub capacity_vcpus: i32,
    /// How much Amazon S3 storage capacity, in bytes, this item's hardware carries.
    ///
    /// Held BESIDE the projection for the reason [`Self::capacity_vcpus`] is: the
    /// modelled `CatalogItem` reports WHICH storage options an item supports and
    /// never how much of each. It is the published S3 capacity of the hardware,
    /// which is what an Outpost ordered from this item has provisioned on it (see
    /// [`CatalogConfiguration::s3_capacity_bytes`]).
    pub s3_capacity_bytes: i64,
    /// The `OutpostGeneration` of this item's hardware.
    ///
    /// Held BESIDE the projection for the reason [`Self::capacity_vcpus`] is: the
    /// modelled `CatalogItem` carries neither the class nor the generation of a
    /// configuration, only what they imply. It is the generation of the hardware an
    /// Outpost ordered from this item is installed with, which is the generation that
    /// Outpost then reports as its own (see
    /// [`super::ComAmazonawsOutpostsOutpostsolafserviceState::installed_configuration`]).
    pub generation: &'static str,
    /// The entry as a read of the catalog reports it.
    pub item: CatalogItem,
}

impl CatalogItemRecord {
    /// Whether an order can still be placed for the configuration this item
    /// publishes: whether its published `CatalogItemStatus` is the one a
    /// configuration that can still be ordered carries
    /// ([`CATALOG_ITEM_STATUS_AVAILABLE`]).
    ///
    /// The question is asked of the status the catalog publishes NOW, so the one
    /// operation that orders a published configuration reads it there rather than
    /// keeping a second list of which SKUs are orderable. Anything other than that
    /// status — [`CATALOG_ITEM_STATUS_DISCONTINUED`], or a status this service does
    /// not publish an item in at all — is not a configuration still being offered.
    pub fn is_orderable(&self) -> bool {
        self.item.item_status.as_deref() == Some(CATALOG_ITEM_STATUS_AVAILABLE)
    }

    /// Whether this item is of ANY of the classes a filter names.
    ///
    /// Asked of the class the item REPORTS of itself (`CatalogItem$ItemClass`), so a
    /// filtered read can never select on a class different from the one the selected
    /// item says it is. The class is a SINGLE-valued fact about an item, so the
    /// selection is the one rule every list of this service applies to a `*Filter`
    /// member — a filter is a list of accepted values, and one carrying no value at
    /// all narrows nothing, exactly as an absent one does. That rule lives in
    /// [`filter_accepts`] and is stated only there; this method only says which fact
    /// the filter selects on.
    pub fn is_class(&self, classes: &[String]) -> bool {
        filter_accepts(Some(classes), self.item.item_class.as_deref())
    }

    /// Whether this item supports ANY of the storage options a filter names.
    ///
    /// The storage options are a MULTI-valued fact about an item — the item matches
    /// when any of its own values is one of the accepted ones — which is not the
    /// single-valued selection [`filter_accepts`] covers, so the quantifier over the
    /// record's own values is spelled out here.
    pub fn supports_storage(&self, options: &[String]) -> bool {
        options.is_empty()
            || self
                .item
                .supported_storage
                .iter()
                .flatten()
                .any(|supported| options.contains(supported))
    }

    /// Whether this item carries EC2 capacity of ANY of the instance families a
    /// filter names. MULTI-valued, for the reason [`Self::supports_storage`] is.
    pub fn offers_family(&self, families: &[String]) -> bool {
        families.is_empty()
            || self.item.ec2_capacities.iter().flatten().any(|capacity| {
                capacity
                    .family
                    .as_ref()
                    .is_some_and(|family| families.contains(family))
            })
    }
}

/// The EC2 capacity one configuration carries: one entry per instance FAMILY the
/// hardware runs, naming the largest size of that family and how many of that size
/// the configuration's compute capacity provides.
///
/// `Family` is "the family of the EC2 capacity" and `MaxSize` "the maximum size of
/// the EC2 capacity", so the two members split an instance type between them: the
/// family prefix is reported in `Family` and `MaxSize` carries the SIZE SUFFIX alone
/// (`24xlarge`, not `c5.24xlarge`), which is what makes an entry's instance type the
/// join of the two ([`capacity_instance_type`]) rather than one member repeating the
/// other.
///
/// Which families a configuration runs is not a second catalog: it is the instance
/// types this service holds specifications for ([`instance_type_specifications`])
/// narrowed to the configuration's own form factor and generation, so an item can
/// never offer capacity of a family this service does not offer on that hardware —
/// and the size each entry names is that family's whole host ([`whole_host`]), which
/// is the figure `Quantity` counts.
fn ec2_capacities(
    instance_types: &[OrderableInstanceTypeRecord],
    config: &CatalogConfiguration,
) -> Vec<EC2Capacity> {
    let mut largest: BTreeMap<String, (i32, String)> = BTreeMap::new();
    for record in instance_types
        .iter()
        .filter(|record| record.is_offered_as(config.item_class, config.generation))
    {
        let Some(instance_type) = record.instance_type.instance_type.as_deref() else {
            continue;
        };
        let Some((family, size)) = instance_type.split_once('.') else {
            continue;
        };
        let vcpus = record.instance_type.vcp_us.unwrap_or_default();
        let entry = largest
            .entry(family.to_string())
            .or_insert_with(|| (vcpus, size.to_string()));
        if vcpus > entry.0 {
            *entry = (vcpus, size.to_string());
        }
    }
    largest
        .into_iter()
        .map(|(family, (vcpus, max_size))| EC2Capacity {
            family: Some(family),
            max_size: Some(max_size),
            // How many instances of that largest size the configuration's compute
            // capacity runs, which is the figure `Quantity` reports: a whole number
            // of hosts, and at least one, because a family the hardware runs at all
            // runs one of them.
            quantity: Some((config.capacity_vcpus / vcpus.max(1)).max(1).to_string()),
        })
        .collect()
}

/// The instance type one published EC2 capacity names, or `None` for a capacity
/// naming no family or no size.
///
/// An EC2 instance type is `<family>.<size>` and a published capacity reports those
/// two parts separately ([`ec2_capacities`]), so every reader that needs the type
/// itself — to look up what the catalog publishes for it, or to report what hardware
/// carrying that capacity is configured with — joins them HERE, and no reader has to
/// assume one member carries the other's part.
pub fn capacity_instance_type(capacity: &EC2Capacity) -> Option<String> {
    let family = capacity.family.as_deref()?;
    let max_size = capacity.max_size.as_deref()?;
    Some(format!("{family}.{max_size}"))
}

/// The whole catalog of items this service publishes, expanded from
/// [`CATALOG_ITEMS`] and ordered by `CatalogItemId`.
///
/// The order is by the published SKU so it is stable across processes — a page token
/// names a position in it, so the order a caller pages through must not depend on how
/// the table above happens to be written.
pub fn catalog_item_catalog() -> Vec<CatalogItemRecord> {
    // The catalog may only ever name values the model declares, so the table is
    // checked against the GENERATED value sets rather than trusted to agree with
    // them: a model that renames a class, a status, a storage option or a scaling
    // type trips this in every debug build instead of letting the catalog report an
    // unmodelled member.
    debug_assert!(
        CATALOG_ITEMS.iter().all(|config| {
            crate::ops::types::CATALOG_ITEM_CLASS_VALUES.contains(&config.item_class)
                && crate::ops::types::FORM_FACTOR_VALUES.contains(&config.item_class)
                && crate::ops::types::OUTPOST_GENERATION_VALUES.contains(&config.generation)
                && crate::ops::types::CATALOG_ITEM_STATUS_VALUES.contains(&config.item_status)
                && crate::ops::types::RACK_UNIT_HEIGHT_VALUES.contains(&config.rack_unit_height)
                && config
                    .supported_storage
                    .iter()
                    .all(|option| crate::ops::types::SUPPORTED_STORAGE_ENUM_VALUES.contains(option))
                && config.rack_scaling_type.is_none_or(|scaling| {
                    crate::ops::types::RACK_SCALING_TYPE_VALUES.contains(&scaling)
                })
        }),
        "every catalog configuration names a modelled class, status, storage option, rack unit \
         height and scaling type"
    );
    // A configuration carries S3 capacity exactly when it supports S3 storage: a
    // figure on hardware that supports none would be capacity nothing can provision,
    // and an S3-capable configuration with none would be hardware whose S3 storage
    // holds nothing.
    debug_assert!(
        CATALOG_ITEMS.iter().all(|config| {
            config.supported_storage.contains(&SUPPORTED_STORAGE_S3)
                == (config.s3_capacity_bytes > 0)
        }),
        "a configuration carries S3 capacity exactly when it supports S3 storage"
    );
    let instance_types = instance_type_specifications();
    let mut items: Vec<(i32, i64, &'static str, CatalogItem)> = CATALOG_ITEMS
        .iter()
        .map(|config| {
            (
                config.capacity_vcpus,
                config.s3_capacity_bytes,
                config.generation,
                CatalogItem {
                    catalog_item_id: Some(sku_code(&config.sku_seed())),
                    item_status: Some(config.item_status.to_string()),
                    // The class the configuration is published as, which is also
                    // what `ListCatalogItems$ItemClassFilter` selects on: a filter
                    // narrows which items an answer holds, and the item says of
                    // itself which class it is.
                    item_class: Some(config.item_class.to_string()),
                    current_type: Some(CURRENT_TYPE_AC.to_string()),
                    dimensions: Some(config.dimensions()),
                    rack_unit_height: Some(config.rack_unit_height.to_string()),
                    ec2_capacities: Some(ec2_capacities(&instance_types, config)),
                    power_kva: Some(config.power_kva),
                    weight_lbs: Some(config.weight_lbs),
                    supported_uplink_gbps: Some(config.supported_uplink_gbps.to_vec()),
                    // The uplink speeds this configuration supports for the
                    // connection to the Region, which both uplink members report:
                    // they are the same published fact about the hardware, so both
                    // are read off the one figure rather than kept twice.
                    supported_uplink_gi_b: Some(config.supported_uplink_gbps.to_vec()),
                    supported_storage: Some(
                        config
                            .supported_storage
                            .iter()
                            .map(|option| (*option).to_string())
                            .collect(),
                    ),
                    rack_scaling_type: config
                        .rack_scaling_type
                        .map(|scaling| scaling.to_string()),
                },
            )
        })
        .collect();
    items.sort_by(|(.., left), (.., right)| left.catalog_item_id.cmp(&right.catalog_item_id));
    debug_assert!(
        items
            .windows(2)
            .all(|pair| pair[0].3.catalog_item_id != pair[1].3.catalog_item_id),
        "no two catalog configurations are published under one SkuCode"
    );
    items
        .into_iter()
        .enumerate()
        .map(
            |(index, (capacity_vcpus, s3_capacity_bytes, generation, item))| CatalogItemRecord {
                sequence: u64::try_from(index).unwrap_or_default() + 1,
                capacity_vcpus,
                s3_capacity_bytes,
                generation,
                item,
            },
        )
        .collect()
}
