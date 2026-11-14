// SPDX-License-Identifier: CC0-1.0

//! The per-instance-type hardware facts AWS publishes, as a table.
//!
//! An instance type's size ladder — its vCPUs, its cores and threads, its memory — follows
//! from its NAME, and the module above derives it that way. Its HARDWARE does not: how many
//! local devices it carries and how big they are, the EBS bandwidth and IOPS AWS dedicates
//! to it, how many volumes it can hold attached and whether that limit is its own, its
//! network performance and interface limits, its processor and that processor's sustained
//! frequency, its accelerators, and which host features it is offered with (Dedicated Hosts,
//! CloudWatch action based recovery, hibernation, reboot migration, in-transit encryption)
//! are FACTS of the type that AWS publishes per type. No function of the name yields them:
//! two types of one size differ in all of them, and two sizes of one family do not scale by
//! the size multiplier.
//!
//! So they are held here, one row per instance type, and
//! [`super::instance_type_info`] reports what the row carries. A column a row leaves unset
//! is one the derivation above answers for every type of the catalog, and a type with no row
//! is described by that derivation alone — the table grows as AWS's published figures for
//! more of the catalog are recorded here, and nothing about the way a row is read depends on
//! which types have one.

use super::*;
use crate::ops::types::{
    FpgaDeviceInfo, FpgaDeviceMemoryInfo, FpgaInfo, GpuDeviceInfo, GpuDeviceMemoryInfo, GpuInfo,
    InferenceAcceleratorInfo, InferenceDeviceInfo, InferenceDeviceMemoryInfo,
};

/// The EBS-optimized performance AWS publishes for one instance type: the two bandwidth and
/// the two IOPS figures of its row in "Amazon EBS optimized instance types".
///
/// A size with dedicated bandwidth publishes its sustained figure as both its baseline and
/// its maximum; a size below the family's dedicated row publishes a baseline it bursts above
/// up to that maximum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PublishedEbsPerformance {
    /// The bandwidth, in Mbps, the type sustains.
    pub baseline_bandwidth_mbps: i32,
    /// The I/O operations per second it sustains.
    pub baseline_iops: i32,
    /// The bandwidth, in Mbps, it can reach.
    pub maximum_bandwidth_mbps: i32,
    /// The I/O operations per second it can reach.
    pub maximum_iops: i32,
}

impl PublishedEbsPerformance {
    /// This row as the modelled `EbsOptimizedInfo`.
    ///
    /// The throughput members are the bandwidth ones in the other unit — a megabyte being
    /// [`EBS_MEGABITS_PER_MEGABYTE`] megabits — which is exactly how the published table's
    /// Mbps and MB/s columns relate.
    pub fn to_ebs_optimized_info(self) -> EbsOptimizedInfo {
        let megabytes = |mbps: i32| f64::from(mbps) / EBS_MEGABITS_PER_MEGABYTE;
        EbsOptimizedInfo {
            baseline_bandwidth_in_mbps: Some(self.baseline_bandwidth_mbps),
            baseline_throughput_in_m_bps: Some(megabytes(self.baseline_bandwidth_mbps)),
            baseline_iops: Some(self.baseline_iops),
            maximum_bandwidth_in_mbps: Some(self.maximum_bandwidth_mbps),
            maximum_throughput_in_m_bps: Some(megabytes(self.maximum_bandwidth_mbps)),
            maximum_iops: Some(self.maximum_iops),
        }
    }
}

/// The FPGA accelerators of one instance type — "the FPGA accelerator settings for the
/// instance type" (`InstanceTypeInfo$FpgaInfo`).
///
/// The devices of one instance are identical, so the whole inventory is one device
/// description and a count, and the total memory is that device's memory times the count.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PublishedFpgaDevices {
    /// How many accelerators the instance carries.
    pub count: i32,
    /// The accelerator's name, as `FpgaDeviceInfo$Name` publishes it.
    pub name: &'static str,
    /// Who makes it (`FpgaDeviceInfo$Manufacturer`).
    pub manufacturer: &'static str,
    /// The memory of ONE accelerator, in MiB.
    pub memory_mib_each: i32,
}

impl PublishedFpgaDevices {
    /// This inventory as the modelled `FpgaInfo`.
    pub fn to_fpga_info(self) -> FpgaInfo {
        FpgaInfo {
            fpgas: Some(vec![FpgaDeviceInfo {
                name: Some(self.name.to_string()),
                manufacturer: Some(self.manufacturer.to_string()),
                count: Some(self.count),
                memory_info: Some(FpgaDeviceMemoryInfo {
                    size_in_mi_b: Some(self.memory_mib_each),
                }),
            }]),
            total_fpga_memory_in_mi_b: Some(self.memory_mib_each.saturating_mul(self.count)),
        }
    }
}

/// The Inference accelerators of one instance type — "the Inference accelerators for the
/// instance type" (`InstanceTypeInfo$InferenceAcceleratorInfo`).
///
/// Like the FPGA inventory above, the devices of one instance are identical, so the whole
/// inventory is one device description and a count, and the total memory is that device's
/// memory times the count.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PublishedInferenceDevices {
    /// How many accelerators the instance carries.
    pub count: i32,
    /// The accelerator's name, as `InferenceDeviceInfo$Name` publishes it.
    pub name: &'static str,
    /// Who makes it (`InferenceDeviceInfo$Manufacturer`).
    pub manufacturer: &'static str,
    /// The memory of ONE accelerator, in MiB.
    pub memory_mib_each: i32,
}

impl PublishedInferenceDevices {
    /// This inventory as the modelled `InferenceAcceleratorInfo`.
    pub fn to_inference_accelerator_info(self) -> InferenceAcceleratorInfo {
        InferenceAcceleratorInfo {
            accelerators: Some(vec![InferenceDeviceInfo {
                count: Some(self.count),
                name: Some(self.name.to_string()),
                manufacturer: Some(self.manufacturer.to_string()),
                memory_info: Some(InferenceDeviceMemoryInfo {
                    size_in_mi_b: Some(self.memory_mib_each),
                }),
            }]),
            total_inference_memory_in_mi_b: Some(self.memory_mib_each.saturating_mul(self.count)),
        }
    }
}

/// The GPU accelerators of one instance type — "the GPU accelerator settings for the
/// instance type" (`InstanceTypeInfo$GpuInfo`).
///
/// The accelerators of one instance are identical, so the whole inventory is one device
/// description and a count, and the total memory is that device's memory times the count.
/// `partition_size` is "the size of each GPU as a fraction of a full GPU"
/// (`GpuDeviceInfo$GpuPartitionSize`), which is [`WHOLE_GPU_PARTITION`] where the instance
/// holds whole devices, and `logical_count` is "the total number of GPU devices of this
/// type" (`$LogicalGpuCount`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PublishedGpuDevices {
    /// How many accelerators the instance carries (`GpuDeviceInfo$Count`).
    pub count: i32,
    /// How many devices of this type the instance holds (`$LogicalGpuCount`).
    pub logical_count: i32,
    /// The accelerator's name, as `GpuDeviceInfo$Name` publishes it.
    pub name: &'static str,
    /// Who makes it (`$Manufacturer`).
    pub manufacturer: &'static str,
    /// The memory of ONE accelerator, in MiB.
    pub memory_mib_each: i32,
    /// The fraction of a full GPU each one is (`$GpuPartitionSize`).
    pub partition_size: f64,
    /// "A list of workload types this GPU supports" (`$Workloads`).
    pub workloads: &'static [&'static str],
}

impl PublishedGpuDevices {
    /// This inventory as the modelled `GpuInfo`.
    pub fn to_gpu_info(self) -> GpuInfo {
        GpuInfo {
            gpus: Some(vec![GpuDeviceInfo {
                name: Some(self.name.to_string()),
                manufacturer: Some(self.manufacturer.to_string()),
                count: Some(self.count),
                logical_gpu_count: Some(self.logical_count),
                gpu_partition_size: Some(self.partition_size),
                workloads: Some(
                    self.workloads
                        .iter()
                        .map(|workload| (*workload).to_string())
                        .collect(),
                ),
                memory_info: Some(GpuDeviceMemoryInfo {
                    size_in_mi_b: Some(self.memory_mib_each),
                }),
            }]),
            total_gpu_memory_in_mi_b: Some(self.memory_mib_each.saturating_mul(self.count)),
        }
    }
}

