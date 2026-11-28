// SPDX-License-Identifier: CC0-1.0

//! The instance-type catalog this service reports: the Amazon EC2 instance families EMR
//! offers, the sizes each is offered in, and the facts every size of a family shares.
//!
//! One concern, split out of [`super`] so the catalog table lives beside the code that
//! projects it. It is a fact about the service rather than account state: nothing here is
//! per-caller, nothing is written, and `ListSupportedInstanceTypes` projects the table
//! below through [`supported_instance_types`].

use super::*;

/// The generation half of an [`InstanceFamilySpec::instance_family_id`]: a family Amazon
/// EC2 still offers as current, or one it has superseded.
const CURRENT_GEN: &str = "CURRENT_GEN";
const PREVIOUS_GEN: &str = "PREVIOUS_GEN";

/// One family of the instance-type catalog: an EC2 instance family, the sizes it is
/// offered in, and the facts every size of it shares.
///
/// This is how the catalog is DECLARED. An instance type name is `<family>.<size>`,
/// and within a family vCPUs and memory scale with the size
/// ([`vcpu_for_size`], [`InstanceFamilySpec::memory_gb_per_vcpu`]), so one row per
/// family with its published EC2 size ladder states the whole catalog without hiding
/// a sample behind a hand-picked list of names.
#[derive(Debug, Clone, Copy)]
pub struct InstanceFamilySpec {
    /// The family part of the name: `m5`, `c6g`, `r7g`.
    pub family: &'static str,
    /// The size part of every name in this family: its published EC2 size ladder.
    pub sizes: &'static [&'static str],
    /// GiB of memory per vCPU — the constant that makes this family general purpose
    /// (4), compute optimised (2) or memory optimised (8).
    pub memory_gb_per_vcpu: f32,
    /// The instance-store disks each size of this family is attached, or an EMPTY slice
    /// for an Amazon EBS-only family. See [`InstanceStoreDisks`].
    pub instance_store: &'static [InstanceStoreDisks],
    /// The [`family_class`] value of this family's workload class.
    pub class: &'static str,
    /// Whether Amazon EC2 offers this family as a current generation, which is the
    /// generation half of its [`InstanceFamilySpec::instance_family_id`].
    pub current_generation: bool,
    /// The `architecture::` value of this family's processors.
    pub architecture: &'static str,
    /// Whether EBS optimization is on by default for this family, rather than merely
    /// available (`SupportedInstanceType$EbsOptimizedByDefault`).
    pub ebs_optimized_by_default: bool,
    /// The FIRST release of each major series that supports this family, as a
    /// `(major, minor, patch)` triple, oldest major first. One entry per major series
    /// the introduction point differs in: Graviton reached `emr-5.31.0` and
    /// `emr-6.1.0`, so it carries both, and a family a major series never offered
    /// simply has no entry for it. See [`InstanceFamilySpec::supported_by`].
    pub introduced_in: &'static [(u32, u32, u32)],
}

/// The instance-store disks ONE size of a family is attached: how many, and the
/// capacity of each.
///
/// A size's `StorageGB` is the capacity the instance type carries
/// (`number_of_disks * gb_per_disk`), and `NumberOfDisks` is `number_of_disks`; both
/// are 0 for an Amazon EBS-only type, which is a family declaring no disks at all.
/// Instance-store capacity does NOT follow the size ladder the way vCPUs and memory do
/// — `i3.large` carries one 475 GB disk and `i3.xlarge` one 950 GB disk, and a family
/// jumps from one disk to several part way up its ladder — so it is declared per size
/// rather than derived.
#[derive(Debug, Clone, Copy)]
pub struct InstanceStoreDisks {
    /// The size this row is for, as it appears on the family's [size ladder].
    ///
    /// [size ladder]: InstanceFamilySpec::sizes
    pub size: &'static str,
    /// How many instance-store disks that size is attached:
    /// `SupportedInstanceType$NumberOfDisks`.
    pub number_of_disks: i32,
    /// The capacity of ONE of those disks, in GB, as Amazon EC2 publishes it.
    pub gb_per_disk: i32,
}

impl InstanceFamilySpec {
    /// Whether the release `release` — a `(major, minor, patch)` triple, as
    /// [`catalog_release`] resolves a label to — supports this family.
    ///
    /// Support is carried forward: the first release of a major series that offers a
    /// family is the point it becomes supported in that series, and every later major
    /// series keeps it unless it declares a later introduction point of its own.
    pub fn supported_by(&self, release: (u32, u32, u32)) -> bool {
        let mut carried = false;
        for introduced in self.introduced_in {
            if introduced.0 == release.0 {
                return release >= *introduced;
            }
            if introduced.0 < release.0 {
                carried = true;
            }
        }
        carried
    }