/// The published facts of ONE instance type.
///
/// Every column is optional: `None` means AWS's figure for this type is the one the
/// derivation above computes for every type, so the row says nothing about it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PublishedInstanceTypeFacts {
    /// The instance type these facts are about — a value of the modelled `InstanceType`
    /// enum.
    pub instance_type: &'static str,
    /// Is it "the latest generation instance type of an instance family" — AWS's "Previous
    /// generation instances" page answers per family, and a family that page lists is NOT.
    pub current_generation: Option<bool>,
    /// Its memory, in MiB, where the family's ratio of memory to vCPUs is its own.
    pub memory_mib: Option<i64>,
    /// The vCPUs it runs with, where AWS's figure for the type is its own rather than the one
    /// its step of the size ladder carries.
    pub default_vcpus: Option<i32>,
    /// The step the published core counts of the type go up in (`VCpuInfo$ValidCores`): a
    /// type whose cores sit on more than one processor socket takes them one per socket, so
    /// its counts are multiples of the sockets rather than every count up to its default.
    pub valid_core_step: Option<i32>,
    /// Its EBS-optimized performance.
    pub ebs_performance: Option<PublishedEbsPerformance>,
    /// How many Amazon EBS volumes it can hold attached.
    pub maximum_ebs_attachments: Option<i32>,
    /// Whether that limit is shared with its interfaces and instance store devices or is the
    /// EBS volumes' own — one value of the modelled `AttachmentLimitType`.
    pub attachment_limit_type: Option<&'static str>,
    /// Its local instance store, as (devices, GB each). A row that publishes NO devices
    /// (`Some((0, 0))`) says the type carries no instance store at all, which is a fact as
    /// much as a device count is; a row that leaves this unset keeps the derived answer.
    pub instance_store_devices: Option<(i32, i64)>,
    /// "The network performance", as the published table writes it.
    pub network_performance: Option<&'static str>,
    /// The bandwidth weightings the type can be configured with
    /// (`NetworkInfo$BandwidthWeightings`): a type whose network and EBS bandwidth can be
    /// weighted towards one or the other publishes the weightings it accepts, one value of
    /// the modelled `BandwidthWeightingType` per entry, and a type whose bandwidth is fixed
    /// publishes none at all.
    pub bandwidth_weightings: Option<&'static [&'static str]>,
    /// Whether the number of ENA queues can be changed
    /// (`NetworkInfo$FlexibleEnaQueuesSupport`), where AWS's answer for the type is its own
    /// rather than the one its hardware generation gives.
    pub flexible_ena_queues_supported: Option<bool>,
    /// The most network interfaces it can have attached.
    pub maximum_network_interfaces: Option<i32>,
    /// The most private IPv4 addresses each of those interfaces can have.
    pub ipv4_addresses_per_interface: Option<i32>,
    /// The Elastic Fabric Adapters it is offered, zero where it is offered none.
    pub efa_interfaces: Option<i32>,
    /// Who makes its processor (`ProcessorInfo$Manufacturer`).
    pub processor_manufacturer: Option<&'static str>,
    /// That processor's sustained all-core frequency, in GHz.
    pub sustained_clock_speed_ghz: Option<f64>,
    /// Is the type offered on a Dedicated Host?
    pub dedicated_hosts_supported: Option<bool>,
    /// Is Amazon CloudWatch action based recovery supported?
    pub auto_recovery_supported: Option<bool>,
    /// Whether Elastic Network Adapter is supported — one value of the modelled `EnaSupport`,
    /// which AWS publishes per instance type ("Enable enhanced networking with ENA").
    pub ena_support: Option<&'static str>,
    /// Does the type support ENA Express, the Scalable Reliable Datagram transport AWS
    /// publishes the supported types of?
    pub ena_srd_supported: Option<bool>,
    /// The boot modes the type is offered with, where AWS's list for it is its own rather
    /// than the one its architecture and hypervisor give — one value of the modelled
    /// `BootModeType` per entry.
    pub supported_boot_modes: Option<&'static [&'static str]>,
    /// Is On-Demand hibernation supported?
    pub hibernation_supported: Option<bool>,
    /// Does the type encrypt in-transit traffic between instances automatically?
    pub encryption_in_transit_supported: Option<bool>,
    /// Is reboot migration during a user-initiated reboot supported?
    pub reboot_migration_supported: Option<bool>,
    /// Is a local Precision Time Protocol hardware clock supported?
    pub phc_supported: Option<bool>,
    /// Does the type support Traffic Mirroring — whether the packets of a session can be
    /// copied under its guest?
    pub traffic_mirroring_supported: Option<bool>,
    /// Does it take interface attachments from a secondary network?
    pub secondary_network_supported: Option<bool>,
    /// Is Nitro Enclaves supported?
    pub nitro_enclaves_supported: Option<bool>,
    /// The idle timeout, in seconds, its connection tracking applies to an established TCP
    /// connection by default (`DefaultConnectionTrackingConfiguration$DefaultTcpEstablishedTimeout`),
    /// where the type's own default is not the one every earlier type reports.
    pub default_tcp_established_timeout: Option<i32>,
    /// Its FPGA accelerators.
    pub fpga_devices: Option<PublishedFpgaDevices>,
    /// Its GPU accelerators.
    pub gpu_devices: Option<PublishedGpuDevices>,
    /// Its Inference accelerators.
    pub inference_devices: Option<PublishedInferenceDevices>,
}

impl PublishedInstanceTypeFacts {
    /// A row that carries no fact yet: every column is the derivation's.
    const fn of(instance_type: &'static str) -> Self {
        Self {
            instance_type,
            current_generation: None,
            memory_mib: None,
            default_vcpus: None,
            valid_core_step: None,
            ebs_performance: None,
            maximum_ebs_attachments: None,
            attachment_limit_type: None,
            instance_store_devices: None,
            network_performance: None,
            bandwidth_weightings: None,
            flexible_ena_queues_supported: None,
            maximum_network_interfaces: None,
            ipv4_addresses_per_interface: None,
            efa_interfaces: None,
            processor_manufacturer: None,
            sustained_clock_speed_ghz: None,
            dedicated_hosts_supported: None,
            auto_recovery_supported: None,
            ena_support: None,
            ena_srd_supported: None,
            supported_boot_modes: None,
            hibernation_supported: None,
            encryption_in_transit_supported: None,
            reboot_migration_supported: None,
            phc_supported: None,
            traffic_mirroring_supported: None,
            secondary_network_supported: None,
            nitro_enclaves_supported: None,
            default_tcp_established_timeout: None,
            fpga_devices: None,
            gpu_devices: None,
            inference_devices: None,
        }
    }

    /// A family AWS's "Previous generation instances" page lists, so the type is not the
    /// latest generation of its family.
    const fn previous_generation(mut self) -> Self {
        self.current_generation = Some(false);
        self
    }

    /// The memory of the type, in MiB, where the published specification states a figure that
    /// is not a whole number of GiB (3.75 GiB is 3,840 MiB).
    const fn memory_mib(mut self, mib: i64) -> Self {
        self.memory_mib = Some(mib);
        self
    }

    /// The vCPUs the type runs with, where AWS's figure for it is its own rather than the one
    /// its step of the size ladder carries.
    const fn vcpus(mut self, vcpus: i32) -> Self {
        self.default_vcpus = Some(vcpus);
        self
    }

    /// The step the type's published core counts go up in: `sockets` for a type whose cores
    /// sit on that many processor sockets and are taken one per socket.
    const fn cores_in_steps_of(mut self, sockets: i32) -> Self {
        self.valid_core_step = Some(sockets);
        self
    }

    /// Whether Elastic Network Adapter is supported, as the published list of the types
    /// offered enhanced networking with ENA says.
    const fn ena(mut self, support: &'static str) -> Self {
        self.ena_support = Some(support);
        self
    }

    /// Whether ENA Express is supported, as the published list of the types offered it says.
    const fn ena_srd(mut self, supported: bool) -> Self {
        self.ena_srd_supported = Some(supported);
        self
    }