    /// EMR's `SupportedInstanceType$InstanceFamilyId` for every size of this family:
    /// its workload class and its generation, joined — `GENERAL_CURRENT_GEN` for a
    /// current-generation general-purpose family.
    pub fn instance_family_id(&self) -> String {
        let generation = if self.current_generation {
            CURRENT_GEN
        } else {
            PREVIOUS_GEN
        };
        format!("{}_{generation}", self.class)
    }

    /// This family's rows: one [`SupportedInstanceType`] per size on its ladder,
    /// smallest first.
    pub fn instance_types(&self) -> Vec<SupportedInstanceType> {
        self.sizes
            .iter()
            .filter_map(|size| {
                let scaled = vcpu_for_size(size);
                // A ladder entry this catalog cannot scale would silently shrink a
                // release's list, so a typo in the table fails the tests rather than
                // quietly dropping a row.
                debug_assert!(
                    scaled.is_some(),
                    "catalog size {size:?} of family {} must scale",
                    self.family
                );
                let vcpu = scaled?;
                // "StorageGB represents the storage capacity … This value is 0 for
                // Amazon EBS-only instance types", and "Number of disks … This value
                // is 0 for Amazon EBS-only instance types": a family declaring no
                // instance-store disks reports both as 0 and is EBS storage only, and
                // one declaring them reports what the SIZE carries.
                let disks = self.disks_of(size);
                let number_of_disks = disks.map_or(0, |disks| disks.number_of_disks);
                let storage_gb = disks.map_or(0, |disks| disks.number_of_disks * disks.gb_per_disk);
                Some(SupportedInstanceType {
                    r#type: Some(format!("{}.{size}", self.family)),
                    memory_gb: Some(vcpu as f32 * self.memory_gb_per_vcpu),
                    storage_gb: Some(storage_gb),
                    vcpu: Some(vcpu),
                    // Every family here runs a 64-bit processor only.
                    is64_bits_only: Some(true),
                    instance_family_id: Some(self.instance_family_id()),
                    ebs_optimized_available: Some(true),
                    ebs_optimized_by_default: Some(self.ebs_optimized_by_default),
                    number_of_disks: Some(number_of_disks),
                    // Read off the SAME lookup as the two members above, so the three
                    // always agree: a type carrying disks is not EBS storage only.
                    ebs_storage_only: Some(disks.is_none()),
                    architecture: Some(self.architecture.to_string()),
                })
            })
            .collect()
    }

    /// The instance-store disks the size `size` of this family carries, or `None` when
    /// it carries none: every size of an Amazon EBS-only family.
    fn disks_of(&self, size: &str) -> Option<&'static InstanceStoreDisks> {
        // An instance-store family declares one row per size on its ladder, so a size
        // with no row in a non-empty table is a hole in the table rather than an
        // EBS-only size, and a typo in it fails the tests rather than reporting a
        // storage-optimised type as EBS-only.
        debug_assert!(
            self.instance_store.is_empty()
                || self.instance_store.iter().any(|disks| disks.size == size),
            "catalog family {} declares instance-store disks but none for size {size:?}",
            self.family
        );
        self.instance_store.iter().find(|disks| disks.size == size)
    }
}

/// GiB of memory per vCPU of a general-purpose (`m`) family, and of the accelerated
/// (`g`) families, which carry the same ratio.
const MEMORY_GENERAL_PURPOSE: f32 = 4.0;
/// GiB of memory per vCPU of a compute-optimised (`c`) family.
const MEMORY_COMPUTE_OPTIMIZED: f32 = 2.0;
/// GiB of memory per vCPU of a memory-optimised (`r`) family, and of the Nitro
/// storage-optimised families (`i3en`, `d3`), which carry the same ratio.
const MEMORY_MEMORY_OPTIMIZED: f32 = 8.0;
/// GiB of memory per vCPU of the Xen-generation memory-, storage- and
/// accelerator-optimised families (`r4`, `i3`, `p3`), which carry 7.625 rather than the
/// round 8 their Nitro successors do — `i3.large`'s 15.25 GiB over 2 vCPUs.
const MEMORY_XEN_MEMORY_OPTIMIZED: f32 = 7.625;

/// The size ladder of the fifth-generation Intel families (`m5`, `r5`) and the AMD
/// ones (`m5a`, `r5a`).
const SIZES_5TH_GENERATION: &[&str] = &[
    "large", "xlarge", "2xlarge", "4xlarge", "8xlarge", "12xlarge", "16xlarge", "24xlarge",
];
/// The `c5` ladder, which steps through `9xlarge` and `18xlarge`.
const SIZES_C5: &[&str] = &[
    "large", "xlarge", "2xlarge", "4xlarge", "9xlarge", "12xlarge", "18xlarge", "24xlarge",
];
/// The sixth- and seventh-generation Intel ladder (`m6i`, `c6i`, `r6i`), which adds
/// `32xlarge`.
const SIZES_6TH_GENERATION_INTEL: &[&str] = &[
    "large", "xlarge", "2xlarge", "4xlarge", "8xlarge", "12xlarge", "16xlarge", "24xlarge",
    "32xlarge",
];
/// The sixth-generation AMD ladder (`m6a`, `c6a`, `r6a`), which goes up to `48xlarge`.
const SIZES_6TH_GENERATION_AMD: &[&str] = &[
    "large", "xlarge", "2xlarge", "4xlarge", "8xlarge", "12xlarge", "16xlarge", "24xlarge",
    "32xlarge", "48xlarge",
];
/// The seventh-generation AMD ladder (`m7a`, `c7a`, `r7a`), which is the sixth's. The
/// sub-`large` `medium` size these families also come in is outside this catalog, the
/// same way Graviton's is.
const SIZES_7TH_GENERATION_AMD: &[&str] = SIZES_6TH_GENERATION_AMD;
/// The seventh-generation Intel ladder (`m7i`, `c7i`, `r7i`), which steps from
/// `24xlarge` to `48xlarge`.
const SIZES_7TH_GENERATION_INTEL: &[&str] = &[
    "large", "xlarge", "2xlarge", "4xlarge", "8xlarge", "12xlarge", "16xlarge", "24xlarge",
    "48xlarge",
];
/// The Graviton ladder (`m6g`, `c6g`, `r6g`, `m7g`, `c7g`, `r7g`) from `large` up.
/// The sub-`large` `medium` size each of these families also comes in is outside this
/// catalog: EMR runs no cluster node smaller than `large`.
const SIZES_GRAVITON: &[&str] = &[
    "large", "xlarge", "2xlarge", "4xlarge", "8xlarge", "12xlarge", "16xlarge",
];
/// The Xen-generation ladder of `r4` and `i3`, which doubles from `large` to
/// `16xlarge`.
const SIZES_LARGE_TO_16XLARGE: &[&str] = &[
    "large", "xlarge", "2xlarge", "4xlarge", "8xlarge", "16xlarge",
];
/// The `m4` ladder, which steps through `10xlarge`.
const SIZES_M4: &[&str] = &[
    "large", "xlarge", "2xlarge", "4xlarge", "10xlarge", "16xlarge",
];
/// The `i3en` ladder, which steps through `3xlarge` and `6xlarge` and tops out at
/// `24xlarge`.
const SIZES_I3EN: &[&str] = &[
    "large", "xlarge", "2xlarge", "3xlarge", "6xlarge", "12xlarge", "24xlarge",
];
/// The dense-HDD `d3` ladder, which starts at `xlarge`.
const SIZES_D3: &[&str] = &["xlarge", "2xlarge", "4xlarge", "8xlarge"];
/// The `g4dn` ladder, which starts at `xlarge` and tops out at `16xlarge`.
const SIZES_G4DN: &[&str] = &[
    "xlarge", "2xlarge", "4xlarge", "8xlarge", "12xlarge", "16xlarge",
];
/// The `g5` ladder, which carries on past `16xlarge` to `24xlarge` and `48xlarge`.
const SIZES_G5: &[&str] = &[
    "xlarge", "2xlarge", "4xlarge", "8xlarge", "12xlarge", "16xlarge", "24xlarge", "48xlarge",
];
/// The `p3` ladder: three sizes, the smallest `2xlarge`.
const SIZES_P3: &[&str] = &["2xlarge", "8xlarge", "16xlarge"];

/// The NVMe SSD disks each `i3` size carries (`i3.4xlarge`: 2 × 1,900 GB).
const DISKS_I3: &[InstanceStoreDisks] = &[
    disks("large", 1, 475),
    disks("xlarge", 1, 950),
    disks("2xlarge", 1, 1900),
    disks("4xlarge", 2, 1900),
    disks("8xlarge", 4, 1900),
    disks("16xlarge", 8, 1900),
];
/// The NVMe SSD disks each `i3en` size carries.
const DISKS_I3EN: &[InstanceStoreDisks] = &[
    disks("large", 1, 1250),
    disks("xlarge", 1, 2500),
    disks("2xlarge", 2, 2500),
    disks("3xlarge", 1, 7500),
    disks("6xlarge", 2, 7500),
    disks("12xlarge", 4, 7500),
    disks("24xlarge", 8, 7500),
];
/// The HDD disks each `d3` size carries: `1,980 GB` spindles, three per `xlarge`.
const DISKS_D3: &[InstanceStoreDisks] = &[
    disks("xlarge", 3, 1980),
    disks("2xlarge", 6, 1980),
    disks("4xlarge", 12, 1980),
    disks("8xlarge", 24, 1980),
];
/// The NVMe SSD disk each `g4dn` size carries: one, growing with the size.
const DISKS_G4DN: &[InstanceStoreDisks] = &[
    disks("xlarge", 1, 125),
    disks("2xlarge", 1, 225),
    disks("4xlarge", 1, 225),
    disks("8xlarge", 1, 900),
    disks("12xlarge", 1, 900),
    disks("16xlarge", 1, 900),
];
/// The NVMe SSD disks each `g5` size carries.
const DISKS_G5: &[InstanceStoreDisks] = &[
    disks("xlarge", 1, 250),
    disks("2xlarge", 1, 450),
    disks("4xlarge", 1, 600),
    disks("8xlarge", 1, 900),
    disks("12xlarge", 1, 3800),
    disks("16xlarge", 1, 1900),
    disks("24xlarge", 1, 3800),
    disks("48xlarge", 2, 3800),
];