    /// The boot modes the type is offered with, where AWS's list for it is its own.
    const fn boot_modes(mut self, modes: &'static [&'static str]) -> Self {
        self.supported_boot_modes = Some(modes);
        self
    }

    /// The memory of the type, in GiB as the published specification writes it.
    const fn memory_gib(mut self, gib: i64) -> Self {
        self.memory_mib = Some(gib * 1024);
        self
    }

    /// The four EBS figures of the type's row: sustained bandwidth and IOPS, then the
    /// bandwidth and IOPS it can reach.
    const fn ebs(
        mut self,
        baseline_bandwidth_mbps: i32,
        baseline_iops: i32,
        maximum_bandwidth_mbps: i32,
        maximum_iops: i32,
    ) -> Self {
        self.ebs_performance = Some(PublishedEbsPerformance {
            baseline_bandwidth_mbps,
            baseline_iops,
            maximum_bandwidth_mbps,
            maximum_iops,
        });
        self
    }

    /// An EBS volume limit of the type's own, rather than one shared with its interfaces and
    /// instance store devices.
    const fn dedicated_ebs_attachments(mut self, limit: i32) -> Self {
        self.maximum_ebs_attachments = Some(limit);
        self.attachment_limit_type = Some(DEDICATED_ATTACHMENT_LIMIT);
        self
    }

    /// An EBS volume limit above the attachments a shared pool leaves, the type's EBS
    /// devices being attached beside that pool rather than out of it — the limit stays the
    /// `shared` kind the type reports.
    const fn ebs_attachments(mut self, limit: i32) -> Self {
        self.maximum_ebs_attachments = Some(limit);
        self
    }

    /// An EBS volume limit shared with the type's interfaces and instance store devices,
    /// where the shared total is the type's own rather than the Nitro pool.
    const fn shared_ebs_attachments(mut self, limit: i32) -> Self {
        self.maximum_ebs_attachments = Some(limit);
        self.attachment_limit_type = Some(SHARED_ATTACHMENT_LIMIT);
        self
    }

    /// The type carries NO local instance store, where the families its name reads as having
    /// one do.
    const fn no_instance_store(mut self) -> Self {
        self.instance_store_devices = Some((0, 0));
        self
    }

    /// The type's local instance store: `devices` devices of `gb_each` GB.
    const fn instance_store(mut self, devices: i32, gb_each: i64) -> Self {
        self.instance_store_devices = Some((devices, gb_each));
        self
    }

    /// The type's network performance, as the published table writes it.
    const fn network(mut self, performance: &'static str) -> Self {
        self.network_performance = Some(performance);
        self
    }

    /// The bandwidth weightings the type publishes, as the modelled enum spells them.
    const fn bandwidth_weightings(mut self, weightings: &'static [&'static str]) -> Self {
        self.bandwidth_weightings = Some(weightings);
        self
    }

    /// Whether the type's adapter takes a configurable number of ENA queues, as the published
    /// list of the types offered that says.
    const fn flexible_ena_queues(mut self, supported: bool) -> Self {
        self.flexible_ena_queues_supported = Some(supported);
        self
    }

    /// The type's EBS volumes have a limit of their OWN — they are attached to EBS hardware
    /// of the type's own rather than out of a pool shared with its interfaces and its instance
    /// store devices — where the published figure itself is not recorded yet, which leaves the
    /// limit the one the derivation above counts.
    const fn dedicated_attachment_limit(mut self) -> Self {
        self.attachment_limit_type = Some(DEDICATED_ATTACHMENT_LIMIT);
        self
    }

    /// The most network interfaces the type can have attached, where AWS's figure for it is
    /// its own rather than the one its step of the size ladder publishes.
    const fn interfaces(mut self, interfaces: i32) -> Self {
        self.maximum_network_interfaces = Some(interfaces);
        self
    }

    /// The most private IPv4 addresses each of those interfaces can have, on the same terms.
    const fn ipv4_per_interface(mut self, addresses: i32) -> Self {
        self.ipv4_addresses_per_interface = Some(addresses);
        self
    }

    /// The Elastic Fabric Adapters the type is offered, zero where it is offered none.
    const fn efa(mut self, interfaces: i32) -> Self {
        self.efa_interfaces = Some(interfaces);
        self
    }

    /// The type's processor and that processor's sustained frequency.
    const fn processor(mut self, manufacturer: &'static str, sustained_ghz: f64) -> Self {
        self.processor_manufacturer = Some(manufacturer);
        self.sustained_clock_speed_ghz = Some(sustained_ghz);
        self
    }

    /// Whether the type is offered on a Dedicated Host.
    const fn dedicated_hosts(mut self, supported: bool) -> Self {
        self.dedicated_hosts_supported = Some(supported);
        self
    }

    /// Whether CloudWatch action based recovery is supported.
    const fn auto_recovery(mut self, supported: bool) -> Self {
        self.auto_recovery_supported = Some(supported);
        self
    }

    /// Whether On-Demand hibernation is supported.
    const fn hibernation(mut self, supported: bool) -> Self {
        self.hibernation_supported = Some(supported);
        self
    }

    /// Whether the type encrypts in-transit traffic between instances.
    const fn encryption_in_transit(mut self, supported: bool) -> Self {
        self.encryption_in_transit_supported = Some(supported);
        self
    }

    /// Whether reboot migration is supported, and whether a local PTP hardware clock is.
    const fn host_features(mut self, reboot_migration: bool, phc: bool) -> Self {
        self.reboot_migration_supported = Some(reboot_migration);
        self.phc_supported = Some(phc);
        self
    }

    /// Whether Nitro Enclaves is supported, where the published answer for the type is not
    /// the one its hypervisor and size give.
    const fn nitro_enclaves(mut self, supported: bool) -> Self {
        self.nitro_enclaves_supported = Some(supported);
        self
    }

    /// The idle timeout, in seconds, the type's connection tracking applies to an established
    /// TCP connection by default.
    const fn tcp_established_timeout(mut self, seconds: i32) -> Self {
        self.default_tcp_established_timeout = Some(seconds);
        self
    }

    /// Whether interface attachments from a secondary network are supported, where the
    /// published answer for the type is not the one its hardware generation gives.
    const fn secondary_network(mut self, supported: bool) -> Self {
        self.secondary_network_supported = Some(supported);
        self
    }

    /// Whether Traffic Mirroring is supported, where the published answer for the type is not
    /// the one its hypervisor gives.
    const fn traffic_mirroring(mut self, supported: bool) -> Self {
        self.traffic_mirroring_supported = Some(supported);
        self
    }

    /// The type's GPU accelerators: `count` whole devices of `memory_mib_each` MiB, offered for
    /// the `workloads` the published inventory names.
    const fn gpus(
        mut self,
        count: i32,
        name: &'static str,
        manufacturer: &'static str,
        memory_mib_each: i32,
        workloads: &'static [&'static str],
    ) -> Self {
        self.gpu_devices = Some(PublishedGpuDevices {
            count,
            logical_count: count,
            name,
            manufacturer,
            memory_mib_each,
            partition_size: WHOLE_GPU_PARTITION,
            workloads,
        });
        self
    }

    /// The type's FPGA accelerators.
    const fn fpgas(
        mut self,
        count: i32,
        name: &'static str,
        manufacturer: &'static str,
        memory_mib_each: i32,
    ) -> Self {
        self.fpga_devices = Some(PublishedFpgaDevices {
            count,
            name,
            manufacturer,
            memory_mib_each,
        });
        self
    }

    /// The type's Inference accelerators.
    const fn inference(
        mut self,
        count: i32,
        name: &'static str,
        manufacturer: &'static str,
        memory_mib_each: i32,
    ) -> Self {
        self.inference_devices = Some(PublishedInferenceDevices {
            count,
            name,
            manufacturer,
            memory_mib_each,
        });
        self
    }
}

/// The `AttachmentLimitType` of a type whose EBS volumes have a limit of their own.
const DEDICATED_ATTACHMENT_LIMIT: &str = "dedicated";

/// The `AttachmentLimitType` of a type whose EBS volumes share their limit with its network
/// interfaces and its instance store devices.
const SHARED_ATTACHMENT_LIMIT: &str = "shared";

/// The `EnaSupport` of a type that predates the Elastic Network Adapter.
const ENA_UNSUPPORTED: &str = "unsupported";

/// The `GpuDeviceInfo$GpuPartitionSize` of an instance that holds WHOLE GPUs: each one is a
/// full device, which is the whole of the fraction that member reports.
const WHOLE_GPU_PARTITION: f64 = 1.0;

/// The `GpuDeviceInfo$Workloads` entry of an accelerator built for machine learning, which is
/// what the published inventory of the accelerated families names.
const ML_AI_WORKLOAD: &[&str] = &["ml-ai"];

/// The established-TCP idle timeout, in seconds, the newest instance types' connection
/// tracking applies by default: a figure of the type's own hardware, well below the one the
/// earlier types report ([`CONNECTION_TRACKING_TIMEOUTS`]).
const SHORT_TCP_ESTABLISHED_TIMEOUT: i32 = 350;

/// The `BandwidthWeightingType` values a type whose network and EBS bandwidth can be weighted
/// towards one or the other publishes: the default split, and one weighting for each side the
/// extra bandwidth can be given to.
const CONFIGURABLE_BANDWIDTH_WEIGHTINGS: &[&str] = &["default", "ebs-1", "vpc-1"];