/// One row of a family's instance-store disk table, so the tables above read as the
/// published "<n> × <capacity>" pairs.
const fn disks(size: &'static str, number_of_disks: i32, gb_per_disk: i32) -> InstanceStoreDisks {
    InstanceStoreDisks {
        size,
        number_of_disks,
        gb_per_disk,
    }
}

/// The instance families this emulator's EMR catalog publishes, each with the sizes
/// it is offered in, its memory ratio, the instance-store disks its sizes carry, its
/// architecture and the release it became supported in.
///
/// SOURCE: the Amazon EMR Management Guide's "Supported instance types" table (which
/// instance types each release supports) crossed with each family's published EC2
/// size ladder and instance-store configuration above. Membership, the introduction
/// points and the hardware facts are catalog data, not modelled facts: the model
/// enumerates no instance type at all. Refresh them from that page.
///
/// The five workload classes of [`family_class`] are all published, each under the
/// generation its families belong to: the EBS-only general-purpose, compute- and
/// memory-optimised families; the instance-store storage-optimised ones (`i3`, `i3en`,
/// `d3`), whose sizes report the disks they carry; the accelerated ones (`g4dn`, `g5`,
/// `p3`); and the previous-generation families EMR still lists (`m4`, `r4`). A release
/// supports the families its OWN series reached, so the newest 7.x catalogue is not the
/// newest 6.x one: the seventh-generation AMD families arrived after the final 6.x
/// release.
///
/// What is deliberately OUTSIDE it: the instance-store variants of the EBS-only
/// families (`m5d`, `c5d`, `r5d`, `z1d`), which add no workload class or generation
/// this catalog does not already publish; burstable (`t`) families and sizes below
/// `large`, which EMR does not run cluster nodes on; bare-metal sizes; and the
/// families whose published ladder does NOT scale vCPUs and memory uniformly with the
/// size (`c4` and `d2`, whose `8xlarge` carries 36 vCPUs, and `p2`), which this
/// catalog cannot state without a per-size vCPU table.
pub const INSTANCE_FAMILIES: &[InstanceFamilySpec] = &[
    // General purpose, x86-64.
    InstanceFamilySpec {
        family: "m5",
        sizes: SIZES_5TH_GENERATION,
        memory_gb_per_vcpu: MEMORY_GENERAL_PURPOSE,
        instance_store: &[],
        class: family_class::GENERAL,
        current_generation: true,
        architecture: architecture::X86_64,
        ebs_optimized_by_default: true,
        introduced_in: &[(5, 13, 0)],
    },
    InstanceFamilySpec {
        family: "m5a",
        sizes: SIZES_5TH_GENERATION,
        memory_gb_per_vcpu: MEMORY_GENERAL_PURPOSE,
        instance_store: &[],
        class: family_class::GENERAL,
        current_generation: true,
        architecture: architecture::X86_64,
        ebs_optimized_by_default: true,
        introduced_in: &[(5, 20, 0)],
    },
    InstanceFamilySpec {
        family: "m6i",
        sizes: SIZES_6TH_GENERATION_INTEL,
        memory_gb_per_vcpu: MEMORY_GENERAL_PURPOSE,
        instance_store: &[],
        class: family_class::GENERAL,
        current_generation: true,
        architecture: architecture::X86_64,
        ebs_optimized_by_default: true,
        introduced_in: &[(6, 5, 0)],
    },
    InstanceFamilySpec {
        family: "m6a",
        sizes: SIZES_6TH_GENERATION_AMD,
        memory_gb_per_vcpu: MEMORY_GENERAL_PURPOSE,
        instance_store: &[],
        class: family_class::GENERAL,
        current_generation: true,
        architecture: architecture::X86_64,
        ebs_optimized_by_default: true,
        introduced_in: &[(6, 6, 0)],
    },
    InstanceFamilySpec {
        family: "m7i",
        sizes: SIZES_7TH_GENERATION_INTEL,
        memory_gb_per_vcpu: MEMORY_GENERAL_PURPOSE,
        instance_store: &[],
        class: family_class::GENERAL,
        current_generation: true,
        architecture: architecture::X86_64,
        ebs_optimized_by_default: true,
        introduced_in: &[(6, 15, 0)],
    },
    // General purpose, Graviton.
    InstanceFamilySpec {
        family: "m6g",
        sizes: SIZES_GRAVITON,
        memory_gb_per_vcpu: MEMORY_GENERAL_PURPOSE,
        instance_store: &[],
        class: family_class::GENERAL,
        current_generation: true,
        architecture: architecture::AARCH64,
        ebs_optimized_by_default: true,
        introduced_in: &[(5, 31, 0), (6, 1, 0)],
    },
    InstanceFamilySpec {
        family: "m7g",
        sizes: SIZES_GRAVITON,
        memory_gb_per_vcpu: MEMORY_GENERAL_PURPOSE,
        instance_store: &[],
        class: family_class::GENERAL,
        current_generation: true,
        architecture: architecture::AARCH64,
        ebs_optimized_by_default: true,
        introduced_in: &[(6, 15, 0)],
    },
    // Compute optimised.
    InstanceFamilySpec {
        family: "c5",
        sizes: SIZES_C5,
        memory_gb_per_vcpu: MEMORY_COMPUTE_OPTIMIZED,
        instance_store: &[],
        class: family_class::COMPUTE,
        current_generation: true,
        architecture: architecture::X86_64,
        ebs_optimized_by_default: true,
        introduced_in: &[(5, 13, 0)],
    },
    InstanceFamilySpec {
        family: "c6i",
        sizes: SIZES_6TH_GENERATION_INTEL,
        memory_gb_per_vcpu: MEMORY_COMPUTE_OPTIMIZED,
        instance_store: &[],
        class: family_class::COMPUTE,
        current_generation: true,
        architecture: architecture::X86_64,
        ebs_optimized_by_default: true,
        introduced_in: &[(6, 5, 0)],
    },
    InstanceFamilySpec {
        family: "c7i",
        sizes: SIZES_7TH_GENERATION_INTEL,
        memory_gb_per_vcpu: MEMORY_COMPUTE_OPTIMIZED,
        instance_store: &[],
        class: family_class::COMPUTE,
        current_generation: true,
        architecture: architecture::X86_64,
        ebs_optimized_by_default: true,
        introduced_in: &[(6, 15, 0)],
    },
    InstanceFamilySpec {
        family: "c6g",
        sizes: SIZES_GRAVITON,
        memory_gb_per_vcpu: MEMORY_COMPUTE_OPTIMIZED,
        instance_store: &[],
        class: family_class::COMPUTE,
        current_generation: true,
        architecture: architecture::AARCH64,
        ebs_optimized_by_default: true,
        introduced_in: &[(5, 31, 0), (6, 1, 0)],
    },
    InstanceFamilySpec {
        family: "c7g",
        sizes: SIZES_GRAVITON,
        memory_gb_per_vcpu: MEMORY_COMPUTE_OPTIMIZED,
        instance_store: &[],
        class: family_class::COMPUTE,
        current_generation: true,
        architecture: architecture::AARCH64,
        ebs_optimized_by_default: true,
        introduced_in: &[(6, 15, 0)],
    },
    // Memory optimised.
    InstanceFamilySpec {
        family: "r5",
        sizes: SIZES_5TH_GENERATION,
        memory_gb_per_vcpu: MEMORY_MEMORY_OPTIMIZED,
        instance_store: &[],
        class: family_class::HI_MEM,
        current_generation: true,
        architecture: architecture::X86_64,
        ebs_optimized_by_default: true,
        introduced_in: &[(5, 20, 0)],
    },
    InstanceFamilySpec {
        family: "r6i",
        sizes: SIZES_6TH_GENERATION_INTEL,
        memory_gb_per_vcpu: MEMORY_MEMORY_OPTIMIZED,
        instance_store: &[],
        class: family_class::HI_MEM,
        current_generation: true,
        architecture: architecture::X86_64,
        ebs_optimized_by_default: true,
        introduced_in: &[(6, 5, 0)],
    },
    InstanceFamilySpec {
        family: "r6g",
        sizes: SIZES_GRAVITON,
        memory_gb_per_vcpu: MEMORY_MEMORY_OPTIMIZED,
        instance_store: &[],
        class: family_class::HI_MEM,
        current_generation: true,
        architecture: architecture::AARCH64,
        ebs_optimized_by_default: true,
        introduced_in: &[(5, 31, 0), (6, 1, 0)],
    },
    InstanceFamilySpec {
        family: "r7g",
        sizes: SIZES_GRAVITON,
        memory_gb_per_vcpu: MEMORY_MEMORY_OPTIMIZED,
        instance_store: &[],
        class: family_class::HI_MEM,
        current_generation: true,
        architecture: architecture::AARCH64,
        ebs_optimized_by_default: true,
        introduced_in: &[(6, 15, 0)],
    },
    // Storage optimised: instance-store families, so every size reports the disks it
    // carries rather than 0.
    InstanceFamilySpec {
        family: "i3",
        sizes: SIZES_LARGE_TO_16XLARGE,
        memory_gb_per_vcpu: MEMORY_XEN_MEMORY_OPTIMIZED,
        instance_store: DISKS_I3,
        class: family_class::STORAGE,
        current_generation: true,
        architecture: architecture::X86_64,
        ebs_optimized_by_default: true,
        introduced_in: &[(5, 9, 0)],
    },
    InstanceFamilySpec {
        family: "i3en",
        sizes: SIZES_I3EN,
        memory_gb_per_vcpu: MEMORY_MEMORY_OPTIMIZED,
        instance_store: DISKS_I3EN,
        class: family_class::STORAGE,
        current_generation: true,
        architecture: architecture::X86_64,
        ebs_optimized_by_default: true,
        introduced_in: &[(5, 25, 0)],
    },
    InstanceFamilySpec {
        family: "d3",
        sizes: SIZES_D3,
        memory_gb_per_vcpu: MEMORY_MEMORY_OPTIMIZED,
        instance_store: DISKS_D3,
        class: family_class::STORAGE,
        current_generation: true,
        architecture: architecture::X86_64,
        ebs_optimized_by_default: true,
        introduced_in: &[(6, 5, 0)],
    },
    // Accelerated computing: the GPU families. `g4dn` and `g5` carry instance-store
    // NVMe, `p3` is EBS-only.
    InstanceFamilySpec {
        family: "g4dn",
        sizes: SIZES_G4DN,
        memory_gb_per_vcpu: MEMORY_GENERAL_PURPOSE,
        instance_store: DISKS_G4DN,
        class: family_class::GPU,
        current_generation: true,
        architecture: architecture::X86_64,
        ebs_optimized_by_default: true,
        introduced_in: &[(5, 30, 0)],
    },
    InstanceFamilySpec {
        family: "g5",
        sizes: SIZES_G5,
        memory_gb_per_vcpu: MEMORY_GENERAL_PURPOSE,
        instance_store: DISKS_G5,
        class: family_class::GPU,
        current_generation: true,
        architecture: architecture::X86_64,
        ebs_optimized_by_default: true,
        introduced_in: &[(6, 9, 0)],
    },
    InstanceFamilySpec {
        family: "p3",
        sizes: SIZES_P3,
        memory_gb_per_vcpu: MEMORY_XEN_MEMORY_OPTIMIZED,
        instance_store: &[],
        class: family_class::GPU,
        current_generation: true,
        architecture: architecture::X86_64,
        ebs_optimized_by_default: true,
        introduced_in: &[(5, 10, 0)],
    },
    // The previous-generation families Amazon EMR still lists, which is the other half
    // of every `InstanceFamilyId`: a workload class crossed with its generation.
    InstanceFamilySpec {
        family: "m4",
        sizes: SIZES_M4,
        memory_gb_per_vcpu: MEMORY_GENERAL_PURPOSE,
        instance_store: &[],
        class: family_class::GENERAL,
        current_generation: false,
        architecture: architecture::X86_64,
        ebs_optimized_by_default: true,
        // The 4.x series offers this family too: it is the generation those releases were
        // published alongside, so the oldest releases the catalog publishes provision from
        // it.
        introduced_in: &[(4, 2, 0), (5, 0, 0)],
    },
    InstanceFamilySpec {
        family: "r4",
        sizes: SIZES_LARGE_TO_16XLARGE,
        memory_gb_per_vcpu: MEMORY_XEN_MEMORY_OPTIMIZED,
        instance_store: &[],
        class: family_class::HI_MEM,
        current_generation: false,
        architecture: architecture::X86_64,
        ebs_optimized_by_default: true,
        introduced_in: &[(5, 0, 0)],
    },
    // The families the 7.x series added, which no 6.x release offers: the final 6.x
    // release predates them.
    InstanceFamilySpec {
        family: "m7a",
        sizes: SIZES_7TH_GENERATION_AMD,
        memory_gb_per_vcpu: MEMORY_GENERAL_PURPOSE,
        instance_store: &[],
        class: family_class::GENERAL,
        current_generation: true,
        architecture: architecture::X86_64,
        ebs_optimized_by_default: true,
        introduced_in: &[(7, 0, 0)],
    },
    InstanceFamilySpec {
        family: "c7a",
        sizes: SIZES_7TH_GENERATION_AMD,
        memory_gb_per_vcpu: MEMORY_COMPUTE_OPTIMIZED,
        instance_store: &[],
        class: family_class::COMPUTE,
        current_generation: true,
        architecture: architecture::X86_64,
        ebs_optimized_by_default: true,
        introduced_in: &[(7, 0, 0)],
    },
    InstanceFamilySpec {
        family: "r7a",
        sizes: SIZES_7TH_GENERATION_AMD,
        memory_gb_per_vcpu: MEMORY_MEMORY_OPTIMIZED,
        instance_store: &[],
        class: family_class::HI_MEM,
        current_generation: true,
        architecture: architecture::X86_64,
        ebs_optimized_by_default: true,
        introduced_in: &[(7, 0, 0)],
    },
];

/// The vCPU count of the size `size` on an EC2 size ladder, or `None` for a size this
/// catalog does not scale (`medium`, `metal`).
///
/// EC2 sizes a family in units of two vCPUs: `large` is 2, `xlarge` is 4, and
/// `<n>xlarge` is `4 × n` — `m5.24xlarge`'s 96 vCPUs, say.
pub fn vcpu_for_size(size: &str) -> Option<i32> {
    const VCPU_PER_XLARGE: i32 = 4;
    match size {
        "large" => Some(VCPU_PER_XLARGE / 2),
        "xlarge" => Some(VCPU_PER_XLARGE),
        _ => {
            let multiple = size.strip_suffix("xlarge")?;
            if multiple.is_empty() || !multiple.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            multiple
                .parse::<i32>()
                .ok()
                // A multiple no EC2 ladder reaches is refused rather than wrapped:
                // the size is not one this catalog can scale.
                .and_then(|multiple| multiple.checked_mul(VCPU_PER_XLARGE))
        }
    }
}

/// The CPU architecture one instance of `instance_type` runs — the `architecture::` value this
/// catalog carries for its family, which is what `SupportedInstanceType$Architecture` reports
/// ("the CPU architecture, for example `X86_64` or `AARCH64`") — or `None` for a value this
/// catalog names no instance type under.
///
/// The ONE reading of that fact, so what a release's catalog reports and what a cluster's nodes
/// are really launched from cannot disagree ([`super::cluster_instances`] picks the image a
/// node boots by it: machine code built for one architecture does not run on the other).
#[must_use]
pub fn instance_type_architecture(instance_type: &str) -> Option<&'static str> {
    let (family, size) = instance_type.split_once('.')?;
    INSTANCE_FAMILIES
        .iter()
        .find(|spec| spec.family == family && spec.sizes.contains(&size))
        .map(|spec| spec.architecture)
}