/// The published hardware facts of the instance types recorded so far, one row per type.
///
/// Read through [`published_instance_type_facts`]. The rows are grouped by family, and each
/// family's comment says where its figures come from: the family's own specification page
/// (vCPUs, memory, instance store devices, network performance, accelerators), the
/// "Amazon EBS optimized instance types" table (the four EBS figures), "Instance volume
/// limits" (the EBS attachment limit and whether it is dedicated), and the "IP addresses per
/// network interface per instance type" table (the interface limits).
static PUBLISHED_INSTANCE_TYPE_FACTS: &[PublishedInstanceTypeFacts] = &[
    // C4 — compute optimized, 2nd generation Intel Xeon E5-2666 v3 (Haswell) at a sustained
    // 2.9 GHz, EBS-optimized by default with a DEDICATED figure per size (a pre-Nitro type
    // does not burst above it, so each size publishes one bandwidth and one IOPS figure), and
    // no local instance store. The family is on AWS's "Previous generation instances" page, so
    // it is not the latest generation of its family, and it predates the Elastic Network
    // Adapter. Its EBS volumes share one attachment total with its other devices — 40, the
    // figure "Instance volume limits" publishes for the type — rather than having a limit of
    // their own. Dedicated Hosts ARE offered on the family, and reboot migration is, while a
    // PTP hardware clock is not. Memory is the family's published 3.75 GiB per two vCPUs,
    // which is not a whole number of GiB at the small sizes; the largest size carries 36
    // vCPUs on two sockets rather than the 32 its step of the ladder would give, and its
    // cores are configured one per socket.
    PublishedInstanceTypeFacts::of("c4.large")
        .previous_generation()
        .memory_mib(3_840)
        .ebs(500, 4_000, 500, 4_000)
        .shared_ebs_attachments(40)
        .network("Moderate")
        .processor("Intel", 2.9)
        .ena(ENA_UNSUPPORTED)
        .dedicated_hosts(true)
        .host_features(true, false),
    PublishedInstanceTypeFacts::of("c4.xlarge")
        .previous_generation()
        .memory_mib(7_680)
        .ebs(750, 6_000, 750, 6_000)
        .shared_ebs_attachments(40)
        .network("High")
        .processor("Intel", 2.9)
        .ena(ENA_UNSUPPORTED)
        .dedicated_hosts(true)
        .host_features(true, false),
    PublishedInstanceTypeFacts::of("c4.2xlarge")
        .previous_generation()
        .memory_mib(15_360)
        .ebs(1_000, 8_000, 1_000, 8_000)
        .shared_ebs_attachments(40)
        .network("High")
        .processor("Intel", 2.9)
        .ena(ENA_UNSUPPORTED)
        .dedicated_hosts(true)
        .host_features(true, false),
    PublishedInstanceTypeFacts::of("c4.4xlarge")
        .previous_generation()
        .memory_mib(30_720)
        .ebs(2_000, 16_000, 2_000, 16_000)
        .shared_ebs_attachments(40)
        .network("High")
        .processor("Intel", 2.9)
        .ena(ENA_UNSUPPORTED)
        .dedicated_hosts(true)
        .host_features(true, false),
    PublishedInstanceTypeFacts::of("c4.8xlarge")
        .previous_generation()
        .memory_mib(61_440)
        .vcpus(36)
        .cores_in_steps_of(2)
        .ebs(4_000, 32_000, 4_000, 32_000)
        .shared_ebs_attachments(40)
        .network("10 Gigabit")
        .processor("Intel", 2.9)
        .ena(ENA_UNSUPPORTED)
        .dedicated_hosts(true)
        .host_features(true, false),
    // C5 — compute optimized on the AWS Nitro System, Intel Xeon Platinum: the 8124M
    // (Skylake) at a sustained 3.4 GHz on most of the ladder, and the 8275CL (Cascade Lake) at
    // a sustained 3.6 GHz on the sizes built on it (`12xlarge`, `24xlarge` and the bare metal
    // size). The family is NOT the latest generation of its family, and no size carries a
    // local instance store. Its EBS figures are the family's own rows of "Amazon EBS optimized
    // instance types": the three smallest sizes publish a baseline they burst above, up to
    // 4,750 Mbps and 20,000 IOPS, and the family's bandwidth is dedicated from the `4xlarge`
    // up, two sizes sharing each of the figures above that one. The published core counts go
    // up in twos from the `xlarge`, the bare metal size publishing every count. Dedicated
    // Hosts are offered on the family EXCEPT on the two sizes that hold the whole host, and
    // the bare metal size is offered the legacy BIOS boot mode alone, supports CloudWatch
    // action based recovery — which the sizes with no instance store already do — and supports
    // reboot migration, the move to another host being the HOST's to make and not the
    // hypervisor's that a bare metal instance has none of.
    PublishedInstanceTypeFacts::of("c5.large")
        .previous_generation()
        .ebs(650, 4_000, 4_750, 20_000)
        .network("Up to 10 Gigabit")
        .processor("Intel", 3.4)
        .dedicated_hosts(true),
    PublishedInstanceTypeFacts::of("c5.xlarge")
        .previous_generation()
        .cores_in_steps_of(2)
        .ebs(1_150, 6_000, 4_750, 20_000)
        .network("Up to 10 Gigabit")
        .processor("Intel", 3.4)
        .dedicated_hosts(true),
    PublishedInstanceTypeFacts::of("c5.2xlarge")
        .previous_generation()
        .cores_in_steps_of(2)
        .ebs(2_300, 10_000, 4_750, 20_000)
        .network("Up to 10 Gigabit")
        .processor("Intel", 3.4)
        .dedicated_hosts(true),
    PublishedInstanceTypeFacts::of("c5.4xlarge")
        .previous_generation()
        .cores_in_steps_of(2)
        .ebs(4_750, 20_000, 4_750, 20_000)
        .network("Up to 10 Gigabit")
        .processor("Intel", 3.4)
        .dedicated_hosts(true),
    PublishedInstanceTypeFacts::of("c5.9xlarge")
        .previous_generation()
        .cores_in_steps_of(2)
        .ebs(9_500, 40_000, 9_500, 40_000)
        // The two sizes carved out of the same half of the host report the same figure, which
        // is the one the API publishes for them rather than the round number of the family's
        // specification page.
        .network("12 Gigabit")
        .processor("Intel", 3.4)
        .dedicated_hosts(true),
    PublishedInstanceTypeFacts::of("c5.12xlarge")
        .previous_generation()
        .cores_in_steps_of(2)
        .ebs(9_500, 40_000, 9_500, 40_000)
        .network("12 Gigabit")
        .processor("Intel", 3.6)
        .dedicated_hosts(true),
    PublishedInstanceTypeFacts::of("c5.18xlarge")
        .previous_generation()
        .cores_in_steps_of(2)
        .ebs(19_000, 80_000, 19_000, 80_000)
        .network("25 Gigabit")
        .processor("Intel", 3.4)
        .dedicated_hosts(true),
    PublishedInstanceTypeFacts::of("c5.24xlarge")
        .previous_generation()
        .cores_in_steps_of(2)
        .ebs(19_000, 80_000, 19_000, 80_000)
        .network("25 Gigabit")
        .processor("Intel", 3.6)
        .dedicated_hosts(false),
    PublishedInstanceTypeFacts::of("c5.metal")
        .previous_generation()
        .ebs(19_000, 80_000, 19_000, 80_000)
        .network("25 Gigabit")
        .processor("Intel", 3.6)
        .dedicated_hosts(false)
        .auto_recovery(true)
        .host_features(true, false)
        .boot_modes(&["legacy-bios"]),
    // C5a — compute optimized on the AMD EPYC 7R32 at a sustained 3.3 GHz, with no local
    // instance store. The family's EBS bandwidth doubles up the ladder and is DEDICATED from
    // the 8xlarge up, the four sizes below that one bursting to the 8xlarge's figure. Its EBS
    // volumes come out of the shared Nitro attachment pool, which the derivation above already
    // counts, so these rows leave that limit unset. The family is offered on NO Dedicated Host
    // and supports no On-Demand hibernation. Two cells are the family's own RATIO rather than a
    // figure read off the published table — the 4xlarge's IOPS, which is four times its
    // bandwidth as the three smaller sizes' published pairs are, and the core counts going up
    // in twos from the 4xlarge, where the published list is shorter than every count up to the
    // default — so a published figure for either replaces it.
    PublishedInstanceTypeFacts::of("c5a.large")
        .ebs(200, 800, 3_170, 13_300)
        .network("Up to 10 Gigabit")
        .processor("AMD", 3.3)
        .dedicated_hosts(false)
        .hibernation(false),
    PublishedInstanceTypeFacts::of("c5a.xlarge")
        .ebs(400, 1_600, 3_170, 13_300)
        .network("Up to 10 Gigabit")
        .processor("AMD", 3.3)
        .dedicated_hosts(false)
        .hibernation(false),
    PublishedInstanceTypeFacts::of("c5a.2xlarge")
        .ebs(800, 3_200, 3_170, 13_300)
        .network("Up to 10 Gigabit")
        .processor("AMD", 3.3)
        .dedicated_hosts(false)
        .hibernation(false),
    PublishedInstanceTypeFacts::of("c5a.4xlarge")
        .cores_in_steps_of(2)
        .ebs(1_580, 6_320, 3_170, 13_300)
        .network("Up to 10 Gigabit")
        .processor("AMD", 3.3)
        .dedicated_hosts(false)
        .hibernation(false),
    PublishedInstanceTypeFacts::of("c5a.8xlarge")
        .cores_in_steps_of(2)
        .ebs(3_170, 13_300, 3_170, 13_300)
        .network("10 Gigabit")
        .processor("AMD", 3.3)
        .dedicated_hosts(false)
        .hibernation(false),
    PublishedInstanceTypeFacts::of("c5a.12xlarge")
        .cores_in_steps_of(2)
        .ebs(4_750, 20_000, 4_750, 20_000)
        .network("12 Gigabit")
        .processor("AMD", 3.3)
        .dedicated_hosts(false)
        .hibernation(false),
    PublishedInstanceTypeFacts::of("c5a.16xlarge")
        .cores_in_steps_of(2)
        .ebs(6_300, 26_700, 6_300, 26_700)
        .network("20 Gigabit")
        .processor("AMD", 3.3)
        .dedicated_hosts(false)
        .hibernation(false),
    PublishedInstanceTypeFacts::of("c5a.24xlarge")
        .cores_in_steps_of(2)
        .ebs(9_500, 40_000, 9_500, 40_000)
        .network("20 Gigabit")
        .processor("AMD", 3.3)
        .dedicated_hosts(false)
        .hibernation(false),
    // C9gd — compute optimized with local NVMe on the newest AWS Graviton hardware. The
    // family's network and EBS bandwidth is CONFIGURABLE: it publishes the bandwidth
    // weightings a launch may ask for, which the families whose split is fixed publish none
    // of, and its EBS volumes are attached to EBS hardware of the type's own rather than out
    // of the pool shared with its interfaces and instance store devices, so the limit it
    // reports is of the `dedicated` kind at every size. The Elastic Fabric Adapter is offered
    // on the two sizes that hold the whole host. Its bare metal size's published hardware is
    // recorded below; the figures of the other sizes are not recorded yet, so each of those
    // rows carries only what IS published for it and the derivation answers the rest.
    PublishedInstanceTypeFacts::of("c9gd.medium")
        .dedicated_attachment_limit()
        .bandwidth_weightings(CONFIGURABLE_BANDWIDTH_WEIGHTINGS),
    PublishedInstanceTypeFacts::of("c9gd.large")
        .dedicated_attachment_limit()
        .bandwidth_weightings(CONFIGURABLE_BANDWIDTH_WEIGHTINGS),
    PublishedInstanceTypeFacts::of("c9gd.xlarge")
        .dedicated_attachment_limit()
        .bandwidth_weightings(CONFIGURABLE_BANDWIDTH_WEIGHTINGS),
    PublishedInstanceTypeFacts::of("c9gd.2xlarge")
        .dedicated_attachment_limit()
        .bandwidth_weightings(CONFIGURABLE_BANDWIDTH_WEIGHTINGS),
    PublishedInstanceTypeFacts::of("c9gd.4xlarge")
        .dedicated_attachment_limit()
        .bandwidth_weightings(CONFIGURABLE_BANDWIDTH_WEIGHTINGS),
    PublishedInstanceTypeFacts::of("c9gd.8xlarge")
        .dedicated_attachment_limit()
        .bandwidth_weightings(CONFIGURABLE_BANDWIDTH_WEIGHTINGS),
    PublishedInstanceTypeFacts::of("c9gd.12xlarge")
        .dedicated_attachment_limit()
        .bandwidth_weightings(CONFIGURABLE_BANDWIDTH_WEIGHTINGS),
    PublishedInstanceTypeFacts::of("c9gd.16xlarge")
        .dedicated_attachment_limit()
        .bandwidth_weightings(CONFIGURABLE_BANDWIDTH_WEIGHTINGS),
    PublishedInstanceTypeFacts::of("c9gd.24xlarge")
        .dedicated_attachment_limit()
        .bandwidth_weightings(CONFIGURABLE_BANDWIDTH_WEIGHTINGS),
    PublishedInstanceTypeFacts::of("c9gd.48xlarge")
        .dedicated_attachment_limit()
        .bandwidth_weightings(CONFIGURABLE_BANDWIDTH_WEIGHTINGS)
        .efa(1),
    PublishedInstanceTypeFacts::of("c9gd.metal-48xl")
        .ebs(72_000, 288_000, 72_000, 288_000)
        .dedicated_ebs_attachments(79)
        .instance_store(3, 3_800)
        .bandwidth_weightings(CONFIGURABLE_BANDWIDTH_WEIGHTINGS)
        .efa(1),
    // D3en — dense HDD storage, 2nd generation Intel Xeon Scalable (Cascade Lake) at a
    // sustained 3.1 GHz, 14 TB magnetic devices (13,980 GB as the catalog counts them), and
    // the enhanced network the family's `n` names, up to 75 Gigabit on its largest size. The
    // family's EBS bandwidth is dedicated from `4xlarge` up, and the two sizes below that one
    // burst to its figure. The devices take most of the shared attachment pool, which is why
    // the EBS volume limit falls with the size: the derivation above already counts the pool,
    // so these rows leave it unset. Every size of the family carries FOUR network interfaces,
    // rather than the count its step of the size ladder publishes.
    // Dedicated Hosts are not offered on the family, and neither reboot migration nor a PTP
    // hardware clock is.
    PublishedInstanceTypeFacts::of("d3en.xlarge")
        .ebs(850, 5_000, 2_800, 15_000)
        .instance_store(2, 13_980)
        .network("Up to 25 Gigabit")
        .interfaces(4)
        .processor("Intel", 3.1)
        .dedicated_hosts(false)
        .host_features(false, false),
    PublishedInstanceTypeFacts::of("d3en.2xlarge")
        .ebs(1_700, 10_000, 2_800, 15_000)
        .instance_store(4, 13_980)
        .network("Up to 25 Gigabit")
        .interfaces(4)
        .processor("Intel", 3.1)
        .dedicated_hosts(false)
        .host_features(false, false),
    PublishedInstanceTypeFacts::of("d3en.4xlarge")
        .ebs(2_800, 15_000, 2_800, 15_000)
        .instance_store(8, 13_980)
        .network("25 Gigabit")
        .interfaces(4)
        .processor("Intel", 3.1)
        .dedicated_hosts(false)
        .host_features(false, false),
    PublishedInstanceTypeFacts::of("d3en.6xlarge")
        .ebs(4_000, 25_000, 4_000, 25_000)
        .instance_store(12, 13_980)
        .network("40 Gigabit")
        .interfaces(4)
        .processor("Intel", 3.1)
        .dedicated_hosts(false)
        .host_features(false, false),
    PublishedInstanceTypeFacts::of("d3en.8xlarge")
        .ebs(5_000, 30_000, 5_000, 30_000)
        .instance_store(16, 13_980)
        .network("50 Gigabit")
        .interfaces(4)
        .processor("Intel", 3.1)
        .dedicated_hosts(false)
        .host_features(false, false),
    PublishedInstanceTypeFacts::of("d3en.12xlarge")
        .ebs(7_000, 40_000, 7_000, 40_000)
        .instance_store(24, 13_980)
        .network("75 Gigabit")
        .interfaces(4)
        .processor("Intel", 3.1)
        .dedicated_hosts(false)
        .host_features(false, false),
    // Dl2q — inference accelerated, 3rd generation Intel Xeon Scalable (Ice Lake), carrying
    // eight Qualcomm AI100 accelerators of 16 GB each and NO local instance store, with the
    // 100 Gigabit network and the in-transit encryption of that class of adapter. Its EBS
    // volumes are limited to 19 attachments, below what the shared Nitro pool would leave, and
    // its 48 cores sit on two sockets, so they are configured one per socket.
    PublishedInstanceTypeFacts::of("dl2q.24xlarge")
        .memory_gib(768)
        .cores_in_steps_of(2)
        .ebs(19_000, 80_000, 19_000, 80_000)
        .ebs_attachments(19)
        .no_instance_store()
        .network("100 Gigabit")
        .processor("Intel", 3.5)
        .encryption_in_transit(true)
        .inference(8, "Qualcomm AI100", "Qualcomm", 16_000),
    // F2 — FPGA accelerated, 3rd generation AMD EPYC (Milan) at a sustained 3.6 GHz, with
    // one Xilinx Virtex UltraScale+ (VU47P) accelerator of 80 GiB and one 1,900 GB local NVMe
    // device per two sizes of the ladder, and an EBS volume limit of its own rather than one
    // out of the shared pool.
    PublishedInstanceTypeFacts::of("f2.6xlarge")
        .memory_gib(256)
        .ebs(7_500, 30_000, 15_000, 60_000)
        .dedicated_ebs_attachments(32)
        .instance_store(1, 1_900)
        .network("Up to 12.5 Gigabit")
        .interfaces(8)
        .ipv4_per_interface(30)
        .efa(0)
        .processor("AMD", 3.6)
        .encryption_in_transit(true)
        .host_features(false, false)
        .fpgas(1, "Virtex UltraScale+ (VU47P)", "Xilinx", 81_920),
    PublishedInstanceTypeFacts::of("f2.12xlarge")
        .memory_gib(512)
        .ebs(15_000, 60_000, 15_000, 60_000)
        .dedicated_ebs_attachments(32)
        .instance_store(2, 1_900)
        .network("25 Gigabit")
        .interfaces(8)
        .ipv4_per_interface(30)
        .efa(0)
        .processor("AMD", 3.6)
        .encryption_in_transit(true)
        .host_features(false, false)
        .fpgas(2, "Virtex UltraScale+ (VU47P)", "Xilinx", 81_920),
    PublishedInstanceTypeFacts::of("f2.48xlarge")
        .memory_gib(2_048)
        .ebs(60_000, 240_000, 60_000, 240_000)
        .dedicated_ebs_attachments(32)
        .instance_store(8, 1_900)
        .network("100 Gigabit")
        .interfaces(15)
        .ipv4_per_interface(50)
        .efa(1)
        .processor("AMD", 3.6)
        .encryption_in_transit(true)
        .host_features(false, false)
        .fpgas(8, "Virtex UltraScale+ (VU47P)", "Xilinx", 81_920),
    // M6i — general purpose, 3rd generation Intel Xeon Scalable (Ice Lake) at a sustained
    // 3.5 GHz, with NO local instance store and up to 50 Gigabit of network on the sizes that
    // hold the whole host. The family is not the latest generation of its family. Its EBS
    // figures are its own rows of "Amazon EBS optimized instance types": the sizes below the
    // `8xlarge` publish a baseline they burst above, up to 10,000 Mbps and 40,000 IOPS, and
    // the bandwidth is dedicated from that size up. The adapter takes a configurable number of
    // ENA queues, while ENA Express is offered only from the size that carries the family's
    // whole dedicated network share, and the Elastic Fabric Adapter only on the two sizes that
    // hold the whole host. The published core counts go up in twos from the `2xlarge`, the
    // bare metal size publishing every count, and that bare metal size is offered the legacy
    // BIOS boot mode alone and supports CloudWatch action based recovery.
    PublishedInstanceTypeFacts::of("m6i.large")
        .previous_generation()
        .ebs(625, 3_600, 10_000, 40_000)
        .network("Up to 12.5 Gigabit")
        .flexible_ena_queues(true)
        .efa(0)
        .processor("Intel", 3.5)
        .ena_srd(false)
        .dedicated_hosts(true),
    PublishedInstanceTypeFacts::of("m6i.xlarge")
        .previous_generation()
        .ebs(1_250, 6_000, 10_000, 40_000)
        .network("Up to 12.5 Gigabit")
        .flexible_ena_queues(true)
        .efa(0)
        .processor("Intel", 3.5)
        .ena_srd(false)
        .dedicated_hosts(true),
    PublishedInstanceTypeFacts::of("m6i.2xlarge")
        .previous_generation()
        .cores_in_steps_of(2)
        .ebs(2_500, 12_000, 10_000, 40_000)
        .network("Up to 12.5 Gigabit")
        .flexible_ena_queues(true)
        .efa(0)
        .processor("Intel", 3.5)
        .ena_srd(false)
        .dedicated_hosts(true),
    PublishedInstanceTypeFacts::of("m6i.4xlarge")
        .previous_generation()
        .cores_in_steps_of(2)
        .ebs(5_000, 20_000, 10_000, 40_000)
        .network("Up to 12.5 Gigabit")
        .flexible_ena_queues(true)
        .efa(0)
        .processor("Intel", 3.5)
        .ena_srd(false)
        .dedicated_hosts(true),
    PublishedInstanceTypeFacts::of("m6i.8xlarge")
        .previous_generation()
        .cores_in_steps_of(2)
        .ebs(10_000, 40_000, 10_000, 40_000)
        .network("12.5 Gigabit")
        .flexible_ena_queues(true)
        .efa(0)
        .processor("Intel", 3.5)
        .ena_srd(true)
        .dedicated_hosts(true),
    PublishedInstanceTypeFacts::of("m6i.12xlarge")
        .previous_generation()
        .cores_in_steps_of(2)
        .ebs(15_000, 60_000, 15_000, 60_000)
        .network("18.75 Gigabit")
        .flexible_ena_queues(true)
        .efa(0)
        .processor("Intel", 3.5)
        .ena_srd(true)
        .dedicated_hosts(true),
    PublishedInstanceTypeFacts::of("m6i.16xlarge")
        .previous_generation()
        .cores_in_steps_of(2)
        .ebs(20_000, 80_000, 20_000, 80_000)
        .network("25 Gigabit")
        .flexible_ena_queues(true)
        .efa(0)
        .processor("Intel", 3.5)
        .ena_srd(true)
        .dedicated_hosts(true),
    PublishedInstanceTypeFacts::of("m6i.24xlarge")
        .previous_generation()
        .cores_in_steps_of(2)
        .ebs(30_000, 120_000, 30_000, 120_000)
        .network("37.5 Gigabit")
        .flexible_ena_queues(true)
        .efa(0)
        .processor("Intel", 3.5)
        .ena_srd(true)
        .dedicated_hosts(true),
    PublishedInstanceTypeFacts::of("m6i.32xlarge")
        .previous_generation()
        .cores_in_steps_of(2)
        .ebs(40_000, 160_000, 40_000, 160_000)
        .network("50 Gigabit")
        .flexible_ena_queues(true)
        .efa(1)
        .processor("Intel", 3.5)
        .ena_srd(true)
        .dedicated_hosts(true),
    PublishedInstanceTypeFacts::of("m6i.metal")
        .previous_generation()
        .ebs(40_000, 160_000, 40_000, 160_000)
        .network("50 Gigabit")
        .flexible_ena_queues(true)
        .efa(1)
        .processor("Intel", 3.5)
        .ena_srd(true)
        .dedicated_hosts(true)
        .auto_recovery(true)
        .boot_modes(&["legacy-bios"]),
    // M6idn — general purpose with local NVMe and the enhanced network the family's `n`
    // names, 3rd generation Intel Xeon Scalable (Ice Lake) at a sustained 3.5 GHz. The
    // family carries up to 200 Gigabit of network and 100 Gbps of EBS bandwidth on its
    // largest size, each size taking its share of both, and the Elastic Fabric Adapter is
    // offered only on that largest size and the bare metal one.
    PublishedInstanceTypeFacts::of("m6idn.large")
        .ebs(1_562, 6_250, 25_000, 100_000)
        .instance_store(1, 118)
        .network("Up to 25 Gigabit")
        .interfaces(3)
        .ipv4_per_interface(10)
        .efa(0)
        .host_features(false, false),
    PublishedInstanceTypeFacts::of("m6idn.xlarge")
        .ebs(3_125, 12_500, 25_000, 100_000)
        .instance_store(1, 237)
        .network("Up to 30 Gigabit")
        .interfaces(4)
        .ipv4_per_interface(15)
        .efa(0)
        .host_features(false, false),
    PublishedInstanceTypeFacts::of("m6idn.2xlarge")
        .ebs(6_250, 25_000, 25_000, 100_000)
        .instance_store(1, 474)
        .network("Up to 40 Gigabit")
        .interfaces(4)
        .ipv4_per_interface(15)
        .efa(0)
        .host_features(false, false),
    PublishedInstanceTypeFacts::of("m6idn.4xlarge")
        .ebs(12_500, 50_000, 25_000, 100_000)
        .instance_store(1, 950)
        .network("Up to 50 Gigabit")
        .interfaces(8)
        .ipv4_per_interface(30)
        .efa(0)
        .host_features(false, false),
    PublishedInstanceTypeFacts::of("m6idn.8xlarge")
        .ebs(25_000, 100_000, 25_000, 100_000)
        .instance_store(1, 1_900)
        .network("50 Gigabit")
        .interfaces(8)
        .ipv4_per_interface(30)
        .efa(0)
        .host_features(false, false),
    PublishedInstanceTypeFacts::of("m6idn.12xlarge")
        .ebs(37_500, 150_000, 37_500, 150_000)
        .instance_store(2, 1_425)
        .network("75 Gigabit")
        .interfaces(8)
        .ipv4_per_interface(30)
        .efa(0)
        .host_features(false, false),
    PublishedInstanceTypeFacts::of("m6idn.16xlarge")
        .ebs(50_000, 200_000, 50_000, 200_000)
        .instance_store(2, 1_900)
        .network("100 Gigabit")
        .interfaces(15)
        .ipv4_per_interface(50)
        .efa(0)
        .host_features(false, false),
    PublishedInstanceTypeFacts::of("m6idn.24xlarge")
        .ebs(75_000, 300_000, 75_000, 300_000)
        .instance_store(4, 1_425)
        .network("150 Gigabit")
        .interfaces(15)
        .ipv4_per_interface(50)
        .efa(0)
        .host_features(false, false),
    PublishedInstanceTypeFacts::of("m6idn.32xlarge")
        .ebs(100_000, 400_000, 100_000, 400_000)
        .instance_store(4, 1_900)
        .network("200 Gigabit")
        .interfaces(15)
        .ipv4_per_interface(50)
        .efa(1)
        .host_features(false, false),
    PublishedInstanceTypeFacts::of("m6idn.metal")
        .ebs(100_000, 400_000, 100_000, 400_000)
        .instance_store(4, 1_900)
        .network("200 Gigabit")
        .interfaces(15)
        .ipv4_per_interface(50)
        .efa(1)
        .host_features(false, false),
    // P6-B200 — accelerated compute carrying EIGHT NVIDIA B200 accelerators of 183,359 MiB
    // each, 2 TiB of system memory, eight 3,800 GB local NVMe devices and the family's 100 Gbps
    // of EBS bandwidth on EBS hardware of its own (so its 64 volume attachments are the EBS
    // devices' own limit rather than a share of the Nitro pool). Its Elastic Fabric Adapters
    // are one per network card, its connection tracking applies the shorter established-TCP
    // idle timeout the newest hardware publishes, Nitro Enclaves is not offered on it, and its
    // cores are configured two at a time.
    PublishedInstanceTypeFacts::of("p6-b200.48xlarge")
        .memory_gib(2_048)
        .cores_in_steps_of(2)
        .ebs(100_000, 400_000, 100_000, 400_000)
        .dedicated_ebs_attachments(64)
        .instance_store(8, 3_800)
        .efa(8)
        .tcp_established_timeout(SHORT_TCP_ESTABLISHED_TIMEOUT)
        .nitro_enclaves(false)
        .gpus(8, "B200", "NVIDIA", 183_359, ML_AI_WORKLOAD),
    // P6-B300 — the same shape of accelerated instance as P6-B200 above, with EIGHT NVIDIA
    // B300 accelerators of 275,040 MiB each and 4 TiB of system memory.
    PublishedInstanceTypeFacts::of("p6-b300.48xlarge")
        .memory_gib(4_096)
        .cores_in_steps_of(2)
        .ebs(100_000, 400_000, 100_000, 400_000)
        .dedicated_ebs_attachments(64)
        .instance_store(8, 3_800)
        .efa(8)
        .tcp_established_timeout(SHORT_TCP_ESTABLISHED_TIMEOUT)
        .nitro_enclaves(false)
        .gpus(8, "B300", "NVIDIA", 275_040, ML_AI_WORKLOAD),
    // R5b — memory optimized with the family's EBS-optimized hardware, 2nd generation Intel
    // Xeon Scalable (Cascade Lake) at a sustained 3.1 GHz: up to 60 Gbps of EBS bandwidth and
    // 260,000 IOPS, the highest of the R5 ladder, and 31 EBS volumes whatever the size — more
    // than the shared pool leaves, those volumes being attached to the family's own EBS
    // hardware, while the limit itself is still reported as the `shared` kind. The family is
    // not among the hibernation prerequisites, and its bare metal size supports CloudWatch
    // action based recovery like every other size.
    PublishedInstanceTypeFacts::of("r5b.large")
        .ebs(1_250, 5_417, 10_000, 43_333)
        .ebs_attachments(31)
        .network("Up to 10 Gigabit")
        .interfaces(3)
        .ipv4_per_interface(10)
        .processor("Intel", 3.1)
        .auto_recovery(true)
        .hibernation(false)
        .host_features(false, false),
    PublishedInstanceTypeFacts::of("r5b.xlarge")
        .ebs(2_500, 10_833, 10_000, 43_333)
        .ebs_attachments(31)
        .network("Up to 10 Gigabit")
        .interfaces(4)
        .ipv4_per_interface(15)
        .processor("Intel", 3.1)
        .auto_recovery(true)
        .hibernation(false)
        .host_features(false, false),
    PublishedInstanceTypeFacts::of("r5b.2xlarge")
        .ebs(5_000, 21_667, 10_000, 43_333)
        .ebs_attachments(31)
        .network("Up to 10 Gigabit")
        .interfaces(4)
        .ipv4_per_interface(15)
        .processor("Intel", 3.1)
        .auto_recovery(true)
        .hibernation(false)
        .host_features(false, false),
    PublishedInstanceTypeFacts::of("r5b.4xlarge")
        .ebs(10_000, 43_333, 10_000, 43_333)
        .ebs_attachments(31)
        .network("Up to 10 Gigabit")
        .interfaces(8)
        .ipv4_per_interface(30)
        .processor("Intel", 3.1)
        .auto_recovery(true)
        .hibernation(false)
        .host_features(false, false),
    PublishedInstanceTypeFacts::of("r5b.8xlarge")
        .ebs(20_000, 86_667, 20_000, 86_667)
        .ebs_attachments(31)
        .network("10 Gigabit")
        .interfaces(8)
        .ipv4_per_interface(30)
        .processor("Intel", 3.1)
        .auto_recovery(true)
        .hibernation(false)
        .host_features(false, false),
    PublishedInstanceTypeFacts::of("r5b.12xlarge")
        .ebs(30_000, 130_000, 30_000, 130_000)
        .ebs_attachments(31)
        .network("12 Gigabit")
        .interfaces(8)
        .ipv4_per_interface(30)
        .processor("Intel", 3.1)
        .auto_recovery(true)
        .hibernation(false)
        .host_features(false, false),
    PublishedInstanceTypeFacts::of("r5b.16xlarge")
        .ebs(40_000, 173_333, 40_000, 173_333)
        .ebs_attachments(31)
        .network("20 Gigabit")
        .interfaces(15)
        .ipv4_per_interface(50)
        .processor("Intel", 3.1)
        .auto_recovery(true)
        .hibernation(false)
        .host_features(false, false),
    PublishedInstanceTypeFacts::of("r5b.24xlarge")
        .ebs(60_000, 260_000, 60_000, 260_000)
        .ebs_attachments(31)
        .network("25 Gigabit")
        .interfaces(15)
        .ipv4_per_interface(50)
        .processor("Intel", 3.1)
        .auto_recovery(true)
        .hibernation(false)
        .host_features(false, false),
    PublishedInstanceTypeFacts::of("r5b.metal")
        .ebs(60_000, 260_000, 60_000, 260_000)
        .ebs_attachments(31)
        .network("25 Gigabit")
        .interfaces(15)
        .ipv4_per_interface(50)
        .processor("Intel", 3.1)
        .auto_recovery(true)
        .hibernation(false)
        .host_features(false, false),
    // R8ib — memory optimized on the EBS-optimized hardware the family's `b` names, 8th
    // generation Intel at a sustained 3.9 GHz: 3,125 Mbps and 15,000 IOPS of EBS performance
    // per `xlarge` of the ladder, dedicated from the 8xlarge up with the sizes below it
    // bursting to that 8xlarge figure, and up to 300 Gbps of bandwidth with the family's
    // published ceiling of 1,200,000 IOPS on its largest size. Those volumes attach to EBS
    // hardware of their own, so the family's 32 attachments are the EBS devices' own limit
    // rather than a share of the Nitro pool. ENA Express is not offered on the family, its
    // connection tracking applies the shorter established-TCP idle timeout the newest hardware
    // publishes, the Elastic Fabric Adapter is offered on the two largest sizes and the bare
    // metal ones alone, and from the 24xlarge up its cores are configured two at a time. ENA
    // Express is not offered below the 8xlarge, the sizes under it carrying less than the
    // family's full network share, and the family's hardware carries no local PTP clock and
    // copies no session's packets for Traffic Mirroring, while reboot migration is offered on
    // it. It takes no interface attachment from a secondary network, so it reports no
    // secondary interface limit either.
    PublishedInstanceTypeFacts::of("r8ib.large")
        .ebs(1_563, 7_500, 25_000, 120_000)
        .dedicated_ebs_attachments(32)
        .processor("Intel", 3.9)
        .ena_srd(false)
        .tcp_established_timeout(SHORT_TCP_ESTABLISHED_TIMEOUT)
        .host_features(true, false)
        .traffic_mirroring(false)
        .secondary_network(false),
    PublishedInstanceTypeFacts::of("r8ib.xlarge")
        .ebs(3_125, 15_000, 25_000, 120_000)
        .dedicated_ebs_attachments(32)
        .processor("Intel", 3.9)
        .ena_srd(false)
        .tcp_established_timeout(SHORT_TCP_ESTABLISHED_TIMEOUT)
        .host_features(true, false)
        .traffic_mirroring(false)
        .secondary_network(false),
    PublishedInstanceTypeFacts::of("r8ib.2xlarge")
        .ebs(6_250, 30_000, 25_000, 120_000)
        .dedicated_ebs_attachments(32)
        .processor("Intel", 3.9)
        .ena_srd(false)
        .tcp_established_timeout(SHORT_TCP_ESTABLISHED_TIMEOUT)
        .host_features(true, false)
        .traffic_mirroring(false)
        .secondary_network(false),
    PublishedInstanceTypeFacts::of("r8ib.4xlarge")
        .ebs(12_500, 60_000, 25_000, 120_000)
        .dedicated_ebs_attachments(32)
        .processor("Intel", 3.9)
        .ena_srd(false)
        .tcp_established_timeout(SHORT_TCP_ESTABLISHED_TIMEOUT)
        .host_features(true, false)
        .traffic_mirroring(false)
        .secondary_network(false),
    PublishedInstanceTypeFacts::of("r8ib.8xlarge")
        .ebs(25_000, 120_000, 25_000, 120_000)
        .dedicated_ebs_attachments(32)
        .processor("Intel", 3.9)
        .tcp_established_timeout(SHORT_TCP_ESTABLISHED_TIMEOUT)
        .host_features(true, false)
        .traffic_mirroring(false)
        .secondary_network(false),
    PublishedInstanceTypeFacts::of("r8ib.12xlarge")
        .ebs(37_500, 180_000, 37_500, 180_000)
        .dedicated_ebs_attachments(32)
        .processor("Intel", 3.9)
        .tcp_established_timeout(SHORT_TCP_ESTABLISHED_TIMEOUT)
        .host_features(true, false)
        .traffic_mirroring(false)
        .secondary_network(false),
    PublishedInstanceTypeFacts::of("r8ib.16xlarge")
        .ebs(50_000, 240_000, 50_000, 240_000)
        .dedicated_ebs_attachments(32)
        .processor("Intel", 3.9)
        .tcp_established_timeout(SHORT_TCP_ESTABLISHED_TIMEOUT)
        .host_features(true, false)
        .traffic_mirroring(false)
        .secondary_network(false),
    PublishedInstanceTypeFacts::of("r8ib.24xlarge")
        .cores_in_steps_of(2)
        .ebs(75_000, 360_000, 75_000, 360_000)
        .dedicated_ebs_attachments(32)
        .processor("Intel", 3.9)
        .tcp_established_timeout(SHORT_TCP_ESTABLISHED_TIMEOUT)
        .host_features(true, false)
        .traffic_mirroring(false)
        .secondary_network(false),
    PublishedInstanceTypeFacts::of("r8ib.32xlarge")
        .cores_in_steps_of(2)
        .ebs(100_000, 480_000, 100_000, 480_000)
        .dedicated_ebs_attachments(32)
        .processor("Intel", 3.9)
        .tcp_established_timeout(SHORT_TCP_ESTABLISHED_TIMEOUT)
        .host_features(true, false)
        .traffic_mirroring(false)
        .secondary_network(false),
    PublishedInstanceTypeFacts::of("r8ib.48xlarge")
        .cores_in_steps_of(2)
        .ebs(150_000, 720_000, 150_000, 720_000)
        .dedicated_ebs_attachments(32)
        .efa(1)
        .processor("Intel", 3.9)
        .tcp_established_timeout(SHORT_TCP_ESTABLISHED_TIMEOUT)
        .host_features(true, false)
        .traffic_mirroring(false)
        .secondary_network(false),
    PublishedInstanceTypeFacts::of("r8ib.96xlarge")
        .cores_in_steps_of(2)
        .ebs(300_000, 1_200_000, 300_000, 1_200_000)
        .dedicated_ebs_attachments(32)
        .efa(1)
        .processor("Intel", 3.9)
        .tcp_established_timeout(SHORT_TCP_ESTABLISHED_TIMEOUT)
        .host_features(true, false)
        .traffic_mirroring(false)
        .secondary_network(false),
    PublishedInstanceTypeFacts::of("r8ib.metal-48xl")
        .cores_in_steps_of(2)
        .ebs(150_000, 720_000, 150_000, 720_000)
        .dedicated_ebs_attachments(32)
        .efa(1)
        .processor("Intel", 3.9)
        .tcp_established_timeout(SHORT_TCP_ESTABLISHED_TIMEOUT)
        .host_features(true, false)
        .traffic_mirroring(false)
        .secondary_network(false),
    PublishedInstanceTypeFacts::of("r8ib.metal-96xl")
        .cores_in_steps_of(2)
        .ebs(300_000, 1_200_000, 300_000, 1_200_000)
        .dedicated_ebs_attachments(32)
        .efa(1)
        .processor("Intel", 3.9)
        .tcp_established_timeout(SHORT_TCP_ESTABLISHED_TIMEOUT)
        .host_features(true, false)
        .traffic_mirroring(false)
        .secondary_network(false),
    // R7gd — memory optimized with local NVMe on AWS Graviton3, at a sustained 2.6 GHz. The
    // family is not the latest generation of its family. Its EBS figures are its own rows of
    // "Amazon EBS optimized instance types": the sizes below the `8xlarge` publish a baseline
    // they burst above, up to 10,000 Mbps and 40,000 IOPS, and the bandwidth is dedicated from
    // that size up. The local devices are the share of the host's two 1,900 GB devices each
    // size holds, which is one device of a size of its own up to the `8xlarge` and two devices
    // above it; because those devices take part of the shared attachment pool, the EBS volume
    // limit follows from the device count the derivation above already counts, so these rows
    // leave it unset. The smallest size takes two network interfaces of four private addresses
    // each rather than the count its step of the size ladder publishes, Dedicated Hosts are
    // offered on the family, and the Elastic Fabric Adapter on the two sizes that hold the
    // whole host.
    PublishedInstanceTypeFacts::of("r7gd.medium")
        .previous_generation()
        .ebs(315, 2_500, 10_000, 40_000)
        .instance_store(1, 59)
        .network("Up to 12.5 Gigabit")
        .interfaces(2)
        .ipv4_per_interface(4)
        .efa(0)
        .processor("AWS", 2.6)
        .dedicated_hosts(true),
    PublishedInstanceTypeFacts::of("r7gd.large")
        .previous_generation()
        .ebs(630, 3_600, 10_000, 40_000)
        .instance_store(1, 118)
        .network("Up to 12.5 Gigabit")
        .efa(0)
        .processor("AWS", 2.6)
        .dedicated_hosts(true),
    PublishedInstanceTypeFacts::of("r7gd.xlarge")
        .previous_generation()
        .ebs(1_250, 6_000, 10_000, 40_000)
        .instance_store(1, 237)
        .network("Up to 12.5 Gigabit")
        .efa(0)
        .processor("AWS", 2.6)
        .dedicated_hosts(true),
    PublishedInstanceTypeFacts::of("r7gd.2xlarge")
        .previous_generation()
        .ebs(2_500, 12_000, 10_000, 40_000)
        .instance_store(1, 474)
        .network("Up to 15 Gigabit")
        .efa(0)
        .processor("AWS", 2.6)
        .dedicated_hosts(true),
    PublishedInstanceTypeFacts::of("r7gd.4xlarge")
        .previous_generation()
        .ebs(5_000, 20_000, 10_000, 40_000)
        .instance_store(1, 950)
        .network("Up to 15 Gigabit")
        .efa(0)
        .processor("AWS", 2.6)
        .dedicated_hosts(true),
    PublishedInstanceTypeFacts::of("r7gd.8xlarge")
        .previous_generation()
        .ebs(10_000, 40_000, 10_000, 40_000)
        .instance_store(1, 1_900)
        .network("15 Gigabit")
        .efa(0)
        .processor("AWS", 2.6)
        .dedicated_hosts(true),
    PublishedInstanceTypeFacts::of("r7gd.12xlarge")
        .previous_generation()
        .ebs(15_000, 60_000, 15_000, 60_000)
        .instance_store(2, 1_425)
        .network("22.5 Gigabit")
        .efa(0)
        .processor("AWS", 2.6)
        .dedicated_hosts(true),
    PublishedInstanceTypeFacts::of("r7gd.16xlarge")
        .previous_generation()
        .ebs(20_000, 80_000, 20_000, 80_000)
        .instance_store(2, 1_900)
        .network("30 Gigabit")
        .efa(1)
        .processor("AWS", 2.6)
        .dedicated_hosts(true),
    PublishedInstanceTypeFacts::of("r7gd.metal")
        .previous_generation()
        .ebs(20_000, 80_000, 20_000, 80_000)
        .instance_store(2, 1_900)
        .network("30 Gigabit")
        .efa(1)
        .processor("AWS", 2.6)
        .dedicated_hosts(true),
    // T3 — burstable performance, Intel Xeon Scalable at a sustained 2.5 GHz. The family's
    // EBS figures do not follow the size ladder: they double up to `large` and then stop, so
    // `large`, `xlarge` and `2xlarge` share one row, and `large` takes 12 private addresses
    // per interface rather than the 10 the ladder's step publishes. Dedicated Hosts ARE
    // offered on the family.
    PublishedInstanceTypeFacts::of("t3.nano")
        .ebs(43, 250, 2_085, 11_800)
        .network("Up to 5 Gigabit")
        .interfaces(2)
        .ipv4_per_interface(2)
        .processor("Intel", 2.5)
        .dedicated_hosts(true)
        .host_features(false, false),
    PublishedInstanceTypeFacts::of("t3.micro")
        .ebs(87, 500, 2_085, 11_800)
        .network("Up to 5 Gigabit")
        .interfaces(2)
        .ipv4_per_interface(2)
        .processor("Intel", 2.5)
        .dedicated_hosts(true)
        .host_features(false, false),
    PublishedInstanceTypeFacts::of("t3.small")
        .ebs(174, 1_000, 2_085, 11_800)
        .network("Up to 5 Gigabit")
        .interfaces(3)
        .ipv4_per_interface(4)
        .processor("Intel", 2.5)
        .dedicated_hosts(true)
        .host_features(false, false),
    PublishedInstanceTypeFacts::of("t3.medium")
        .ebs(347, 2_000, 2_085, 11_800)
        .network("Up to 5 Gigabit")
        .interfaces(3)
        .ipv4_per_interface(6)
        .processor("Intel", 2.5)
        .dedicated_hosts(true)
        .host_features(false, false),
    PublishedInstanceTypeFacts::of("t3.large")
        .ebs(695, 4_000, 2_780, 15_700)
        .network("Up to 5 Gigabit")
        .interfaces(3)
        .ipv4_per_interface(12)
        .processor("Intel", 2.5)
        .dedicated_hosts(true)
        .host_features(false, false),
    PublishedInstanceTypeFacts::of("t3.xlarge")
        .ebs(695, 4_000, 2_780, 15_700)
        .network("Up to 5 Gigabit")
        .interfaces(4)
        .ipv4_per_interface(15)
        .processor("Intel", 2.5)
        .dedicated_hosts(true)
        .host_features(false, false),
    PublishedInstanceTypeFacts::of("t3.2xlarge")
        .ebs(695, 4_000, 2_780, 15_700)
        .network("Up to 5 Gigabit")
        .interfaces(4)
        .ipv4_per_interface(15)
        .processor("Intel", 2.5)
        .dedicated_hosts(true)
        .host_features(false, false),
    // X2iedn — memory optimized for in-memory databases, 3rd generation Intel Xeon Scalable
    // (Ice Lake) at a sustained 3.5 GHz, 32 GiB of memory per vCPU, and ONE local NVMe device
    // up to the 16xlarge size and two above it, each size's devices being the share of the
    // host's two 1,900 GB devices it holds. The family's EBS bandwidth is dedicated from the
    // 8xlarge up, the three sizes below that one bursting to its figure, and its IOPS figure
    // is 3.25 per Mbps at every size. CloudWatch action based recovery is offered at every
    // size, the bare metal one included, and the Elastic Fabric Adapter only on the two sizes
    // that hold the whole host. ENA Express is offered from the 8xlarge up, the sizes below
    // it carrying less than the family's full network share, and neither reboot migration nor
    // a PTP hardware clock is offered at all. The cores are configured one per socket from the
    // 2xlarge up, the two sizes AWS publishes every count for being the smallest one and the
    // bare metal one. The bare metal size is offered the legacy BIOS boot mode alone, and
    // holds 31 EBS volumes — more than the shared pool leaves, those volumes being attached
    // beside the pool rather than out of it, while the limit is still of the `shared` kind.
    PublishedInstanceTypeFacts::of("x2iedn.xlarge")
        .memory_gib(128)
        .ebs(2_500, 8_125, 20_000, 65_000)
        .instance_store(1, 118)
        .network("Up to 25 Gigabit")
        .efa(0)
        .processor("Intel", 3.5)
        .auto_recovery(true)
        .ena_srd(false)
        .host_features(false, false),
    PublishedInstanceTypeFacts::of("x2iedn.2xlarge")
        .memory_gib(256)
        .cores_in_steps_of(2)
        .ebs(5_000, 16_250, 20_000, 65_000)
        .instance_store(1, 237)
        .network("Up to 25 Gigabit")
        .efa(0)
        .processor("Intel", 3.5)
        .auto_recovery(true)
        .ena_srd(false)
        .host_features(false, false),
    PublishedInstanceTypeFacts::of("x2iedn.4xlarge")
        .memory_gib(512)
        .cores_in_steps_of(2)
        .ebs(10_000, 32_500, 20_000, 65_000)
        .instance_store(1, 475)
        .network("Up to 25 Gigabit")
        .efa(0)
        .processor("Intel", 3.5)
        .auto_recovery(true)
        .ena_srd(false)
        .host_features(false, false),
    PublishedInstanceTypeFacts::of("x2iedn.8xlarge")
        .memory_gib(1_024)
        .cores_in_steps_of(2)
        .ebs(20_000, 65_000, 20_000, 65_000)
        .instance_store(1, 950)
        .network("25 Gigabit")
        .efa(0)
        .processor("Intel", 3.5)
        .auto_recovery(true)
        .ena_srd(true)
        .host_features(false, false),
    PublishedInstanceTypeFacts::of("x2iedn.16xlarge")
        .memory_gib(2_048)
        .cores_in_steps_of(2)
        .ebs(40_000, 130_000, 40_000, 130_000)
        .instance_store(1, 1_900)
        .network("50 Gigabit")
        .efa(0)
        .processor("Intel", 3.5)
        .auto_recovery(true)
        .ena_srd(true)
        .host_features(false, false),
    PublishedInstanceTypeFacts::of("x2iedn.24xlarge")
        .memory_gib(3_072)
        .cores_in_steps_of(2)
        .ebs(60_000, 195_000, 60_000, 195_000)
        .instance_store(2, 1_425)
        .network("75 Gigabit")
        .efa(0)
        .processor("Intel", 3.5)
        .auto_recovery(true)
        .ena_srd(true)
        .host_features(false, false),
    PublishedInstanceTypeFacts::of("x2iedn.32xlarge")
        .memory_gib(4_096)
        .cores_in_steps_of(2)
        .ebs(80_000, 260_000, 80_000, 260_000)
        .instance_store(2, 1_900)
        .network("100 Gigabit")
        .efa(1)
        .processor("Intel", 3.5)
        .auto_recovery(true)
        .ena_srd(true)
        .host_features(false, false),
    PublishedInstanceTypeFacts::of("x2iedn.metal")
        .memory_gib(4_096)
        .ebs(80_000, 260_000, 80_000, 260_000)
        .ebs_attachments(31)
        .instance_store(2, 1_900)
        .network("100 Gigabit")
        .efa(1)
        .processor("Intel", 3.5)
        .auto_recovery(true)
        .ena_srd(true)
        .boot_modes(&["legacy-bios"])
        .host_features(false, false),
];