/// The memory, in MiB, one instance of `instance_type` carries, or `None` for a value this
/// catalog names no instance type under.
///
/// The same figure [`InstanceFamilySpec::instance_types`] reports as
/// `SupportedInstanceType$MemoryGB` — a size's vCPU count times its family's memory per vCPU
/// — in the unit every memory a scheduler allocates is stated in, so one table answers both.
pub fn instance_type_memory_mib(instance_type: &str) -> Option<i64> {
    const MIB_PER_GIB: f32 = 1024.0;
    let (family, size) = instance_type.split_once('.')?;
    let spec = INSTANCE_FAMILIES
        .iter()
        .find(|spec| spec.family == family && spec.sizes.contains(&size))?;
    let vcpu = vcpu_for_size(size)?;
    Some((vcpu as f32 * spec.memory_gb_per_vcpu * MIB_PER_GIB) as i64)
}

/// The instance types the release `release` — a `(major, minor, patch)` triple, as
/// [`catalog_release`] resolves a label to — supports, in catalog order: by family as
/// [`INSTANCE_FAMILIES`] declares them, and within a family smallest size first.
pub fn supported_instance_types(release: (u32, u32, u32)) -> Vec<SupportedInstanceType> {
    INSTANCE_FAMILIES
        .iter()
        .filter(|family| family.supported_by(release))
        .flat_map(InstanceFamilySpec::instance_types)
        .collect()
}

/// Whether the release `release` — a `(major, minor, patch)` triple, as
/// [`catalog_release`] resolves a label to — offers the instance type `instance_type`.
///
/// An instance type is named `<family>.<size>`, so a type a release offers is a size on
/// the published ladder of a family that release supports: the same membership
/// [`supported_instance_types`] projects ("the list of instance types that the release
/// specified … supports", `ListSupportedInstanceTypesOutput$SupportedInstanceTypes`
/// doc), decided without building that list. A value that is not a `<family>.<size>`
/// name at all names no instance type, so no release offers it.
pub fn release_offers_instance_type(release: (u32, u32, u32), instance_type: &str) -> bool {
    let Some((family, size)) = instance_type.split_once('.') else {
        return false;
    };
    INSTANCE_FAMILIES.iter().any(|spec| {
        spec.family == family && spec.sizes.contains(&size) && spec.supported_by(release)
    })
}

/// Whether `instance_type` names an Amazon EC2 instance type at all: a size on the
/// published ladder of a family this catalog carries, whatever release offers it.
///
/// An instance type is named `<family>.<size>`, so a value that is not that shape, names a
/// family no ladder carries, or names a size its family's ladder does not step through,
/// names no instance type — which is a different thing from a real instance type an Amazon
/// EMR release predates ([`release_offers_instance_type`]), and the two are refused by
/// different sentences ([`unknown_instance_type_message`],
/// [`instance_type_unsupported_for_release_message`]).
pub fn is_catalogued_instance_type(instance_type: &str) -> bool {
    let Some((family, size)) = instance_type.split_once('.') else {
        return false;
    };
    INSTANCE_FAMILIES
        .iter()
        .any(|spec| spec.family == family && spec.sizes.contains(&size))
}