/// The published facts of `instance_type`, or `None` where the catalog holds no row for it
/// and every member is the derivation's.
pub fn published_instance_type_facts(
    instance_type: &str,
) -> Option<&'static PublishedInstanceTypeFacts> {
    PUBLISHED_INSTANCE_TYPE_FACTS
        .iter()
        .find(|facts| facts.instance_type == instance_type)
}

/// Every instance type the table above holds a row for.
///
/// A row is reached by its instance type ([`published_instance_type_facts`]), so a key that
/// names no type of the catalog would be a row nothing ever reads; this is what lets a test
/// hold every key to being one.
pub fn published_instance_types() -> impl Iterator<Item = &'static str> {
    PUBLISHED_INSTANCE_TYPE_FACTS
        .iter()
        .map(|facts| facts.instance_type)
}

/// The local instance store of one instance type: the devices `facts` publishes for it, or
/// the `derived` store of its name where its row publishes none.
///
/// A row that publishes devices says how many there are and how big they are, which is the
/// half of the fact no name yields; the device MODEL — magnetic or solid state, NVMe or not,
/// and therefore its encryption — comes from the derived store of the same type where its
/// name yields one, and from the name itself where it does not. A row that publishes NO
/// devices says the type carries no instance store at all, which overrides the derivation
/// rather than falling back to it: a family whose name reads as having local devices can
/// still have a size that carries none.
pub(super) fn published_instance_storage(
    facts: Option<&'static PublishedInstanceTypeFacts>,
    name: &InstanceTypeName<'_>,
    derived: Option<InstanceStorageFacts>,
) -> Option<InstanceStorageFacts> {
    let Some((devices, gb_each)) = facts.and_then(|facts| facts.instance_store_devices) else {
        return derived;
    };
    if devices <= 0 {
        return None;
    }
    let (disk_type, nvme) = match derived {
        Some(derived) => (derived.disk_type, derived.nvme),
        None => instance_storage_device_model(name),
    };
    Some(InstanceStorageFacts {
        total_size_gb: gb_each.saturating_mul(i64::from(devices)),
        disk_count: devices,
        disk_size_gb: gb_each,
        disk_type,
        nvme,
    })
}