/// How Amazon EMR refuses a value that names no instance type at all
/// ([`is_catalogued_instance_type`]).
///
/// Recorded behavior for the text, whose fixed parts are `Instance type '` and
/// `' is not supported.`: the one variable part is the value, quoted and verbatim from the
/// member of the request that named it, so a caller that mistyped a type reads back exactly
/// what it sent. The sentence names no release, because the value names no instance type
/// any release could offer.
pub fn unknown_instance_type_message(instance_type: &str) -> String {
    format!("Instance type '{instance_type}' is not supported.")
}

/// How Amazon EMR refuses a real instance type that the release the request names does not
/// offer ([`release_offers_instance_type`]).
///
/// Recorded behavior for the text. Its fixed parts are `Instance type ` and
/// ` not supported for release `; its two variable parts both come from the request — the
/// instance type, verbatim and unquoted, and the release label it was resolved against — so
/// a caller that asked for a type its release predates reads both the type and the release
/// out of the sentence.
pub fn instance_type_unsupported_for_release_message(
    instance_type: &str,
    release_label: &str,
) -> String {
    format!("Instance type {instance_type} not supported for release {release_label}.")
}

/// How Amazon EMR refuses an instance type a launch against the release `release_label`
/// may not provision from: the sentence of the class the value falls in — a value that
/// names no instance type at all ([`unknown_instance_type_message`]), or a real instance
/// type that release does not offer ([`instance_type_unsupported_for_release_message`]).
pub fn unsupported_instance_type_message(instance_type: &str, release_label: &str) -> String {
    if is_catalogued_instance_type(instance_type) {
        return instance_type_unsupported_for_release_message(instance_type, release_label);
    }
    unknown_instance_type_message(instance_type)
}

/// Every instance type a launch would provision capacity from, in request order.
///
/// A launch describes its capacity one of three ways, and this is all of them: the
/// `InstanceTypeConfigs` of every instance fleet it names ("an array of
/// `InstanceTypeConfig` objects that specify how Amazon EMR provisions Amazon EC2
/// instances", `InstanceFleetConfig$InstanceTypeConfigs` doc) — EVERY fleet, whatever
/// role it fills — then every instance group it names ("the Amazon EC2 instance type for
/// all instances in the instance group", `InstanceGroupConfig$InstanceType` doc), and
/// where it named no collection at all, the groups its uniform
/// `MasterInstanceType`/`SlaveInstanceType` form implies
/// ([`implied_instance_group_configs`]).
///
/// `instance_groups` is passed beside `instances` because the launch handler TAKES the
/// submitted groups out of its input before anything resolves them.
pub fn launch_instance_types(
    instances: &JobFlowInstancesConfig,
    instance_groups: &[InstanceGroupConfig],
) -> Vec<String> {
    let mut instance_types: Vec<String> = instances
        .instance_fleets
        .as_deref()
        .unwrap_or_default()
        .iter()
        .flat_map(|fleet| fleet.instance_type_configs.as_deref().unwrap_or_default())
        .map(|config| config.instance_type.clone())
        .collect();
    if !instance_groups.is_empty() {
        instance_types.extend(
            instance_groups
                .iter()
                .map(|group| group.instance_type.clone()),
        );
    } else if instance_types.is_empty() {
        instance_types.extend(
            implied_instance_group_configs(instances)
                .into_iter()
                .map(|group| group.instance_type),
        );
    }
    instance_types
}

/// The first instance type this launch names that the release `release` does not offer,
/// if it names one — the value [`unsupported_instance_type_message`] refuses.
///
/// The types are judged in the order the request states them
/// ([`launch_instance_types`]), so a launch that gets several wrong is refused for the
/// first.
pub fn unsupported_launch_instance_type(
    release: (u32, u32, u32),
    instances: &JobFlowInstancesConfig,
    instance_groups: &[InstanceGroupConfig],
) -> Option<String> {
    launch_instance_types(instances, instance_groups)
        .into_iter()
        .find(|instance_type| !release_offers_instance_type(release, instance_type))
}

/// `ListSupportedInstanceTypes` page size.
///
/// CHOSEN, not documented: the operation is paginated on `Marker` but AWS publishes no
/// page size for it. 100 matches the maximum page the service's other paginated
/// catalog reader models (`MaxResultsNumber`, `range 1..100`), so a caller that
/// follows the marker walks the whole catalog in pages of a familiar size.
pub const LIST_SUPPORTED_INSTANCE_TYPES_PAGE_SIZE: usize = 100;
