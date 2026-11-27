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
    EbsCardInfo, FpgaDeviceInfo, FpgaDeviceMemoryInfo, FpgaInfo, GpuDeviceInfo,
    GpuDeviceMemoryInfo, GpuInfo, InferenceAcceleratorInfo, InferenceDeviceInfo,
    InferenceDeviceMemoryInfo, NeuronDeviceCoreInfo, NeuronDeviceInfo, NeuronDeviceMemoryInfo,
    NeuronInfo,
};

mod amd_epyc_rows;
mod block_storage_optimized_graviton_memory_rows;
mod burstable_rows;
mod burstable_xen_rows;
mod dense_hdd_storage_rows;
mod eighth_generation_intel_rows;
mod fifth_generation_intel_compute_rows;
mod fifth_generation_intel_nvme_compute_rows;
mod fifth_generation_intel_rows;
mod fifth_generation_network_optimized_rows;
mod first_generation_graviton_rows;
mod fourth_generation_xen_rows;
mod general_purpose_nvme_rows;
mod gpu_rows;
mod intel_nvme_compute_rows;
mod intel_storage_optimized_rows;
mod mac_host_rows;
mod memory_optimized_nvme_rows;
mod network_optimized_rows;
mod neuron_accelerated_rows;
mod newest_generation_graviton_rows;
mod previous_generation_rows;
mod seventh_generation_graphics_rows;
mod sixth_generation_graviton_rows;
mod storage_optimized_graviton_rows;
mod xen_high_memory_rows;
mod xen_storage_optimized_rows;

/// The EBS-optimized performance AWS publishes for one instance type: the two bandwidth and
/// the two IOPS figures of its row in "Amazon EBS optimized instance types".
///
/// A size with dedicated bandwidth publishes its sustained figure as both its baseline and
/// its maximum; a size below the family's dedicated row publishes a baseline it bursts above
/// up to that maximum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PublishedEbsPerformance {
    /// The bandwidth the type sustains, in HALVES of an Mbps.
    ///
    /// The unit is a half because the published figure of a size whose share of its host's
    /// bandwidth is not a whole number of Mbps carries one — 1,562.5 Mbps is 3,125 halves —
    /// and the two members that report it relate differently to that half: the integer
    /// `baselineBandwidthInMbps` cannot hold it and reports the whole Mbps, while
    /// `baselineThroughputInMBps` carries the megabytes of the UNtruncated figure, which is
    /// how the published table's Mbps and MB/s columns relate for such a size. Holding the
    /// figure in this one unit is what keeps the two members, and the per-card shares of
    /// them, from disagreeing about it.
    pub baseline_bandwidth_half_mbps: i32,
    /// The I/O operations per second it sustains.
    pub baseline_iops: i32,
    /// The bandwidth, in Mbps, it can reach.
    pub maximum_bandwidth_mbps: i32,
    /// The I/O operations per second it can reach.
    pub maximum_iops: i32,
}

/// The halves of an Mbps in one Mbps, the unit
/// [`PublishedEbsPerformance::baseline_bandwidth_half_mbps`] counts a published sustained
/// bandwidth in.
const EBS_HALVES_PER_MBPS: i32 = 2;

impl PublishedEbsPerformance {
    /// The whole Mbps of a sustained bandwidth of `halves`, as the integer
    /// `baselineBandwidthInMbps` member reports it: the member cannot hold a half, so a
    /// published figure that carries one reports the whole Mbps below it.
    const fn whole_mbps(halves: i32) -> i32 {
        halves / EBS_HALVES_PER_MBPS
    }

    /// The megabytes per second of a sustained bandwidth of `halves`, as the
    /// `baselineThroughputInMBps` member reports it: the megabits of the WHOLE published
    /// figure, half included, in the other unit.
    fn megabytes_of_halves(halves: i32) -> f64 {
        f64::from(halves)
            / f64::from(EBS_HALVES_PER_MBPS)
            / EBS_MEGABITS_PER_MEGABYTE
    }

    /// This row as the modelled `EbsOptimizedInfo`.
    ///
    /// The throughput members are the bandwidth ones in the other unit — a megabyte being
    /// [`EBS_MEGABITS_PER_MEGABYTE`] megabits — which is exactly how the published table's
    /// Mbps and MB/s columns relate.
    pub fn to_ebs_optimized_info(self) -> EbsOptimizedInfo {
        let megabytes = |mbps: i32| f64::from(mbps) / EBS_MEGABITS_PER_MEGABYTE;
        EbsOptimizedInfo {
            baseline_bandwidth_in_mbps: Some(Self::whole_mbps(self.baseline_bandwidth_half_mbps)),
            baseline_throughput_in_m_bps: Some(Self::megabytes_of_halves(
                self.baseline_bandwidth_half_mbps,
            )),
            baseline_iops: Some(self.baseline_iops),
            maximum_bandwidth_in_mbps: Some(self.maximum_bandwidth_mbps),
            maximum_throughput_in_m_bps: Some(megabytes(self.maximum_bandwidth_mbps)),
            maximum_iops: Some(self.maximum_iops),
        }
    }

    /// This row spread over the `cards` EBS cards the type carries, as
    /// `EbsInfo$EbsCards` reports them.
    ///
    /// The cards of one instance are identical and the instance's own figures are what its
    /// cards carry together — the same way [`super::network_cards`] reads the network cards of
    /// an instance — so each card takes an equal share of the four figures. A card's share of
    /// the sustained bandwidth is taken in the same halves the row holds it in, and reported
    /// through the same two readings, so a card cannot report that figure differently from the
    /// instance it belongs to.
    pub fn to_ebs_cards(self, cards: i32) -> Vec<EbsCardInfo> {
        let count = cards.max(1);
        let share = |figure: i32| (figure / count).max(1);
        let megabytes = |mbps: i32| f64::from(mbps) / EBS_MEGABITS_PER_MEGABYTE;
        let baseline_halves = share(self.baseline_bandwidth_half_mbps);
        (0..count)
            .map(|index| EbsCardInfo {
                ebs_card_index: Some(index),
                baseline_bandwidth_in_mbps: Some(Self::whole_mbps(baseline_halves)),
                baseline_throughput_in_m_bps: Some(Self::megabytes_of_halves(baseline_halves)),
                baseline_iops: Some(share(self.baseline_iops)),
                maximum_bandwidth_in_mbps: Some(share(self.maximum_bandwidth_mbps)),
                maximum_throughput_in_m_bps: Some(megabytes(share(
                    self.maximum_bandwidth_mbps,
                ))),
                maximum_iops: Some(share(self.maximum_iops)),
            })
            .collect()
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

/// The Neuron devices of one instance type — "describes the Neuron accelerator settings for
/// the instance type" (`InstanceTypeInfo$NeuronInfo`).
///
/// Like the inventories above, the devices of one instance are identical, so the whole
/// inventory is one device description and a count. What a Neuron device publishes BESIDE its
/// memory is the NeuronCores it carries and which version of the core those are
/// (`NeuronDeviceCoreInfo`), which is the half of the fact an Inference accelerator inventory
/// does not carry; the total device memory is one device's memory times the count.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PublishedNeuronDevices {
    /// How many Neuron devices the instance carries.
    pub count: i32,
    /// The device's name, as `NeuronDeviceInfo$Name` publishes it.
    pub name: &'static str,
    /// The NeuronCores ONE device carries (`NeuronDeviceCoreInfo$Count`).
    pub cores_each: i32,
    /// The version of those cores (`NeuronDeviceCoreInfo$Version`).
    pub core_version: i32,
    /// The memory of ONE device, in MiB.
    pub memory_mib_each: i32,
}

impl PublishedNeuronDevices {
    /// This inventory as the modelled `NeuronInfo`.
    pub fn to_neuron_info(self) -> NeuronInfo {
        NeuronInfo {
            neuron_devices: Some(vec![NeuronDeviceInfo {
                count: Some(self.count),
                name: Some(self.name.to_string()),
                core_info: Some(NeuronDeviceCoreInfo {
                    count: Some(self.cores_each),
                    version: Some(self.core_version),
                }),
                memory_info: Some(NeuronDeviceMemoryInfo {
                    size_in_mi_b: Some(self.memory_mib_each),
                }),
            }]),
            total_neuron_device_memory_in_mi_b: Some(
                self.memory_mib_each.saturating_mul(self.count),
            ),
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
    /// Is it "a burstable performance T instance type"
    /// (`InstanceTypeInfo$BurstablePerformanceSupported`), where the answer for the type is not
    /// the one its family letter gives: the first burstable family predates the CPU credit
    /// model that member reports, so AWS publishes it as not one.
    pub burstable_performance_supported: Option<bool>,
    /// Its memory, in MiB, where the family's ratio of memory to vCPUs is its own.
    pub memory_mib: Option<i64>,
    /// The vCPUs it runs with, where AWS's figure for the type is its own rather than the one
    /// its step of the size ladder carries.
    pub default_vcpus: Option<i32>,
    /// The threads each of its cores runs (`VCpuInfo$DefaultThreadsPerCore`), where AWS's
    /// figure for the type is its own rather than the one its processor's architecture gives:
    /// a type whose hardware runs — or is shipped with — one thread per core publishes its
    /// vCPUs as that many CORES.
    pub threads_per_core: Option<i32>,
    /// The step the published core counts of the type go up in (`VCpuInfo$ValidCores`): a
    /// type whose cores sit on more than one processor socket takes them one per socket, so
    /// its counts are multiples of the sockets rather than every count up to its default.
    pub valid_core_step: Option<i32>,
    /// The smallest core count AWS publishes for the type, where its published list starts
    /// above its step rather than at it.
    pub valid_core_minimum: Option<i32>,
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
    /// Whether that store is encrypted (`InstanceStorageInfo$EncryptionSupport`), where the
    /// published answer for the type is not the one its device model gives: a store AWS
    /// encrypts with a key the host holds reports `required` whether or not the devices are
    /// presented as NVMe ones, which `InstanceStorageInfo$NvmeSupport` answers on its own.
    pub instance_store_encrypted: Option<bool>,
    /// "The network performance", as the published table writes it.
    pub network_performance: Option<&'static str>,
    /// The network bandwidth AWS publishes for the type, in Gbps, as (the figure it sustains,
    /// the figure it can reach).
    ///
    /// This is the ONE published network figure of the type, and what differs between the
    /// members that report it is only the reading: `NetworkInfo$NetworkPerformance` is its
    /// TEXT as the published table writes that column — `Up to` the ceiling where the size
    /// sustains less than it reaches, and the figure itself where it sustains what it reaches
    /// ([`PublishedInstanceTypeFacts::published_network_performance`]) — while a network card
    /// of the type reports the same figure in Gbps through
    /// `NetworkCardInfo$BaselineBandwidthInGbps` and `$PeakBandwidthInGbps`. A row carries
    /// the pair rather than the text because the text of an `Up to` row does not say what the
    /// size SUSTAINS, which is half the published figure.
    pub network_bandwidth_gbps: Option<(f64, f64)>,
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
    /// The ENA queues each of the type's network interfaces is given by default
    /// (`NetworkCardInfo$DefaultEnaQueueCountPerInterface`): a figure of the adapter the type's
    /// network card carries, which AWS publishes per instance type. A row that publishes none
    /// leaves the member absent, the way every other member the catalog has no fact for is
    /// reported absent rather than invented.
    pub ena_queues_per_interface: Option<i32>,
    /// The ENA queue CEILINGS the type's network card publishes beside that default count: the
    /// queues ONE card holds (`NetworkCardInfo$MaximumEnaQueueCount`) and the most one interface
    /// of it may be given (`$MaximumEnaQueueCountPerInterface`), as (per card, per interface).
    /// A row that publishes no ceiling leaves both members absent, the way every other member
    /// the catalog has no fact for is reported absent rather than invented.
    pub ena_queue_limits: Option<(i32, i32)>,
    /// The interfaces one card takes BEYOND the ones the type publishes once its ENA queues are
    /// configured (`NetworkCardInfo$AdditionalFlexibleNetworkInterfaces`, "in addition to the
    /// base number specified by `maximumNetworkInterfaces`"), where the row publishes the
    /// figure.
    pub additional_flexible_network_interfaces: Option<i32>,
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
    /// The most private IPv4 addresses one interface attached from a secondary network can
    /// have (`NetworkInfo$Ipv4AddressesPerSecondaryInterface`), reported whatever the answer
    /// above is where the row publishes it.
    pub ipv4_addresses_per_secondary_interface: Option<i32>,
    /// Is Nitro Enclaves supported?
    pub nitro_enclaves_supported: Option<bool>,
    /// Is NitroTPM supported — the virtual trusted platform module AWS publishes the supported
    /// instance types of (`InstanceTypeInfo$NitroTpmSupport`) — where the published answer for
    /// the type is not the one its hypervisor gives?
    pub nitro_tpm_supported: Option<bool>,
    /// The EBS cards the type's volumes are attached to, where its row publishes hardware of
    /// its own for them (`EbsInfo$MaximumEbsCards`).
    pub ebs_cards: Option<i32>,
    /// The idle timeout, in seconds, its connection tracking applies to an established TCP
    /// connection by default (`DefaultConnectionTrackingConfiguration$DefaultTcpEstablishedTimeout`),
    /// where the type's own default is not the one every earlier type reports.
    pub default_tcp_established_timeout: Option<i32>,
    /// Whether Amazon EBS encryption is supported (`EbsInfo$EncryptionSupport`), where the
    /// published answer for the type is not the one its generation gives.
    pub ebs_encryption_supported: Option<bool>,
    /// The usage classes the type is offered in (`InstanceTypeInfo$SupportedUsageClasses`),
    /// where AWS's list for it is its own rather than the one its family gives — one value of
    /// the modelled `UsageClassType` per entry.
    pub usage_classes: Option<&'static [&'static str]>,
    /// Whether the type has an EBS-optimized mode — one value of the modelled
    /// `EbsOptimizedSupport` — where the published "EBS optimized" column of its row is not
    /// what its hardware generation gives. A type published as `unsupported` has no
    /// EBS-optimized performance to report either.
    pub ebs_optimized_support: Option<&'static str>,
    /// The placement group strategies the type is offered
    /// (`PlacementGroupInfo$SupportedStrategies`), where AWS's list for it is its own rather
    /// than the one its generation gives — one value of the modelled
    /// `PlacementGroupStrategy` per entry.
    pub placement_group_strategies: Option<&'static [&'static str]>,
    /// The root device types the type's AMIs may be backed by
    /// (`InstanceTypeInfo$SupportedRootDeviceTypes`), where AWS's list for it is its own —
    /// one value of the modelled `RootDeviceType` per entry.
    pub root_device_types: Option<&'static [&'static str]>,
    /// The additional processor features the type publishes
    /// (`ProcessorInfo$SupportedFeatures`), where AWS's list for it is not the one its
    /// processor's generation gives — one value of the modelled
    /// `SupportedAdditionalProcessorFeature` per entry.
    pub processor_features: Option<&'static [&'static str]>,
    /// Its FPGA accelerators.
    pub fpga_devices: Option<PublishedFpgaDevices>,
    /// Its GPU accelerators.
    pub gpu_devices: Option<PublishedGpuDevices>,
    /// Its Inference accelerators.
    pub inference_devices: Option<PublishedInferenceDevices>,
    /// Its Neuron devices, where it carries any.
    pub neuron_devices: Option<PublishedNeuronDevices>,
}

impl PublishedInstanceTypeFacts {
    /// A row that carries no fact yet: every column is the derivation's.
    const fn of(instance_type: &'static str) -> Self {
        Self {
            instance_type,
            current_generation: None,
            burstable_performance_supported: None,
            memory_mib: None,
            default_vcpus: None,
            threads_per_core: None,
            ebs_encryption_supported: None,
            usage_classes: None,
            valid_core_step: None,
            valid_core_minimum: None,
            ebs_performance: None,
            maximum_ebs_attachments: None,
            attachment_limit_type: None,
            instance_store_devices: None,
            instance_store_encrypted: None,
            network_performance: None,
            network_bandwidth_gbps: None,
            bandwidth_weightings: None,
            flexible_ena_queues_supported: None,
            ena_queues_per_interface: None,
            ena_queue_limits: None,
            additional_flexible_network_interfaces: None,
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
            ipv4_addresses_per_secondary_interface: None,
            nitro_enclaves_supported: None,
            nitro_tpm_supported: None,
            ebs_cards: None,
            default_tcp_established_timeout: None,
            ebs_optimized_support: None,
            placement_group_strategies: None,
            root_device_types: None,
            processor_features: None,
            fpga_devices: None,
            gpu_devices: None,
            inference_devices: None,
            neuron_devices: None,
        }
    }

    /// A family AWS's "Previous generation instances" page lists, so the type is not the
    /// latest generation of its family.
    const fn previous_generation(mut self) -> Self {
        self.current_generation = Some(false);
        self
    }

    /// Whether the type is a burstable performance T instance type, as AWS publishes it for the
    /// type rather than as its family letter reads.
    ///
    /// This answers the REPORTED member (and the burst model that follows from it) alone: which
    /// hypervisor the type runs on, whether it has an EBS-optimized mode and how its sizes are
    /// read off the ladder stay the family letter's own, so a type published as not burstable
    /// is still described by its family's hardware.
    const fn burstable_performance(mut self, supported: bool) -> Self {
        self.burstable_performance_supported = Some(supported);
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

    /// The threads each of the type's cores runs, where AWS's figure for it is its own rather
    /// than the one its architecture gives.
    const fn threads_per_core(mut self, threads: i32) -> Self {
        self.threads_per_core = Some(threads);
        self
    }

    /// Whether Amazon EBS encryption is supported, as the published answer for the type says.
    const fn ebs_encryption(mut self, supported: bool) -> Self {
        self.ebs_encryption_supported = Some(supported);
        self
    }

    /// The usage classes the type is offered in, where AWS's published list for it is its own.
    const fn usage_classes(mut self, classes: &'static [&'static str]) -> Self {
        self.usage_classes = Some(classes);
        self
    }

    /// The step the type's published core counts go up in: `sockets` for a type whose cores
    /// sit on that many processor sockets and are taken one per socket.
    const fn cores_in_steps_of(mut self, sockets: i32) -> Self {
        self.valid_core_step = Some(sockets);
        self
    }

    /// The smallest core count AWS's published list for the type starts at, where that is
    /// above the step the counts go up in.
    const fn cores_from(mut self, minimum: i32) -> Self {
        self.valid_core_minimum = Some(minimum);
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
        self,
        baseline_bandwidth_mbps: i32,
        baseline_iops: i32,
        maximum_bandwidth_mbps: i32,
        maximum_iops: i32,
    ) -> Self {
        self.ebs_in_half_mbps(
            baseline_bandwidth_mbps * EBS_HALVES_PER_MBPS,
            baseline_iops,
            maximum_bandwidth_mbps,
            maximum_iops,
        )
    }

    /// The same four figures for a type whose published sustained bandwidth is NOT a whole
    /// number of Mbps, that one given in halves of an Mbps: a published 1,562.5 Mbps is 3,125.
    const fn ebs_in_half_mbps(
        mut self,
        baseline_bandwidth_half_mbps: i32,
        baseline_iops: i32,
        maximum_bandwidth_mbps: i32,
        maximum_iops: i32,
    ) -> Self {
        self.ebs_performance = Some(PublishedEbsPerformance {
            baseline_bandwidth_half_mbps,
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

    /// Whether that store is encrypted, as the published answer for the type says — a fact of
    /// the store rather than of the device model, which is what lets a type whose devices are
    /// not NVMe still publish an encrypted store.
    const fn instance_store_encryption(mut self, encrypted: bool) -> Self {
        self.instance_store_encrypted = Some(encrypted);
        self
    }

    /// The type's network performance, as the published table writes it.
    const fn network(mut self, performance: &'static str) -> Self {
        self.network_performance = Some(performance);
        self
    }

    /// The network bandwidth the type publishes, in Gbps: the figure it sustains and the
    /// figure it can reach, which are the same where its share of the host is dedicated.
    const fn network_bandwidth(mut self, baseline_gbps: f64, peak_gbps: f64) -> Self {
        self.network_bandwidth_gbps = Some((baseline_gbps, peak_gbps));
        self
    }

    /// The type takes its cores ALL at once, so the one count it can be configured with is
    /// its default: a step no core count of the type is a multiple of, which is what
    /// [`super::InstanceTypeRecord::valid_cores`] reports that default alone for.
    const fn cores_all_at_once(mut self) -> Self {
        self.valid_core_step = Some(i32::MAX);
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

    /// The ENA queues each of the type's interfaces is given by default, as the published
    /// queue counts of its adapter say.
    const fn ena_queues(mut self, queues: i32) -> Self {
        self.ena_queues_per_interface = Some(queues);
        self
    }

    /// The ceilings those published counts carry: the queues one of the type's network cards
    /// holds, and the most queues one interface of it may be given.
    const fn ena_queue_limits(mut self, per_card: i32, per_interface: i32) -> Self {
        self.ena_queue_limits = Some((per_card, per_interface));
        self
    }

    /// The interfaces one card of the type takes beyond the ones it publishes, once its ENA
    /// queues are configured.
    const fn additional_flexible_interfaces(mut self, interfaces: i32) -> Self {
        self.additional_flexible_network_interfaces = Some(interfaces);
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

    /// The type's EBS volumes SHARE their limit with its network interfaces and its instance
    /// store devices, where the published figure itself is not recorded yet, which leaves the
    /// limit the one the derivation above counts.
    const fn shared_attachment_limit(mut self) -> Self {
        self.attachment_limit_type = Some(SHARED_ATTACHMENT_LIMIT);
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

    /// Whether NitroTPM is supported, as the published list of the instance types offered it
    /// says.
    const fn nitro_tpm(mut self, supported: bool) -> Self {
        self.nitro_tpm_supported = Some(supported);
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

    /// The most private IPv4 addresses one interface attached from a secondary network can
    /// have — none at all on a type that takes no such attachment.
    const fn ipv4_per_secondary_interface(mut self, addresses: i32) -> Self {
        self.ipv4_addresses_per_secondary_interface = Some(addresses);
        self
    }

    /// The EBS cards the type's volumes are attached to, where it carries hardware of its own
    /// for them.
    const fn ebs_cards(mut self, cards: i32) -> Self {
        self.ebs_cards = Some(cards);
        self
    }

    /// Whether Traffic Mirroring is supported, where the published answer for the type is not
    /// the one its hypervisor gives.
    const fn traffic_mirroring(mut self, supported: bool) -> Self {
        self.traffic_mirroring_supported = Some(supported);
        self
    }

    /// Whether the type has an EBS-optimized mode at all, as the published "EBS optimized"
    /// column of its row says.
    const fn ebs_optimized(mut self, support: &'static str) -> Self {
        self.ebs_optimized_support = Some(support);
        self
    }

    /// The placement group strategies the type is offered, where AWS's published list for it
    /// is its own.
    const fn placement_strategies(mut self, strategies: &'static [&'static str]) -> Self {
        self.placement_group_strategies = Some(strategies);
        self
    }

    /// The root device types the type's AMIs may be backed by, where AWS's published list for
    /// it is its own.
    const fn root_devices(mut self, types: &'static [&'static str]) -> Self {
        self.root_device_types = Some(types);
        self
    }

    /// The additional processor features the type publishes, where AWS's list for it is not the
    /// one its processor's generation gives.
    const fn processor_features(mut self, features: &'static [&'static str]) -> Self {
        self.processor_features = Some(features);
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

    /// The type's Neuron devices: `count` devices named `name`, each carrying `cores_each`
    /// NeuronCores of version `core_version` and `memory_mib_each` MiB of memory.
    const fn neurons(
        mut self,
        count: i32,
        name: &'static str,
        cores_each: i32,
        core_version: i32,
        memory_mib_each: i32,
    ) -> Self {
        self.neuron_devices = Some(PublishedNeuronDevices {
            count,
            name,
            cores_each,
            core_version,
            memory_mib_each,
        });
        self
    }

    /// The network performance this row publishes, as the published table writes that column,
    /// or `None` where the row publishes no network figure at all.
    ///
    /// A row that carries the TEXT of the column reports that; a row that carries the Gbps
    /// figure instead ([`PublishedInstanceTypeFacts::network_bandwidth_gbps`]) reports the
    /// text OF that figure, so the one published fact answers this member and the type's
    /// network cards alike rather than being written twice and able to disagree.
    pub fn published_network_performance(&self) -> Option<String> {
        if let Some(performance) = self.network_performance {
            return Some(performance.to_string());
        }
        let (baseline_gbps, peak_gbps) = self.network_bandwidth_gbps?;
        Some(network_performance(
            centi_gbps(baseline_gbps),
            centi_gbps(peak_gbps),
        ))
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

/// The `PlacementGroupStrategy` values a type offered EVERY placement group strategy
/// publishes, in the order AWS lists them.
const EVERY_PLACEMENT_STRATEGY: &[&str] = &["cluster", "partition", "spread"];

/// The `PlacementGroupStrategy` values a type that is NOT offered a cluster placement group
/// publishes: a cluster group packs its members onto one rack of hardware, which a type whose
/// share of a host is not a fixed allocation is not offered, while a partition and a spread
/// group place any instance.
const PARTITION_AND_SPREAD_STRATEGIES: &[&str] = &["partition", "spread"];

/// The `UsageClassType` values a type offered on NO Spot capacity publishes: On-Demand alone.
const ON_DEMAND_USAGE_CLASS: &[&str] = &["on-demand"];

/// The `RootDeviceType` values a type whose AMIs may be backed by either an EBS volume or its
/// local instance store publishes.
const EBS_AND_INSTANCE_STORE_ROOT_DEVICES: &[&str] = &["ebs", "instance-store"];

/// The `SupportedAdditionalProcessorFeature` value of a processor that runs a hypervisor inside
/// the guest, which is what a type offered nested virtualization publishes.
const NESTED_VIRTUALIZATION: &[&str] = &["nested-virtualization"];

/// The `BootModeType` value of a type offered the legacy BIOS boot mode alone, which is what a
/// bare metal size of an x86 family publishes.
const LEGACY_BIOS_BOOT_MODE: &[&str] = &["legacy-bios"];

/// The `BootModeType` value of a type offered the UEFI mode alone, which is what a bare metal
/// size of the newest x86 generation publishes — and what its trusted platform module needs to
/// be offered at all.
const UEFI_BOOT_MODE: &[&str] = &["uefi"];

/// The Elastic Fabric Adapters (`EfaInfo$MaximumEfaInterfaces`) the newest hardware
/// generation's whole-host sizes are offered: its adapter carries many more of them than the
/// single one an earlier generation's largest size publishes.
const NEWEST_GENERATION_EFA_INTERFACES: i32 = 16;

/// The network bandwidth, in Gbps, of a WHOLE host of the newest AWS Graviton hardware
/// generation: the figure the "Network performance" column publishes for the size that holds
/// one, which is both the bandwidth that size sustains and the bandwidth it reaches, there
/// being no wider share of the host to burst into.
const NEWEST_GENERATION_GRAVITON_HOST_GBPS: f64 = 100.0;

/// The ENA queues each interface of a newest-generation AWS Graviton instance that holds a
/// WHOLE host is given by default (`NetworkCardInfo$DefaultEnaQueueCountPerInterface`): the
/// published count of the adapter that host's network card carries.
const NEWEST_GENERATION_GRAVITON_ENA_QUEUES: i32 = 32;

/// The network interfaces a size that holds a whole host of the newest hardware generation can
/// have attached, and the private IPv4 addresses each of them takes: the figures the published
/// "IP addresses per network interface per instance type" table carries for a whole host of it,
/// well above the ones an earlier generation's whole-host sizes publish.
///
/// The widest VIRTUALIZED size of a host and the bare metal size of the same host are the same
/// hardware — one host, one network card — so the two publish these figures alike, exactly as
/// they publish one bandwidth figure.
const WHOLE_HOST_NETWORK_INTERFACES: i32 = 24;

/// The private IPv4 addresses one interface of that size takes.
const WHOLE_HOST_IPV4_PER_INTERFACE: i32 = 64;

/// The ENA queues ONE network card of such a size holds
/// (`NetworkCardInfo$MaximumEnaQueueCount`): the queue count its published interfaces are given
/// together — [`WHOLE_HOST_NETWORK_INTERFACES`] of them at
/// [`NEWEST_GENERATION_GRAVITON_ENA_QUEUES`] queues each.
const WHOLE_HOST_ENA_QUEUES_PER_CARD: i32 = 768;

/// The most ENA queues ONE interface of that card may be given
/// (`NetworkCardInfo$MaximumEnaQueueCountPerInterface`): four times the count an interface is
/// given by default, which an interface reaches on queues the card's others give up.
const WHOLE_HOST_ENA_QUEUES_PER_INTERFACE: i32 = 128;

/// The interfaces a card takes BEYOND the ones its instance type publishes once its ENA queues
/// are configured (`NetworkCardInfo$AdditionalFlexibleNetworkInterfaces`, "in addition to the
/// base number specified by `maximumNetworkInterfaces`"): none, where the queues the card holds
/// are exactly the ones its published interfaces are given by default.
const NO_ADDITIONAL_FLEXIBLE_INTERFACES: i32 = 0;

/// The sustained all-core frequency, in GHz, of the AWS Graviton2 processor the sixth
/// generation Graviton families are built on — shared by the burstable and the general purpose
/// families built on it, which is why it is here rather than in one of their tables.
const GRAVITON2_SUSTAINED_GHZ: f64 = 2.5;

/// The sustained all-core frequency, in GHz, of the Intel Xeon 6 processor the eighth
/// generation Intel families are built on — shared by the burstable and the compute families
/// built on it, which is why it is here rather than in one of their tables.
const XEON6_SUSTAINED_GHZ: f64 = 3.9;

/// The published hardware facts of the instance types recorded so far, one row per type.
///
/// Read through [`published_instance_type_facts`]. The rows are grouped by family, and each
/// family's comment says where its figures come from: the family's own specification page
/// (vCPUs, memory, instance store devices, network performance, accelerators), the
/// "Amazon EBS optimized instance types" table (the four EBS figures), "Instance volume
/// limits" (the EBS attachment limit and whether it is dedicated), and the "IP addresses per
/// network interface per instance type" table (the interface limits).
static PUBLISHED_INSTANCE_TYPE_FACTS: &[PublishedInstanceTypeFacts] = &[
    // C5ad — compute optimized, 2nd generation AMD EPYC 7002 series (7R32) at a sustained
    // 3.3 GHz, with local NVMe SSD devices. The family's EBS figures are DEDICATED from
    // `8xlarge` up — 3,170 Mbps and 13,300 IOPS there, then its own figure per size — and the
    // four sizes below it burst to that dedicated figure from a baseline that is their share
    // of the figures the family's largest size carries. The devices come out of the shared attachment pool, so the EBS volume limit each
    // size reports is what the pool leaves and these rows leave it unset. Dedicated Hosts are
    // not offered on the family, hibernation is not (a local instance store cannot be carried
    // over one), and neither reboot migration nor a PTP hardware clock is on this hardware
    // generation. The whole family's cores sit on ONE processor socket — its largest size
    // carries the 48 of an EPYC 7R32 — so they are configured one at a time.
    PublishedInstanceTypeFacts::of("c5ad.large")
        .ebs(200, 800, 3_170, 13_300)
        .instance_store(1, 75)
        .network("Up to 10 Gigabit")
        .processor("AMD", 3.3)
        .dedicated_hosts(false)
        .hibernation(false)
        .host_features(false, false),
    PublishedInstanceTypeFacts::of("c5ad.xlarge")
        .ebs(400, 1_600, 3_170, 13_300)
        .instance_store(1, 150)
        .network("Up to 10 Gigabit")
        .processor("AMD", 3.3)
        .dedicated_hosts(false)
        .hibernation(false)
        .host_features(false, false),
    PublishedInstanceTypeFacts::of("c5ad.2xlarge")
        .ebs(800, 3_200, 3_170, 13_300)
        .instance_store(1, 300)
        .network("Up to 10 Gigabit")
        .processor("AMD", 3.3)
        .dedicated_hosts(false)
        .hibernation(false)
        .host_features(false, false),
    PublishedInstanceTypeFacts::of("c5ad.4xlarge")
        .ebs(1_580, 6_320, 3_170, 13_300)
        .instance_store(2, 300)
        .network("Up to 10 Gigabit")
        .processor("AMD", 3.3)
        .dedicated_hosts(false)
        .hibernation(false)
        .host_features(false, false),
    PublishedInstanceTypeFacts::of("c5ad.8xlarge")
        .ebs(3_170, 13_300, 3_170, 13_300)
        .instance_store(2, 600)
        .network("10 Gigabit")
        .processor("AMD", 3.3)
        .dedicated_hosts(false)
        .hibernation(false)
        .host_features(false, false),
    PublishedInstanceTypeFacts::of("c5ad.12xlarge")
        .ebs(4_750, 20_000, 4_750, 20_000)
        .instance_store(2, 900)
        .network("12 Gigabit")
        .processor("AMD", 3.3)
        .dedicated_hosts(false)
        .hibernation(false)
        .host_features(false, false),
    PublishedInstanceTypeFacts::of("c5ad.16xlarge")
        .ebs(6_300, 26_700, 6_300, 26_700)
        .instance_store(2, 1_200)
        .network("20 Gigabit")
        .processor("AMD", 3.3)
        .dedicated_hosts(false)
        .hibernation(false)
        .host_features(false, false),
    PublishedInstanceTypeFacts::of("c5ad.24xlarge")
        .ebs(9_500, 40_000, 9_500, 40_000)
        .instance_store(2, 1_900)
        .network("20 Gigabit")
        .processor("AMD", 3.3)
        .dedicated_hosts(false)
        .hibernation(false)
        .host_features(false, false),
    // C8gb — compute optimized, AWS Graviton4, block storage optimized: the `b` capability
    // letter is "block storage optimized" (the naming conventions), and what it buys is EBS
    // hardware of the type's own. Every size's EBS figures are DEDICATED from `4xlarge` up,
    // scaling with the size — 100,000 Mbps and 480,000 IOPS at `16xlarge` — while the four
    // sizes below it burst to the `4xlarge` figure; the volumes are attached to that hardware
    // rather than out of the shared pool, so the limit is the type's own — 32 volumes up to
    // the `12xlarge`, then 48, 64 and 128 as the size grows — and is reported as the
    // `dedicated` kind. The largest size spreads its figures over two EBS cards. The
    // family's network is the Graviton4 compute ladder's, its connection tracking holds an
    // established TCP connection for the 350 s of this hardware generation, ENA Express is
    // offered from the `4xlarge` up, EFA on its three largest
    // sizes, and an enclave from two vCPUs up.
    // Reboot migration is offered on the family while a local PTP hardware clock is not, it
    // takes no interface attachments from a secondary network, so no address at all is
    // available on one, and the packets of a session cannot be mirrored under its guest. The
    // largest size
    // carries the cores of TWO 96-core Graviton4 sockets, taken one per socket, so its
    // published core counts go up in twos, and a network card per socket; every smaller size
    // sits on one socket.
    PublishedInstanceTypeFacts::of("c8gb.medium")
        .ebs(1_562, 7_500, 25_000, 120_000)
        .dedicated_ebs_attachments(32)
        .network("Up to 12.5 Gigabit")
        .interfaces(2)
        .ipv4_per_interface(4)
        .efa(0)
        .ena_srd(false)
        .tcp_established_timeout(350)
        .host_features(true, false)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0)
        .traffic_mirroring(false),
    PublishedInstanceTypeFacts::of("c8gb.large")
        .ebs(3_125, 15_000, 25_000, 120_000)
        .dedicated_ebs_attachments(32)
        .network("Up to 12.5 Gigabit")
        .efa(0)
        .ena_srd(false)
        .nitro_enclaves(true)
        .tcp_established_timeout(350)
        .host_features(true, false)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0)
        .traffic_mirroring(false),
    PublishedInstanceTypeFacts::of("c8gb.xlarge")
        .ebs(6_250, 30_000, 25_000, 120_000)
        .dedicated_ebs_attachments(32)
        .network("Up to 12.5 Gigabit")
        .efa(0)
        .ena_srd(false)
        .tcp_established_timeout(350)
        .host_features(true, false)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0)
        .traffic_mirroring(false),
    PublishedInstanceTypeFacts::of("c8gb.2xlarge")
        .ebs(12_500, 60_000, 25_000, 120_000)
        .dedicated_ebs_attachments(32)
        .network("Up to 15 Gigabit")
        .efa(0)
        .ena_srd(false)
        .tcp_established_timeout(350)
        .host_features(true, false)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0)
        .traffic_mirroring(false),
    PublishedInstanceTypeFacts::of("c8gb.4xlarge")
        .ebs(25_000, 120_000, 25_000, 120_000)
        .dedicated_ebs_attachments(32)
        .network("Up to 15 Gigabit")
        .efa(0)
        .tcp_established_timeout(350)
        .host_features(true, false)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0)
        .traffic_mirroring(false),
    PublishedInstanceTypeFacts::of("c8gb.8xlarge")
        .ebs(50_000, 240_000, 50_000, 240_000)
        .dedicated_ebs_attachments(32)
        .network("15 Gigabit")
        .efa(0)
        .tcp_established_timeout(350)
        .host_features(true, false)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0)
        .traffic_mirroring(false),
    PublishedInstanceTypeFacts::of("c8gb.12xlarge")
        .ebs(75_000, 360_000, 75_000, 360_000)
        .dedicated_ebs_attachments(32)
        .network("22.5 Gigabit")
        .efa(0)
        .tcp_established_timeout(350)
        .host_features(true, false)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0)
        .traffic_mirroring(false),
    PublishedInstanceTypeFacts::of("c8gb.16xlarge")
        .ebs(100_000, 480_000, 100_000, 480_000)
        .dedicated_ebs_attachments(48)
        .network("30 Gigabit")
        .efa(1)
        .tcp_established_timeout(350)
        .host_features(true, false)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0)
        .traffic_mirroring(false),
    PublishedInstanceTypeFacts::of("c8gb.24xlarge")
        .ebs(150_000, 720_000, 150_000, 720_000)
        .dedicated_ebs_attachments(64)
        .network("40 Gigabit")
        .efa(1)
        .tcp_established_timeout(350)
        .host_features(true, false)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0)
        .traffic_mirroring(false),
    PublishedInstanceTypeFacts::of("c8gb.48xlarge")
        .cores_in_steps_of(2)
        .ebs(300_000, 1_440_000, 300_000, 1_440_000)
        .dedicated_ebs_attachments(128)
        .ebs_cards(2)
        .network("50 Gigabit")
        .efa(1)
        .tcp_established_timeout(350)
        .host_features(true, false)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0)
        .traffic_mirroring(false),
    // C8g — compute optimized on AWS Graviton4, at a sustained 2.8 GHz on the sizes whose cores
    // sit on ONE processor socket and 2.7 on the two that carry both, with no local instance
    // store. The family's network and EBS bandwidth is CONFIGURABLE, so every size publishes the
    // bandwidth weightings a launch may ask for. Its EBS figures are the Graviton4 compute
    // ladder's rows of "Amazon EBS optimized instance types": the sizes below the `8xlarge`
    // publish a baseline they burst above, up to 10,000 Mbps and 40,000 IOPS, and the bandwidth
    // is dedicated from that size up, reaching 40,000 Mbps and 240,000 IOPS on the sizes that
    // hold the whole host. Those volumes are attached to EBS hardware of the type's own rather
    // than out of the pool shared with the interfaces, so the limit every size reports is the
    // EBS devices' own and is of the `dedicated` kind: 32 volumes up to the `12xlarge`, then 48,
    // 64 and 128 as the size grows, while each bare metal size reports its host's own limit — 79
    // volumes on the whole host and 39 on the half of it the smaller one holds. The network
    // figures are the family's published ones, up to 12.5 and up to 15 Gigabit below the
    // `8xlarge` and its share of the host's 50 from there up; the smallest size takes two
    // interfaces of four private addresses each rather than the count its step of the size
    // ladder publishes. The family takes no interface attachment from a secondary network, so no
    // private address at all is available on one, the number of ENA queues cannot be changed on
    // its adapter and the packets of a session cannot be copied under its guest. The Elastic
    // Fabric Adapter is offered on the two sizes that hold the whole host and on both bare metal
    // ones, which are also offered CloudWatch action based recovery, reboot migration and a
    // local PTP hardware clock; ENA Express is offered from the `8xlarge` up, the sizes under it
    // carrying less dedicated network bandwidth than its transport needs. An enclave is offered
    // from two vCPUs up on this architecture rather than from the four an x86 type needs, so the
    // `large` size carries one, and the `48xlarge` — the size that holds the whole host — takes
    // its cores all at once, so the one count it publishes is its default.
    PublishedInstanceTypeFacts::of("c8g.medium")
        .ebs(315, 2_500, 10_000, 40_000)
        .dedicated_ebs_attachments(32)
        .network("Up to 12.5 Gigabit")
        .bandwidth_weightings(CONFIGURABLE_BANDWIDTH_WEIGHTINGS)
        .interfaces(2)
        .ipv4_per_interface(4)
        .processor("AWS", 2.8)
        .ena_srd(false)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0)
        .flexible_ena_queues(false)
        .traffic_mirroring(false),
    PublishedInstanceTypeFacts::of("c8g.large")
        .ebs(630, 3_600, 10_000, 40_000)
        .dedicated_ebs_attachments(32)
        .network("Up to 12.5 Gigabit")
        .bandwidth_weightings(CONFIGURABLE_BANDWIDTH_WEIGHTINGS)
        .processor("AWS", 2.8)
        .ena_srd(false)
        .nitro_enclaves(true)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0)
        .flexible_ena_queues(false)
        .traffic_mirroring(false),
    PublishedInstanceTypeFacts::of("c8g.xlarge")
        .ebs(1_250, 6_000, 10_000, 40_000)
        .dedicated_ebs_attachments(32)
        .network("Up to 12.5 Gigabit")
        .bandwidth_weightings(CONFIGURABLE_BANDWIDTH_WEIGHTINGS)
        .processor("AWS", 2.8)
        .ena_srd(false)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0)
        .flexible_ena_queues(false)
        .traffic_mirroring(false),
    PublishedInstanceTypeFacts::of("c8g.2xlarge")
        .ebs(2_500, 12_000, 10_000, 40_000)
        .dedicated_ebs_attachments(32)
        .network("Up to 15 Gigabit")
        .bandwidth_weightings(CONFIGURABLE_BANDWIDTH_WEIGHTINGS)
        .processor("AWS", 2.8)
        .ena_srd(false)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0)
        .flexible_ena_queues(false)
        .traffic_mirroring(false),
    PublishedInstanceTypeFacts::of("c8g.4xlarge")
        .ebs(5_000, 20_000, 10_000, 40_000)
        .dedicated_ebs_attachments(32)
        .network("Up to 15 Gigabit")
        .bandwidth_weightings(CONFIGURABLE_BANDWIDTH_WEIGHTINGS)
        .processor("AWS", 2.8)
        .ena_srd(false)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0)
        .flexible_ena_queues(false)
        .traffic_mirroring(false),
    PublishedInstanceTypeFacts::of("c8g.8xlarge")
        .ebs(10_000, 40_000, 10_000, 40_000)
        .dedicated_ebs_attachments(32)
        .network("15 Gigabit")
        .bandwidth_weightings(CONFIGURABLE_BANDWIDTH_WEIGHTINGS)
        .processor("AWS", 2.8)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0)
        .flexible_ena_queues(false)
        .traffic_mirroring(false),
    PublishedInstanceTypeFacts::of("c8g.12xlarge")
        .ebs(15_000, 60_000, 15_000, 60_000)
        .dedicated_ebs_attachments(32)
        .network("22.5 Gigabit")
        .bandwidth_weightings(CONFIGURABLE_BANDWIDTH_WEIGHTINGS)
        .processor("AWS", 2.8)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0)
        .flexible_ena_queues(false)
        .traffic_mirroring(false),
    PublishedInstanceTypeFacts::of("c8g.16xlarge")
        .ebs(20_000, 80_000, 20_000, 80_000)
        .dedicated_ebs_attachments(48)
        .network("30 Gigabit")
        .bandwidth_weightings(CONFIGURABLE_BANDWIDTH_WEIGHTINGS)
        .processor("AWS", 2.8)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0)
        .flexible_ena_queues(false)
        .traffic_mirroring(false),
    PublishedInstanceTypeFacts::of("c8g.24xlarge")
        .ebs(30_000, 120_000, 30_000, 120_000)
        .dedicated_ebs_attachments(64)
        .network("40 Gigabit")
        .bandwidth_weightings(CONFIGURABLE_BANDWIDTH_WEIGHTINGS)
        .efa(1)
        .processor("AWS", 2.8)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0)
        .flexible_ena_queues(false)
        .traffic_mirroring(false),
    PublishedInstanceTypeFacts::of("c8g.48xlarge")
        .cores_in_steps_of(192)
        .ebs(40_000, 240_000, 40_000, 240_000)
        .dedicated_ebs_attachments(128)
        .network("50 Gigabit")
        .bandwidth_weightings(CONFIGURABLE_BANDWIDTH_WEIGHTINGS)
        .efa(1)
        .processor("AWS", 2.7)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0)
        .flexible_ena_queues(false)
        .traffic_mirroring(false),
    PublishedInstanceTypeFacts::of("c8g.metal-24xl")
        .ebs(30_000, 120_000, 30_000, 120_000)
        .dedicated_ebs_attachments(39)
        .network("40 Gigabit")
        .bandwidth_weightings(CONFIGURABLE_BANDWIDTH_WEIGHTINGS)
        .efa(1)
        .processor("AWS", 2.8)
        .ena_srd(true)
        .auto_recovery(true)
        .host_features(true, true)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0)
        .flexible_ena_queues(false)
        .traffic_mirroring(false),
    PublishedInstanceTypeFacts::of("c8g.metal-48xl")
        .ebs(40_000, 240_000, 40_000, 240_000)
        .dedicated_ebs_attachments(79)
        .network("50 Gigabit")
        .bandwidth_weightings(CONFIGURABLE_BANDWIDTH_WEIGHTINGS)
        .efa(1)
        .processor("AWS", 2.7)
        .ena_srd(true)
        .auto_recovery(true)
        .host_features(true, true)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0)
        .flexible_ena_queues(false)
        .traffic_mirroring(false),
    // C8gd — the same Graviton4 compute hardware as C8g above, carrying local NVMe devices, so
    // every figure of the C8g rows holds here and the rows add the family's published store: the
    // share of the host's devices each size holds, which is one device of a size of its own up to
    // the `8xlarge` and whole or half 1,900 GB devices above it. Because the local data of an
    // instance store cannot be carried over to another host, no size of the family is offered
    // CloudWatch action based recovery or reboot migration, the bare metal ones included — while
    // the PTP hardware clock of this hardware generation is offered on every size.
    PublishedInstanceTypeFacts::of("c8gd.medium")
        .ebs(315, 2_500, 10_000, 40_000)
        .dedicated_ebs_attachments(32)
        .instance_store(1, 59)
        .network("Up to 12.5 Gigabit")
        .bandwidth_weightings(CONFIGURABLE_BANDWIDTH_WEIGHTINGS)
        .interfaces(2)
        .ipv4_per_interface(4)
        .processor("AWS", 2.8)
        .ena_srd(false)
        .host_features(false, true)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0)
        .flexible_ena_queues(false)
        .traffic_mirroring(false),
    PublishedInstanceTypeFacts::of("c8gd.large")
        .ebs(630, 3_600, 10_000, 40_000)
        .dedicated_ebs_attachments(32)
        .instance_store(1, 118)
        .network("Up to 12.5 Gigabit")
        .bandwidth_weightings(CONFIGURABLE_BANDWIDTH_WEIGHTINGS)
        .processor("AWS", 2.8)
        .ena_srd(false)
        .nitro_enclaves(true)
        .host_features(false, true)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0)
        .flexible_ena_queues(false)
        .traffic_mirroring(false),
    PublishedInstanceTypeFacts::of("c8gd.xlarge")
        .ebs(1_250, 6_000, 10_000, 40_000)
        .dedicated_ebs_attachments(32)
        .instance_store(1, 237)
        .network("Up to 12.5 Gigabit")
        .bandwidth_weightings(CONFIGURABLE_BANDWIDTH_WEIGHTINGS)
        .processor("AWS", 2.8)
        .ena_srd(false)
        .host_features(false, true)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0)
        .flexible_ena_queues(false)
        .traffic_mirroring(false),
    PublishedInstanceTypeFacts::of("c8gd.2xlarge")
        .ebs(2_500, 12_000, 10_000, 40_000)
        .dedicated_ebs_attachments(32)
        .instance_store(1, 474)
        .network("Up to 15 Gigabit")
        .bandwidth_weightings(CONFIGURABLE_BANDWIDTH_WEIGHTINGS)
        .processor("AWS", 2.8)
        .ena_srd(false)
        .host_features(false, true)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0)
        .flexible_ena_queues(false)
        .traffic_mirroring(false),
    PublishedInstanceTypeFacts::of("c8gd.4xlarge")
        .ebs(5_000, 20_000, 10_000, 40_000)
        .dedicated_ebs_attachments(32)
        .instance_store(1, 950)
        .network("Up to 15 Gigabit")
        .bandwidth_weightings(CONFIGURABLE_BANDWIDTH_WEIGHTINGS)
        .processor("AWS", 2.8)
        .ena_srd(false)
        .host_features(false, true)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0)
        .flexible_ena_queues(false)
        .traffic_mirroring(false),
    PublishedInstanceTypeFacts::of("c8gd.8xlarge")
        .ebs(10_000, 40_000, 10_000, 40_000)
        .dedicated_ebs_attachments(32)
        .instance_store(1, 1_900)
        .network("15 Gigabit")
        .bandwidth_weightings(CONFIGURABLE_BANDWIDTH_WEIGHTINGS)
        .processor("AWS", 2.8)
        .host_features(false, true)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0)
        .flexible_ena_queues(false)
        .traffic_mirroring(false),
    PublishedInstanceTypeFacts::of("c8gd.12xlarge")
        .ebs(15_000, 60_000, 15_000, 60_000)
        .dedicated_ebs_attachments(32)
        .instance_store(3, 950)
        .network("22.5 Gigabit")
        .bandwidth_weightings(CONFIGURABLE_BANDWIDTH_WEIGHTINGS)
        .processor("AWS", 2.8)
        .host_features(false, true)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0)
        .flexible_ena_queues(false)
        .traffic_mirroring(false),
    PublishedInstanceTypeFacts::of("c8gd.16xlarge")
        .ebs(20_000, 80_000, 20_000, 80_000)
        .dedicated_ebs_attachments(48)
        .instance_store(2, 1_900)
        .network("30 Gigabit")
        .bandwidth_weightings(CONFIGURABLE_BANDWIDTH_WEIGHTINGS)
        .processor("AWS", 2.8)
        .host_features(false, true)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0)
        .flexible_ena_queues(false)
        .traffic_mirroring(false),
    PublishedInstanceTypeFacts::of("c8gd.24xlarge")
        .ebs(30_000, 120_000, 30_000, 120_000)
        .dedicated_ebs_attachments(64)
        .instance_store(3, 1_900)
        .network("40 Gigabit")
        .bandwidth_weightings(CONFIGURABLE_BANDWIDTH_WEIGHTINGS)
        .efa(1)
        .processor("AWS", 2.8)
        .host_features(false, true)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0)
        .flexible_ena_queues(false)
        .traffic_mirroring(false),
    PublishedInstanceTypeFacts::of("c8gd.48xlarge")
        .cores_in_steps_of(192)
        .ebs(40_000, 240_000, 40_000, 240_000)
        .dedicated_ebs_attachments(128)
        .instance_store(6, 1_900)
        .network("50 Gigabit")
        .bandwidth_weightings(CONFIGURABLE_BANDWIDTH_WEIGHTINGS)
        .efa(1)
        .processor("AWS", 2.7)
        .host_features(false, true)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0)
        .flexible_ena_queues(false)
        .traffic_mirroring(false),
    PublishedInstanceTypeFacts::of("c8gd.metal-24xl")
        .ebs(30_000, 120_000, 30_000, 120_000)
        .dedicated_ebs_attachments(39)
        .instance_store(3, 1_900)
        .network("40 Gigabit")
        .bandwidth_weightings(CONFIGURABLE_BANDWIDTH_WEIGHTINGS)
        .efa(1)
        .processor("AWS", 2.8)
        .ena_srd(true)
        .host_features(false, true)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0)
        .flexible_ena_queues(false)
        .traffic_mirroring(false),
    PublishedInstanceTypeFacts::of("c8gd.metal-48xl")
        .ebs(40_000, 240_000, 40_000, 240_000)
        .dedicated_ebs_attachments(79)
        .instance_store(6, 1_900)
        .network("50 Gigabit")
        .bandwidth_weightings(CONFIGURABLE_BANDWIDTH_WEIGHTINGS)
        .efa(1)
        .processor("AWS", 2.7)
        .ena_srd(true)
        .host_features(false, true)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0)
        .flexible_ena_queues(false)
        .traffic_mirroring(false),
    // C5a — compute optimized on the AMD EPYC 7R32 at a sustained 3.3 GHz, with no local
    // instance store. The family's EBS bandwidth doubles up the ladder and is DEDICATED from
    // the 8xlarge up, the four sizes below that one bursting to the 8xlarge's figure; the
    // `4xlarge` publishes HALF the dedicated figures of that size, bandwidth and IOPS alike,
    // rather than the four IOPS per Mbps the three smaller sizes carry. Its EBS
    // volumes come out of the shared Nitro attachment pool, which the derivation above already
    // counts, so these rows leave that limit unset. The family is offered on NO Dedicated Host
    // and supports no On-Demand hibernation. Its three narrowest sizes publish every core count
    // up to their own, while each size from the `4xlarge` up takes its cores ALL at once — so
    // the one count such a size publishes is its default.
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
        .cores_all_at_once()
        .ebs(1_580, 6_600, 3_170, 13_300)
        .network("Up to 10 Gigabit")
        .processor("AMD", 3.3)
        .dedicated_hosts(false)
        .hibernation(false),
    PublishedInstanceTypeFacts::of("c5a.8xlarge")
        .cores_all_at_once()
        .ebs(3_170, 13_300, 3_170, 13_300)
        .network("10 Gigabit")
        .processor("AMD", 3.3)
        .dedicated_hosts(false)
        .hibernation(false),
    PublishedInstanceTypeFacts::of("c5a.12xlarge")
        .cores_all_at_once()
        .ebs(4_750, 20_000, 4_750, 20_000)
        .network("12 Gigabit")
        .processor("AMD", 3.3)
        .dedicated_hosts(false)
        .hibernation(false),
    PublishedInstanceTypeFacts::of("c5a.16xlarge")
        .cores_all_at_once()
        .ebs(6_300, 26_700, 6_300, 26_700)
        .network("20 Gigabit")
        .processor("AMD", 3.3)
        .dedicated_hosts(false)
        .hibernation(false),
    PublishedInstanceTypeFacts::of("c5a.24xlarge")
        .cores_all_at_once()
        .ebs(9_500, 40_000, 9_500, 40_000)
        .network("20 Gigabit")
        .processor("AMD", 3.3)
        .dedicated_hosts(false)
        .hibernation(false),
    // C5n — the C5 family's hardware built for the NETWORK, on the same Intel Xeon Platinum
    // 8000 series (Skylake) processors at a sustained 3.4 GHz and with no local instance
    // store at any size. Two columns are the family's own rather than the C5 ladder's: its
    // memory, 5.25 GiB per two vCPUs (which is not a whole number of GiB per vCPU, so the
    // smaller sizes are stated in MiB), and its network performance — the four smallest sizes
    // burst up to 25 Gigabit, the `9xlarge` carries half of the host's 100 and the two
    // full-size rows carry all of it. Its EBS figures are the family's own rows of "Amazon
    // EBS optimized instance types": the three smallest sizes publish a baseline they burst
    // above, up to 4,750 Mbps and 20,000 IOPS, and the bandwidth is dedicated from the
    // `4xlarge` up. The Elastic Fabric Adapter is offered from the `9xlarge` up ("Elastic
    // Fabric Adapter"), which is below the size an adapter is first offered at elsewhere in
    // the catalog. The hibernation prerequisites do not name this family, so NO size of it
    // hibernates. The published core counts go up in twos from the `xlarge` and start at four
    // on the `18xlarge`, whose published list begins above that step, while the bare metal
    // size publishes every count; that size's EBS volumes are attached beside the shared
    // pool rather than out of it, so it holds the 31 volumes "Instance volume limits"
    // publishes for a bare metal instance, it is offered the legacy BIOS boot mode alone, it
    // supports CloudWatch action based recovery — which the sizes with no instance store
    // already do — and it supports reboot migration, the move to another host being the
    // HOST's to make and not the hypervisor's that a bare metal instance has none of.
    PublishedInstanceTypeFacts::of("c5n.large")
        .memory_mib(5_376)
        .ebs(650, 4_000, 4_750, 20_000)
        .network("Up to 25 Gigabit")
        .processor("Intel", 3.4)
        .hibernation(false),
    PublishedInstanceTypeFacts::of("c5n.xlarge")
        .memory_mib(10_752)
        .cores_in_steps_of(2)
        .ebs(1_150, 6_000, 4_750, 20_000)
        .network("Up to 25 Gigabit")
        .processor("Intel", 3.4)
        .hibernation(false),
    PublishedInstanceTypeFacts::of("c5n.2xlarge")
        .memory_mib(21_504)
        .cores_in_steps_of(2)
        .ebs(2_300, 10_000, 4_750, 20_000)
        .network("Up to 25 Gigabit")
        .processor("Intel", 3.4)
        .hibernation(false),
    PublishedInstanceTypeFacts::of("c5n.4xlarge")
        .memory_mib(43_008)
        .cores_in_steps_of(2)
        .ebs(4_750, 20_000, 4_750, 20_000)
        .network("Up to 25 Gigabit")
        .processor("Intel", 3.4)
        .hibernation(false),
    PublishedInstanceTypeFacts::of("c5n.9xlarge")
        .memory_gib(96)
        .cores_in_steps_of(2)
        .ebs(9_500, 40_000, 9_500, 40_000)
        .network("50 Gigabit")
        .efa(1)
        .processor("Intel", 3.4)
        .hibernation(false),
    PublishedInstanceTypeFacts::of("c5n.18xlarge")
        .memory_gib(192)
        .cores_in_steps_of(2)
        .cores_from(4)
        .ebs(19_000, 80_000, 19_000, 80_000)
        .network("100 Gigabit")
        .efa(1)
        .processor("Intel", 3.4)
        .hibernation(false),
    PublishedInstanceTypeFacts::of("c5n.metal")
        .memory_gib(192)
        .ebs(19_000, 80_000, 19_000, 80_000)
        .ebs_attachments(31)
        .network("100 Gigabit")
        .efa(1)
        .processor("Intel", 3.4)
        .auto_recovery(true)
        .host_features(true, false)
        .boot_modes(&["legacy-bios"]),
    // C6a — compute optimized, 3rd generation AMD EPYC 7R13 (Milan) at a sustained 3.6 GHz,
    // with no local instance store. Its EBS figures are its own rows of "Amazon EBS optimized
    // instance types": the four sizes below the `8xlarge` publish a baseline they burst above,
    // up to 10,000 Mbps and 40,000 IOPS, and the bandwidth is dedicated from that size up,
    // reaching 40,000 Mbps on the `32xlarge` and 240,000 IOPS on the sizes that hold the whole
    // host. The network figures are the family's own ladder — a quarter of the host's 50 Gigabit
    // bursting to 12.5 below the `8xlarge`, then its share of the host — and its volumes come
    // out of the attachment pool shared with its interfaces, so the virtualized sizes leave the
    // limit the derivation above counts; the bare metal size takes the 31 EBS volumes "Instance
    // volume limits" publishes for a bare metal instance and IS offered CloudWatch action based
    // recovery, which the hypervisor-backed rule would deny it. The Elastic Fabric Adapter is
    // offered on the largest virtualized size and the bare metal one alone. Hibernation is
    // published for the sizes up to the `8xlarge` and for none above it, and ENA Express is
    // published the other way round — the sizes up to the `8xlarge` carry less dedicated network
    // bandwidth than the Scalable Reliable Datagram transport needs, so it is offered from the
    // `12xlarge` up and on the bare metal size. That bare metal size is also offered reboot
    // migration, and it boots the legacy BIOS alone: UEFI reaches every x86 size built on the
    // Nitro System EXCEPT a bare metal one, which has no hypervisor to present it.
    PublishedInstanceTypeFacts::of("c6a.large")
        .ebs(650, 3_600, 10_000, 40_000)
        .network("Up to 12.5 Gigabit")
        .processor("AMD", 3.6)
        .ena_srd(false),
    PublishedInstanceTypeFacts::of("c6a.xlarge")
        .ebs(1_250, 6_000, 10_000, 40_000)
        .network("Up to 12.5 Gigabit")
        .processor("AMD", 3.6)
        .ena_srd(false),
    PublishedInstanceTypeFacts::of("c6a.2xlarge")
        .ebs(2_500, 12_000, 10_000, 40_000)
        .network("Up to 12.5 Gigabit")
        .processor("AMD", 3.6)
        .ena_srd(false),
    PublishedInstanceTypeFacts::of("c6a.4xlarge")
        .ebs(5_000, 20_000, 10_000, 40_000)
        .network("Up to 12.5 Gigabit")
        .processor("AMD", 3.6)
        .ena_srd(false),
    PublishedInstanceTypeFacts::of("c6a.8xlarge")
        .ebs(10_000, 40_000, 10_000, 40_000)
        .network("12.5 Gigabit")
        .processor("AMD", 3.6)
        .ena_srd(false),
    PublishedInstanceTypeFacts::of("c6a.12xlarge")
        .ebs(15_000, 60_000, 15_000, 60_000)
        .network("18.75 Gigabit")
        .processor("AMD", 3.6)
        .hibernation(false),
    PublishedInstanceTypeFacts::of("c6a.16xlarge")
        .ebs(20_000, 80_000, 20_000, 80_000)
        .network("25 Gigabit")
        .processor("AMD", 3.6)
        .hibernation(false),
    PublishedInstanceTypeFacts::of("c6a.24xlarge")
        .ebs(30_000, 120_000, 30_000, 120_000)
        .network("37.5 Gigabit")
        .processor("AMD", 3.6)
        .hibernation(false),
    PublishedInstanceTypeFacts::of("c6a.32xlarge")
        .ebs(40_000, 160_000, 40_000, 160_000)
        .network("50 Gigabit")
        .processor("AMD", 3.6)
        .hibernation(false),
    PublishedInstanceTypeFacts::of("c6a.48xlarge")
        .ebs(40_000, 240_000, 40_000, 240_000)
        .network("50 Gigabit")
        .efa(1)
        .processor("AMD", 3.6)
        .hibernation(false),
    PublishedInstanceTypeFacts::of("c6a.metal")
        .ebs(40_000, 240_000, 40_000, 240_000)
        .ebs_attachments(31)
        .network("50 Gigabit")
        .efa(1)
        .processor("AMD", 3.6)
        .ena_srd(true)
        .hibernation(false)
        .auto_recovery(true)
        .host_features(true, false)
        .boot_modes(&["legacy-bios"]),
    // C6g — compute optimized on AWS Graviton2, at a sustained 2.5 GHz, with no local instance
    // store. Its EBS figures are the Graviton2 compute ladder's rows of "Amazon EBS optimized
    // instance types": the sizes below the `4xlarge` publish a baseline they burst above, up to
    // 4,750 Mbps and 20,000 IOPS, and the bandwidth is dedicated from that size up. The network
    // figures are the family's published ones — up to 10 Gigabit below the `8xlarge`, then 12,
    // 20 and the host's 25 — and its smallest size takes two interfaces of four private
    // addresses each rather than the count its step of the size ladder publishes. The volumes
    // come out of the shared attachment pool, so the virtualized sizes leave the limit the
    // derivation above counts, while the bare metal size takes the 31 EBS volumes published for
    // a bare metal instance and IS offered CloudWatch action based recovery. ENA Express is not
    // offered on this hardware generation — the adapter that carries it arrives with the
    // network optimized sibling — and the Elastic Fabric Adapter is offered on no size. An
    // enclave is offered from two vCPUs up on this architecture rather than from the four an
    // x86 type needs, so the `large` size carries one; the bare metal size is offered reboot
    // migration and copies a session's packets for Traffic Mirroring, while it carries no local
    // PTP hardware clock.
    PublishedInstanceTypeFacts::of("c6g.medium")
        .ebs(315, 2_500, 4_750, 20_000)
        .network("Up to 10 Gigabit")
        .interfaces(2)
        .ipv4_per_interface(4)
        .processor("AWS", GRAVITON2_SUSTAINED_GHZ)
        .ena_srd(false),
    PublishedInstanceTypeFacts::of("c6g.large")
        .ebs(630, 3_600, 4_750, 20_000)
        .network("Up to 10 Gigabit")
        .processor("AWS", GRAVITON2_SUSTAINED_GHZ)
        .ena_srd(false)
        .nitro_enclaves(true),
    PublishedInstanceTypeFacts::of("c6g.xlarge")
        .ebs(1_188, 6_000, 4_750, 20_000)
        .network("Up to 10 Gigabit")
        .processor("AWS", GRAVITON2_SUSTAINED_GHZ)
        .ena_srd(false),
    PublishedInstanceTypeFacts::of("c6g.2xlarge")
        .ebs(2_375, 12_000, 4_750, 20_000)
        .network("Up to 10 Gigabit")
        .processor("AWS", GRAVITON2_SUSTAINED_GHZ)
        .ena_srd(false),
    PublishedInstanceTypeFacts::of("c6g.4xlarge")
        .ebs(4_750, 20_000, 4_750, 20_000)
        .network("Up to 10 Gigabit")
        .processor("AWS", GRAVITON2_SUSTAINED_GHZ)
        .ena_srd(false),
    PublishedInstanceTypeFacts::of("c6g.8xlarge")
        .ebs(9_500, 40_000, 9_500, 40_000)
        .network("12 Gigabit")
        .processor("AWS", GRAVITON2_SUSTAINED_GHZ)
        .ena_srd(false),
    PublishedInstanceTypeFacts::of("c6g.12xlarge")
        .ebs(14_250, 50_000, 14_250, 50_000)
        .network("20 Gigabit")
        .processor("AWS", GRAVITON2_SUSTAINED_GHZ)
        .ena_srd(false),
    PublishedInstanceTypeFacts::of("c6g.16xlarge")
        .ebs(19_000, 80_000, 19_000, 80_000)
        .network("25 Gigabit")
        .processor("AWS", GRAVITON2_SUSTAINED_GHZ)
        .ena_srd(false),
    PublishedInstanceTypeFacts::of("c6g.metal")
        .ebs(19_000, 80_000, 19_000, 80_000)
        .ebs_attachments(31)
        .network("25 Gigabit")
        .processor("AWS", GRAVITON2_SUSTAINED_GHZ)
        .ena_srd(false)
        .auto_recovery(true)
        .host_features(true, false)
        .traffic_mirroring(true),
    // C6i — compute optimized, 3rd generation Intel Xeon Scalable (Ice Lake) at a sustained
    // 3.5 GHz, with no local instance store. The family sits on the same 128-vCPU host as its
    // general purpose sibling, so its EBS figures are that host's rows of "Amazon EBS
    // optimized instance types": the sizes below the `8xlarge` publish a baseline they burst
    // above, up to 10,000 Mbps and 40,000 IOPS, and the bandwidth is dedicated from that size
    // up, reaching the host's 40,000 Mbps and 160,000 IOPS on the sizes that hold the whole
    // host. The network figures are the family's published ones — up to 12.5 Gigabit below the
    // `8xlarge`, then 12.5, 18.75, 25, 37.5 and the host's 50 — the adapter takes a
    // configurable number of ENA queues on every virtualized size, ENA Express is offered from
    // the size that carries the family's whole dedicated share up, and the Elastic Fabric
    // Adapter only on the two sizes that hold the whole host. The published core counts go up
    // in twos from the `2xlarge`, whose cores sit on both of the host's processor sockets,
    // while the bare metal size publishes every count. That bare metal size is offered the
    // legacy BIOS boot mode alone, supports CloudWatch action based recovery and reboot
    // migration — the move to another host being the HOST's to make and not the hypervisor's it
    // has none of — and takes the 31 EBS volumes "Instance volume limits" publishes for a bare
    // metal instance, its local devices being the host's own hardware rather than attachments.
    PublishedInstanceTypeFacts::of("c6i.large")
        .ebs(650, 3_600, 10_000, 40_000)
        .network("Up to 12.5 Gigabit")
        .flexible_ena_queues(true)
        .efa(0)
        .ena_srd(false)
        .processor("Intel", 3.5),
    PublishedInstanceTypeFacts::of("c6i.xlarge")
        .ebs(1_250, 6_000, 10_000, 40_000)
        .network("Up to 12.5 Gigabit")
        .flexible_ena_queues(true)
        .efa(0)
        .ena_srd(false)
        .processor("Intel", 3.5),
    PublishedInstanceTypeFacts::of("c6i.2xlarge")
        .cores_in_steps_of(2)
        .ebs(2_500, 12_000, 10_000, 40_000)
        .network("Up to 12.5 Gigabit")
        .flexible_ena_queues(true)
        .efa(0)
        .ena_srd(false)
        .processor("Intel", 3.5),
    PublishedInstanceTypeFacts::of("c6i.4xlarge")
        .cores_in_steps_of(2)
        .ebs(5_000, 20_000, 10_000, 40_000)
        .network("Up to 12.5 Gigabit")
        .flexible_ena_queues(true)
        .efa(0)
        .ena_srd(false)
        .processor("Intel", 3.5),
    PublishedInstanceTypeFacts::of("c6i.8xlarge")
        .cores_in_steps_of(2)
        .ebs(10_000, 40_000, 10_000, 40_000)
        .network("12.5 Gigabit")
        .flexible_ena_queues(true)
        .efa(0)
        .processor("Intel", 3.5),
    PublishedInstanceTypeFacts::of("c6i.12xlarge")
        .cores_in_steps_of(2)
        .ebs(15_000, 60_000, 15_000, 60_000)
        .network("18.75 Gigabit")
        .flexible_ena_queues(true)
        .efa(0)
        .processor("Intel", 3.5),
    PublishedInstanceTypeFacts::of("c6i.16xlarge")
        .cores_in_steps_of(2)
        .ebs(20_000, 80_000, 20_000, 80_000)
        .network("25 Gigabit")
        .flexible_ena_queues(true)
        .efa(0)
        .processor("Intel", 3.5),
    PublishedInstanceTypeFacts::of("c6i.24xlarge")
        .cores_in_steps_of(2)
        .ebs(30_000, 120_000, 30_000, 120_000)
        .network("37.5 Gigabit")
        .flexible_ena_queues(true)
        .efa(0)
        .processor("Intel", 3.5),
    PublishedInstanceTypeFacts::of("c6i.32xlarge")
        .cores_in_steps_of(2)
        .ebs(40_000, 160_000, 40_000, 160_000)
        .network("50 Gigabit")
        .flexible_ena_queues(true)
        .efa(1)
        .processor("Intel", 3.5),
    PublishedInstanceTypeFacts::of("c6i.metal")
        .ebs(40_000, 160_000, 40_000, 160_000)
        .ebs_attachments(31)
        .network("50 Gigabit")
        .efa(1)
        .ena_srd(true)
        .processor("Intel", 3.5)
        .auto_recovery(true)
        .host_features(true, false)
        .boot_modes(&["legacy-bios"]),
    // C6id — compute optimized with local NVMe, on the same 128-vCPU 3rd generation Intel
    // Xeon Scalable (Ice Lake) host as `c6i` and at the same sustained 3.5 GHz, so its four
    // EBS figures are that host's rows of "Amazon EBS optimized instance types": a baseline
    // the sizes below the `8xlarge` burst above, up to 10,000 Mbps and 40,000 IOPS, then
    // dedicated bandwidth from that size up to the host's 40,000 Mbps and 160,000 IOPS. Its
    // local store is the family's published NVMe ladder — one device per two vCPUs up to the
    // `8xlarge`, then two and four devices of the published sizes — which is NOT a smooth
    // function of the size: the `2xlarge` carries 474 GB and the `12xlarge` and `24xlarge`
    // carry 1,425 GB devices rather than the quarters and halves of a 1,900 GB device the
    // ladder would otherwise give. The network figures are the family's published ones, the
    // adapter takes a configurable number of ENA queues on every virtualized size, ENA Express
    // is offered from the size that carries the family's whole dedicated share up and on the
    // bare metal size, and the Elastic Fabric Adapter only on the size that holds the whole
    // host and the bare metal one. Those local devices are what a reboot migration cannot
    // carry over, so the family is not offered it, and it carries no local PTP hardware clock,
    // which its hardware generation has none of either. The published core
    // counts go up in twos from the `2xlarge` while the bare metal size publishes every
    // count; that size is offered the legacy BIOS boot mode alone and takes the 31 EBS
    // volumes "Instance volume limits" publishes for a bare metal instance, its local devices
    // being the host's own hardware rather than attachments.
    PublishedInstanceTypeFacts::of("c6id.large")
        .ebs(650, 3_600, 10_000, 40_000)
        .instance_store(1, 118)
        .network("Up to 12.5 Gigabit")
        .flexible_ena_queues(true)
        .efa(0)
        .ena_srd(false)
        .processor("Intel", 3.5)
        .host_features(false, false),
    PublishedInstanceTypeFacts::of("c6id.xlarge")
        .ebs(1_250, 6_000, 10_000, 40_000)
        .instance_store(1, 237)
        .network("Up to 12.5 Gigabit")
        .flexible_ena_queues(true)
        .efa(0)
        .ena_srd(false)
        .processor("Intel", 3.5)
        .host_features(false, false),
    PublishedInstanceTypeFacts::of("c6id.2xlarge")
        .cores_in_steps_of(2)
        .ebs(2_500, 12_000, 10_000, 40_000)
        .instance_store(1, 474)
        .network("Up to 12.5 Gigabit")
        .flexible_ena_queues(true)
        .efa(0)
        .ena_srd(false)
        .processor("Intel", 3.5)
        .host_features(false, false),
    PublishedInstanceTypeFacts::of("c6id.4xlarge")
        .cores_in_steps_of(2)
        .ebs(5_000, 20_000, 10_000, 40_000)
        .instance_store(1, 950)
        .network("Up to 12.5 Gigabit")
        .flexible_ena_queues(true)
        .efa(0)
        .ena_srd(false)
        .processor("Intel", 3.5)
        .host_features(false, false),
    PublishedInstanceTypeFacts::of("c6id.8xlarge")
        .cores_in_steps_of(2)
        .ebs(10_000, 40_000, 10_000, 40_000)
        .instance_store(1, 1_900)
        .network("12.5 Gigabit")
        .flexible_ena_queues(true)
        .efa(0)
        .processor("Intel", 3.5)
        .host_features(false, false),
    PublishedInstanceTypeFacts::of("c6id.12xlarge")
        .cores_in_steps_of(2)
        .ebs(15_000, 60_000, 15_000, 60_000)
        .instance_store(2, 1_425)
        .network("18.75 Gigabit")
        .flexible_ena_queues(true)
        .efa(0)
        .processor("Intel", 3.5)
        .host_features(false, false),
    PublishedInstanceTypeFacts::of("c6id.16xlarge")
        .cores_in_steps_of(2)
        .ebs(20_000, 80_000, 20_000, 80_000)
        .instance_store(2, 1_900)
        .network("25 Gigabit")
        .flexible_ena_queues(true)
        .efa(0)
        .processor("Intel", 3.5)
        .host_features(false, false),
    PublishedInstanceTypeFacts::of("c6id.24xlarge")
        .cores_in_steps_of(2)
        .ebs(30_000, 120_000, 30_000, 120_000)
        .instance_store(4, 1_425)
        .network("37.5 Gigabit")
        .flexible_ena_queues(true)
        .efa(0)
        .processor("Intel", 3.5)
        .host_features(false, false),
    PublishedInstanceTypeFacts::of("c6id.32xlarge")
        .cores_in_steps_of(2)
        .ebs(40_000, 160_000, 40_000, 160_000)
        .instance_store(4, 1_900)
        .network("50 Gigabit")
        .flexible_ena_queues(true)
        .efa(1)
        .processor("Intel", 3.5)
        .host_features(false, false),
    PublishedInstanceTypeFacts::of("c6id.metal")
        .ebs(40_000, 160_000, 40_000, 160_000)
        .ebs_attachments(31)
        .instance_store(4, 1_900)
        .network("50 Gigabit")
        .efa(1)
        .ena_srd(true)
        .processor("Intel", 3.5)
        .boot_modes(&["legacy-bios"]),
    // C6in — compute optimized built for the network, on the same Ice Lake host at the same
    // sustained 3.5 GHz and with no local instance store. The family carries up to 200 Gigabit
    // of network and 100 Gbps of EBS bandwidth on its largest size, each size taking its
    // published share of both, so its EBS figures are the `n` ladder's rows of "Amazon EBS
    // optimized instance types": a baseline the sizes below the `8xlarge` burst above, up to
    // 25,000 Mbps and 100,000 IOPS, then dedicated bandwidth from that size up to 100,000 Mbps
    // and 400,000 IOPS. The adapter takes a configurable number of ENA queues, ENA Express is
    // offered from the size that carries the family's whole dedicated share up, and the
    // Elastic Fabric Adapter only on the two sizes that hold the whole host — which carry TWO
    // cards and so two Elastic Fabric Adapters, one per card. The family takes no interface
    // attachment from a secondary network, so no address at all is available on one, and
    // carries no local PTP hardware clock. Its two largest sizes publish core counts that go
    // up in twos from four rather than every count up to their default, the sizes below them
    // publishing every count. Its bare metal size is offered the legacy BIOS boot
    // mode alone, supports CloudWatch action based recovery and reboot migration — having no
    // local device whose data either would have to carry over, and the move to another host
    // being the HOST's to make and not the hypervisor's it has none of — and takes the 31 EBS
    // volumes published for a bare metal instance.
    PublishedInstanceTypeFacts::of("c6in.large")
        // The smallest size's published share of its host is 1,562.5 Mbps — 3,125 halves of
        // an Mbps — which the integer bandwidth member reports as 1,562 while the throughput
        // member carries the megabytes of the whole figure.
        .ebs_in_half_mbps(3_125, 6_250, 25_000, 100_000)
        .network("Up to 25 Gigabit")
        .flexible_ena_queues(true)
        .efa(0)
        .ena_srd(false)
        .processor("Intel", 3.5)
        .ipv4_per_secondary_interface(0),
    PublishedInstanceTypeFacts::of("c6in.xlarge")
        .ebs(3_125, 12_500, 25_000, 100_000)
        .network("Up to 30 Gigabit")
        .flexible_ena_queues(true)
        .efa(0)
        .ena_srd(false)
        .processor("Intel", 3.5)
        .ipv4_per_secondary_interface(0),
    PublishedInstanceTypeFacts::of("c6in.2xlarge")
        .ebs(6_250, 25_000, 25_000, 100_000)
        .network("Up to 40 Gigabit")
        .flexible_ena_queues(true)
        .efa(0)
        .ena_srd(false)
        .processor("Intel", 3.5)
        .ipv4_per_secondary_interface(0),
    PublishedInstanceTypeFacts::of("c6in.4xlarge")
        .ebs(12_500, 50_000, 25_000, 100_000)
        .network("Up to 50 Gigabit")
        .flexible_ena_queues(true)
        .efa(0)
        .ena_srd(false)
        .processor("Intel", 3.5)
        .ipv4_per_secondary_interface(0),
    PublishedInstanceTypeFacts::of("c6in.8xlarge")
        .ebs(25_000, 100_000, 25_000, 100_000)
        .network("50 Gigabit")
        .flexible_ena_queues(true)
        .efa(0)
        .processor("Intel", 3.5)
        .ipv4_per_secondary_interface(0),
    PublishedInstanceTypeFacts::of("c6in.12xlarge")
        .ebs(37_500, 150_000, 37_500, 150_000)
        .network("75 Gigabit")
        .flexible_ena_queues(true)
        .efa(0)
        .processor("Intel", 3.5)
        .ipv4_per_secondary_interface(0),
    PublishedInstanceTypeFacts::of("c6in.16xlarge")
        .ebs(50_000, 200_000, 50_000, 200_000)
        .network("100 Gigabit")
        .flexible_ena_queues(true)
        .efa(0)
        .processor("Intel", 3.5)
        .ipv4_per_secondary_interface(0),
    PublishedInstanceTypeFacts::of("c6in.24xlarge")
        .cores_in_steps_of(2)
        .cores_from(4)
        .ebs(75_000, 300_000, 75_000, 300_000)
        .network("150 Gigabit")
        .flexible_ena_queues(true)
        .efa(0)
        .processor("Intel", 3.5)
        .ipv4_per_secondary_interface(0),
    PublishedInstanceTypeFacts::of("c6in.32xlarge")
        .cores_in_steps_of(2)
        .cores_from(4)
        .ebs(100_000, 400_000, 100_000, 400_000)
        .network("200 Gigabit")
        .flexible_ena_queues(true)
        .efa(2)
        .processor("Intel", 3.5)
        .ipv4_per_secondary_interface(0),
    PublishedInstanceTypeFacts::of("c6in.metal")
        .ebs(100_000, 400_000, 100_000, 400_000)
        .ebs_attachments(31)
        .network("200 Gigabit")
        .efa(2)
        .ena_srd(true)
        .processor("Intel", 3.5)
        .auto_recovery(true)
        .host_features(true, false)
        .ipv4_per_secondary_interface(0)
        .boot_modes(&["legacy-bios"]),
    // C7g — compute optimized on AWS Graviton3, at a sustained 2.6 GHz, with no local
    // instance store. Its EBS figures are the Graviton3 compute ladder's rows of "Amazon EBS
    // optimized instance types": the sizes below the `8xlarge` publish a baseline they burst
    // above, up to 10,000 Mbps and 40,000 IOPS, and the bandwidth is dedicated from that size
    // up, reaching the host's 20,000 Mbps and 80,000 IOPS on the sizes that hold it whole. The
    // network figures are the family's published ones — up to 12.5 Gigabit at the three
    // smallest sizes and up to 15 at the two above them, then the 15, 22.5 and 30 Gigabit the
    // dedicated sizes carry — ENA Express is offered on the two sizes above the one that
    // carries the family's whole dedicated share and on the bare metal size, and the Elastic
    // Fabric Adapter on the two sizes that hold the whole host. Its smallest size takes two
    // interfaces of four private addresses each rather than the count its step of the size
    // ladder publishes, and an enclave is offered from two vCPUs up on this architecture rather
    // than from the four an x86 type needs, so the `large` size carries one. The hardware
    // generation takes no configurable ENA queue count and no interface attachment from a
    // secondary network — so no address at all is available on one — and carries no local PTP
    // hardware clock, while reboot migration IS offered, on the bare metal size too. The host
    // carries one Graviton3 socket whose cores are configured one at a time, which is what the
    // derivation counts, so no row publishes a step; the bare metal size takes the 31 EBS
    // volumes published for a bare metal instance and supports CloudWatch action based
    // recovery.
    PublishedInstanceTypeFacts::of("c7g.medium")
        .ebs(315, 2_500, 10_000, 40_000)
        .network("Up to 12.5 Gigabit")
        .interfaces(2)
        .ipv4_per_interface(4)
        .efa(0)
        .ena_srd(false)
        .flexible_ena_queues(false)
        .processor("AWS", 2.6)
        .host_features(true, false)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0),
    PublishedInstanceTypeFacts::of("c7g.large")
        .ebs(630, 3_600, 10_000, 40_000)
        .network("Up to 12.5 Gigabit")
        .efa(0)
        .ena_srd(false)
        .flexible_ena_queues(false)
        .nitro_enclaves(true)
        .processor("AWS", 2.6)
        .host_features(true, false)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0),
    PublishedInstanceTypeFacts::of("c7g.xlarge")
        .ebs(1_250, 6_000, 10_000, 40_000)
        .network("Up to 12.5 Gigabit")
        .efa(0)
        .ena_srd(false)
        .flexible_ena_queues(false)
        .processor("AWS", 2.6)
        .host_features(true, false)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0),
    PublishedInstanceTypeFacts::of("c7g.2xlarge")
        .ebs(2_500, 12_000, 10_000, 40_000)
        .network("Up to 15 Gigabit")
        .efa(0)
        .ena_srd(false)
        .flexible_ena_queues(false)
        .processor("AWS", 2.6)
        .host_features(true, false)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0),
    PublishedInstanceTypeFacts::of("c7g.4xlarge")
        .ebs(5_000, 20_000, 10_000, 40_000)
        .network("Up to 15 Gigabit")
        .efa(0)
        .ena_srd(false)
        .flexible_ena_queues(false)
        .processor("AWS", 2.6)
        .host_features(true, false)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0),
    PublishedInstanceTypeFacts::of("c7g.8xlarge")
        .ebs(10_000, 40_000, 10_000, 40_000)
        .network("15 Gigabit")
        .efa(0)
        .ena_srd(false)
        .flexible_ena_queues(false)
        .processor("AWS", 2.6)
        .host_features(true, false)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0),
    PublishedInstanceTypeFacts::of("c7g.12xlarge")
        .ebs(15_000, 60_000, 15_000, 60_000)
        .network("22.5 Gigabit")
        .efa(0)
        .flexible_ena_queues(false)
        .processor("AWS", 2.6)
        .host_features(true, false)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0),
    PublishedInstanceTypeFacts::of("c7g.16xlarge")
        .ebs(20_000, 80_000, 20_000, 80_000)
        .network("30 Gigabit")
        .efa(1)
        .flexible_ena_queues(false)
        .processor("AWS", 2.6)
        .host_features(true, false)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0),
    PublishedInstanceTypeFacts::of("c7g.metal")
        .ebs(20_000, 80_000, 20_000, 80_000)
        .ebs_attachments(31)
        .network("30 Gigabit")
        .efa(1)
        .ena_srd(true)
        .processor("AWS", 2.6)
        .auto_recovery(true)
        .host_features(true, false)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0),
    // C7gd — the C7g host with local NVMe SSD. It is the SAME Graviton3 hardware as the family
    // above, so it publishes that family's rows of "Amazon EBS optimized instance types" and
    // its network figures unchanged — a baseline the sizes below the `8xlarge` burst above, up
    // to 10,000 Mbps and 40,000 IOPS, dedicated bandwidth from that size up, and up to 12.5
    // Gigabit of network at the three smallest sizes, up to 15 at the two above them, then the
    // 15, 22.5 and 30 Gigabit of the dedicated sizes. What the `d` adds is the local store, and
    // its device SIZE is what the family's specification publishes per size while the device
    // COUNT only grows at the two largest sizes: the figures below are that published column,
    // not a multiple of the size in the type's name. The devices come out of the attachment
    // pool the type shares with its interfaces and its EBS volumes, so the EBS limit each size
    // reports is what that pool leaves — except on the bare metal size, which publishes the 31
    // volumes a bare metal instance takes. The rest of the family's capabilities are the C7g
    // ones: the smallest size takes two interfaces of four private addresses each, an enclave is
    // offered from the `large` size up on this architecture, the Elastic Fabric Adapter only on
    // the two sizes that hold the whole host, ENA Express on the two sizes above the one that
    // carries the family's whole dedicated share and on the bare metal size, and the hardware
    // generation takes no configurable ENA queue count and no interface attachment from a
    // secondary network, with no local PTP hardware clock while reboot migration IS offered.
    PublishedInstanceTypeFacts::of("c7gd.medium")
        .ebs(315, 2_500, 10_000, 40_000)
        .instance_store(1, 59)
        .network("Up to 12.5 Gigabit")
        .interfaces(2)
        .ipv4_per_interface(4)
        .efa(0)
        .ena_srd(false)
        .flexible_ena_queues(false)
        .processor("AWS", 2.6)
        .host_features(true, false)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0),
    PublishedInstanceTypeFacts::of("c7gd.large")
        .ebs(630, 3_600, 10_000, 40_000)
        .instance_store(1, 118)
        .network("Up to 12.5 Gigabit")
        .efa(0)
        .ena_srd(false)
        .flexible_ena_queues(false)
        .nitro_enclaves(true)
        .processor("AWS", 2.6)
        .host_features(true, false)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0),
    PublishedInstanceTypeFacts::of("c7gd.xlarge")
        .ebs(1_250, 6_000, 10_000, 40_000)
        .instance_store(1, 237)
        .network("Up to 12.5 Gigabit")
        .efa(0)
        .ena_srd(false)
        .flexible_ena_queues(false)
        .processor("AWS", 2.6)
        .host_features(true, false)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0),
    PublishedInstanceTypeFacts::of("c7gd.2xlarge")
        .ebs(2_500, 12_000, 10_000, 40_000)
        .instance_store(1, 475)
        .network("Up to 15 Gigabit")
        .efa(0)
        .ena_srd(false)
        .flexible_ena_queues(false)
        .processor("AWS", 2.6)
        .host_features(true, false)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0),
    PublishedInstanceTypeFacts::of("c7gd.4xlarge")
        .ebs(5_000, 20_000, 10_000, 40_000)
        .instance_store(1, 950)
        .network("Up to 15 Gigabit")
        .efa(0)
        .ena_srd(false)
        .flexible_ena_queues(false)
        .processor("AWS", 2.6)
        .host_features(true, false)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0),
    PublishedInstanceTypeFacts::of("c7gd.8xlarge")
        .ebs(10_000, 40_000, 10_000, 40_000)
        .instance_store(1, 1_900)
        .network("15 Gigabit")
        .efa(0)
        .ena_srd(false)
        .flexible_ena_queues(false)
        .processor("AWS", 2.6)
        .host_features(true, false)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0),
    PublishedInstanceTypeFacts::of("c7gd.12xlarge")
        .ebs(15_000, 60_000, 15_000, 60_000)
        .instance_store(2, 1_425)
        .network("22.5 Gigabit")
        .efa(0)
        .flexible_ena_queues(false)
        .processor("AWS", 2.6)
        .host_features(true, false)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0),
    PublishedInstanceTypeFacts::of("c7gd.16xlarge")
        .ebs(20_000, 80_000, 20_000, 80_000)
        .instance_store(2, 1_900)
        .network("30 Gigabit")
        .efa(1)
        .flexible_ena_queues(false)
        .processor("AWS", 2.6)
        .host_features(true, false)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0),
    PublishedInstanceTypeFacts::of("c7gd.metal")
        .ebs(20_000, 80_000, 20_000, 80_000)
        .ebs_attachments(31)
        .instance_store(2, 1_900)
        .network("30 Gigabit")
        .efa(1)
        .ena_srd(true)
        .processor("AWS", 2.6)
        .auto_recovery(true)
        .host_features(true, false)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0),
    // C7gn — compute optimized built for the NETWORK, on AWS Graviton3E at a sustained 2.6 GHz
    // and with no local instance store. Its published network column is the family's own ladder —
    // up to 25, 30, 40 and 50 Gigabit on the four smallest sizes, then the share of the host's
    // 200 the dedicated sizes carry (50, 100, 150 and 200) — and its EBS column is the family's
    // rows of "Amazon EBS optimized instance types": a baseline that is the size's share of the
    // host's sustained 33,333 Mbps and 133,333 IOPS, each share published rounded to the whole
    // figure nearest it, and a maximum that is the size's own ceiling, 10,000 Mbps and 40,000
    // IOPS up to the `4xlarge` and the share of the host's 40,000 Mbps and 160,000 IOPS above it.
    // So no size of this family sustains its ceiling: the largest ones publish 33,333 Mbps of
    // baseline under a 40,000 Mbps maximum. The adapter of this family takes no configurable
    // number of ENA queues and no interface attachment from a secondary network — so no private
    // address at all is available on one — the packets of a session cannot be copied under its
    // guest, it carries no local PTP hardware clock, and no size is offered Nitro Enclaves;
    // ENA Express is offered from the `4xlarge`, the first size carrying a dedicated network
    // share, up, and on the bare metal size. Reboot migration IS offered, on the bare metal size
    // too, and that size is offered CloudWatch action based recovery and takes the 31 EBS volumes
    // "Instance volume limits" publishes for a bare metal instance, while the trusted platform
    // module its row publishes none of is the one the published list of the types offered NitroTPM
    // leaves every bare metal instance out of — the virtualized sizes of the family are offered
    // one. The smallest size takes two
    // interfaces of four private addresses each rather than the count its step of the size ladder
    // publishes.
    PublishedInstanceTypeFacts::of("c7gn.medium")
        .ebs(521, 2_083, 10_000, 40_000)
        .network("Up to 25 Gigabit")
        .interfaces(2)
        .ipv4_per_interface(4)
        .ena_srd(false)
        .flexible_ena_queues(false)
        .nitro_enclaves(false)
        .processor("AWS", 2.6)
        .host_features(true, false)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0)
        .traffic_mirroring(false),
    PublishedInstanceTypeFacts::of("c7gn.large")
        .ebs(1_042, 4_167, 10_000, 40_000)
        .network("Up to 30 Gigabit")
        .ena_srd(false)
        .flexible_ena_queues(false)
        .nitro_enclaves(false)
        .processor("AWS", 2.6)
        .host_features(true, false)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0)
        .traffic_mirroring(false),
    PublishedInstanceTypeFacts::of("c7gn.xlarge")
        .ebs(2_083, 8_333, 10_000, 40_000)
        .network("Up to 40 Gigabit")
        .ena_srd(false)
        .flexible_ena_queues(false)
        .nitro_enclaves(false)
        .processor("AWS", 2.6)
        .host_features(true, false)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0)
        .traffic_mirroring(false),
    PublishedInstanceTypeFacts::of("c7gn.2xlarge")
        .ebs(4_167, 16_667, 10_000, 40_000)
        .network("Up to 50 Gigabit")
        .ena_srd(false)
        .flexible_ena_queues(false)
        .nitro_enclaves(false)
        .processor("AWS", 2.6)
        .host_features(true, false)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0)
        .traffic_mirroring(false),
    PublishedInstanceTypeFacts::of("c7gn.4xlarge")
        .ebs(8_333, 33_333, 10_000, 40_000)
        .network("50 Gigabit")
        .flexible_ena_queues(false)
        .nitro_enclaves(false)
        .processor("AWS", 2.6)
        .host_features(true, false)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0)
        .traffic_mirroring(false),
    PublishedInstanceTypeFacts::of("c7gn.8xlarge")
        .ebs(16_667, 66_667, 20_000, 80_000)
        .network("100 Gigabit")
        .flexible_ena_queues(false)
        .nitro_enclaves(false)
        .processor("AWS", 2.6)
        .host_features(true, false)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0)
        .traffic_mirroring(false),
    PublishedInstanceTypeFacts::of("c7gn.12xlarge")
        .ebs(25_000, 100_000, 30_000, 120_000)
        .network("150 Gigabit")
        .flexible_ena_queues(false)
        .nitro_enclaves(false)
        .processor("AWS", 2.6)
        .host_features(true, false)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0)
        .traffic_mirroring(false),
    PublishedInstanceTypeFacts::of("c7gn.16xlarge")
        .ebs(33_333, 133_333, 40_000, 160_000)
        .network("200 Gigabit")
        .flexible_ena_queues(false)
        .nitro_enclaves(false)
        .processor("AWS", 2.6)
        .host_features(true, false)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0)
        .traffic_mirroring(false),
    PublishedInstanceTypeFacts::of("c7gn.metal")
        .ebs(33_333, 133_333, 40_000, 160_000)
        .ebs_attachments(31)
        .network("200 Gigabit")
        .ena_srd(true)
        .flexible_ena_queues(false)
        .nitro_enclaves(false)
        .nitro_tpm(false)
        .processor("AWS", 2.6)
        .auto_recovery(true)
        .host_features(true, false)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0)
        .traffic_mirroring(false),
    // C7i — compute optimized, 4th generation Intel Xeon Scalable (Sapphire Rapids) at a
    // sustained 3.2 GHz, with no local instance store. It sits on the same 192-vCPU host as its
    // general purpose sibling M7i, so its EBS figures are that host's rows of "Amazon EBS
    // optimized instance types" — the sizes below the `8xlarge` publish a baseline they burst
    // above, up to 10,000 Mbps and 40,000 IOPS, and the bandwidth is dedicated from that size up,
    // reaching 40,000 Mbps and 240,000 IOPS on the sizes that hold the whole host — and its
    // network figures are that host's published ones, up to 12.5 Gigabit below the `8xlarge` and
    // the size's share of the host's 50 from there up. Its EBS volumes are attached to EBS
    // hardware of the type's own rather than out of the pool shared with its interfaces, so every
    // size reports the `dedicated` kind of limit: 32 volumes up to the `12xlarge`, then 48, 64
    // and 128 as the size grows, while each bare metal size reports its host's own — 79 on the
    // whole host and 39 on the half of it the smaller one holds. Its processor runs a hypervisor
    // inside the guest, so every virtualized size publishes nested virtualization as a processor
    // feature, which a bare metal size — running the caller's software with no hypervisor under
    // it — does not. The number of ENA queues cannot be changed on its adapter and it takes no
    // interface attachment from a secondary network, so no private address at all is available on
    // one; ENA Express is offered from the `12xlarge` up, the sizes under it carrying less than
    // the family's dedicated network share. Dedicated Hosts are offered except on the sizes that
    // hold most of the host — the `24xlarge`, the `48xlarge` and the smaller bare metal size —
    // the Elastic Fabric Adapter only on the `48xlarge` and the bare metal size that holds the
    // whole host, and the `48xlarge` takes its cores all at once, so the one count it publishes
    // is its default. Both bare metal sizes are offered CloudWatch action based recovery, reboot
    // migration, a local PTP hardware clock, ENA Express and the legacy BIOS boot mode alone, and
    // publish NO trusted platform module: the published list of the instance types offered
    // NitroTPM leaves every bare metal instance out of it, while the virtualized sizes of this
    // family are offered one.
    PublishedInstanceTypeFacts::of("c7i.large")
        .ebs(650, 3_600, 10_000, 40_000)
        .dedicated_ebs_attachments(32)
        .network("Up to 12.5 Gigabit")
        .processor("Intel", 3.2)
        .processor_features(NESTED_VIRTUALIZATION)
        .ena_srd(false)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0)
        .flexible_ena_queues(false),
    PublishedInstanceTypeFacts::of("c7i.xlarge")
        .ebs(1_250, 6_000, 10_000, 40_000)
        .dedicated_ebs_attachments(32)
        .network("Up to 12.5 Gigabit")
        .processor("Intel", 3.2)
        .processor_features(NESTED_VIRTUALIZATION)
        .ena_srd(false)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0)
        .flexible_ena_queues(false),
    PublishedInstanceTypeFacts::of("c7i.2xlarge")
        .ebs(2_500, 12_000, 10_000, 40_000)
        .dedicated_ebs_attachments(32)
        .network("Up to 12.5 Gigabit")
        .processor("Intel", 3.2)
        .processor_features(NESTED_VIRTUALIZATION)
        .ena_srd(false)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0)
        .flexible_ena_queues(false),
    PublishedInstanceTypeFacts::of("c7i.4xlarge")
        .ebs(5_000, 20_000, 10_000, 40_000)
        .dedicated_ebs_attachments(32)
        .network("Up to 12.5 Gigabit")
        .processor("Intel", 3.2)
        .processor_features(NESTED_VIRTUALIZATION)
        .ena_srd(false)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0)
        .flexible_ena_queues(false),
    PublishedInstanceTypeFacts::of("c7i.8xlarge")
        .ebs(10_000, 40_000, 10_000, 40_000)
        .dedicated_ebs_attachments(32)
        .network("12.5 Gigabit")
        .processor("Intel", 3.2)
        .processor_features(NESTED_VIRTUALIZATION)
        .ena_srd(false)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0)
        .flexible_ena_queues(false),
    PublishedInstanceTypeFacts::of("c7i.12xlarge")
        .ebs(15_000, 60_000, 15_000, 60_000)
        .dedicated_ebs_attachments(32)
        .network("18.75 Gigabit")
        .processor("Intel", 3.2)
        .processor_features(NESTED_VIRTUALIZATION)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0)
        .flexible_ena_queues(false),
    PublishedInstanceTypeFacts::of("c7i.16xlarge")
        .ebs(20_000, 80_000, 20_000, 80_000)
        .dedicated_ebs_attachments(48)
        .network("25 Gigabit")
        .processor("Intel", 3.2)
        .processor_features(NESTED_VIRTUALIZATION)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0)
        .flexible_ena_queues(false),
    PublishedInstanceTypeFacts::of("c7i.24xlarge")
        .ebs(30_000, 120_000, 30_000, 120_000)
        .dedicated_ebs_attachments(64)
        .network("37.5 Gigabit")
        .processor("Intel", 3.2)
        .processor_features(NESTED_VIRTUALIZATION)
        .dedicated_hosts(false)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0)
        .flexible_ena_queues(false),
    PublishedInstanceTypeFacts::of("c7i.48xlarge")
        .cores_in_steps_of(96)
        .ebs(40_000, 240_000, 40_000, 240_000)
        .dedicated_ebs_attachments(128)
        .network("50 Gigabit")
        .efa(1)
        .processor("Intel", 3.2)
        .processor_features(NESTED_VIRTUALIZATION)
        .dedicated_hosts(false)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0)
        .flexible_ena_queues(false),
    PublishedInstanceTypeFacts::of("c7i.metal-24xl")
        .ebs(30_000, 120_000, 30_000, 120_000)
        .dedicated_ebs_attachments(39)
        .network("37.5 Gigabit")
        .processor("Intel", 3.2)
        .ena_srd(true)
        .nitro_tpm(false)
        .dedicated_hosts(false)
        .auto_recovery(true)
        .boot_modes(LEGACY_BIOS_BOOT_MODE)
        .host_features(true, true)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0)
        .flexible_ena_queues(false),
    PublishedInstanceTypeFacts::of("c7i.metal-48xl")
        .ebs(40_000, 240_000, 40_000, 240_000)
        .dedicated_ebs_attachments(79)
        .network("50 Gigabit")
        .efa(1)
        .processor("Intel", 3.2)
        .ena_srd(true)
        .nitro_tpm(false)
        .auto_recovery(true)
        .boot_modes(LEGACY_BIOS_BOOT_MODE)
        .host_features(true, true)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0)
        .flexible_ena_queues(false),
    // R9g — memory optimized on the newest AWS Graviton hardware, with no local instance store.
    // The family's network and EBS bandwidth is CONFIGURABLE, so it publishes the bandwidth
    // weightings a launch may ask for, which the families whose split is fixed publish none of;
    // its EBS volumes are attached to EBS hardware of the type's own rather than out of the pool
    // shared with its interfaces, so every size reports a limit of the `dedicated` kind, and the
    // figure that limit carries is the 32 volumes published from the smallest size up to the
    // `12xlarge` — above that the family's published figure is not recorded here yet, so those
    // rows carry the KIND of the limit without a figure; and its connection tracking applies the
    // generation's
    // own, much shorter established-TCP idle timeout. It takes no interface attachment from a
    // secondary network — so no address at all is available on one — and its guest's packets
    // cannot be copied under a mirroring session. The Elastic Fabric Adapter is offered on
    // the two sizes that hold the whole host, each publishing the 16 adapters of this
    // generation. The remaining figures of the family are not recorded here yet, so each row
    // carries only what IS published for it and the derivation answers the rest.
    PublishedInstanceTypeFacts::of("r9g.medium")
        .dedicated_ebs_attachments(32)
        .bandwidth_weightings(CONFIGURABLE_BANDWIDTH_WEIGHTINGS)
        .tcp_established_timeout(SHORT_TCP_ESTABLISHED_TIMEOUT)
        .traffic_mirroring(false)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0),
    PublishedInstanceTypeFacts::of("r9g.large")
        .dedicated_ebs_attachments(32)
        .bandwidth_weightings(CONFIGURABLE_BANDWIDTH_WEIGHTINGS)
        .tcp_established_timeout(SHORT_TCP_ESTABLISHED_TIMEOUT)
        .traffic_mirroring(false)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0),
    PublishedInstanceTypeFacts::of("r9g.xlarge")
        .dedicated_ebs_attachments(32)
        .bandwidth_weightings(CONFIGURABLE_BANDWIDTH_WEIGHTINGS)
        .tcp_established_timeout(SHORT_TCP_ESTABLISHED_TIMEOUT)
        .traffic_mirroring(false)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0),
    PublishedInstanceTypeFacts::of("r9g.2xlarge")
        .dedicated_ebs_attachments(32)
        .bandwidth_weightings(CONFIGURABLE_BANDWIDTH_WEIGHTINGS)
        .tcp_established_timeout(SHORT_TCP_ESTABLISHED_TIMEOUT)
        .traffic_mirroring(false)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0),
    PublishedInstanceTypeFacts::of("r9g.4xlarge")
        .dedicated_ebs_attachments(32)
        .bandwidth_weightings(CONFIGURABLE_BANDWIDTH_WEIGHTINGS)
        .tcp_established_timeout(SHORT_TCP_ESTABLISHED_TIMEOUT)
        .traffic_mirroring(false)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0),
    PublishedInstanceTypeFacts::of("r9g.8xlarge")
        .dedicated_ebs_attachments(32)
        .bandwidth_weightings(CONFIGURABLE_BANDWIDTH_WEIGHTINGS)
        .tcp_established_timeout(SHORT_TCP_ESTABLISHED_TIMEOUT)
        .traffic_mirroring(false)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0),
    PublishedInstanceTypeFacts::of("r9g.12xlarge")
        .dedicated_ebs_attachments(32)
        .bandwidth_weightings(CONFIGURABLE_BANDWIDTH_WEIGHTINGS)
        .tcp_established_timeout(SHORT_TCP_ESTABLISHED_TIMEOUT)
        .traffic_mirroring(false)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0),
    PublishedInstanceTypeFacts::of("r9g.16xlarge")
        .dedicated_attachment_limit()
        .bandwidth_weightings(CONFIGURABLE_BANDWIDTH_WEIGHTINGS)
        .tcp_established_timeout(SHORT_TCP_ESTABLISHED_TIMEOUT)
        .traffic_mirroring(false)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0),
    PublishedInstanceTypeFacts::of("r9g.24xlarge")
        .dedicated_attachment_limit()
        .bandwidth_weightings(CONFIGURABLE_BANDWIDTH_WEIGHTINGS)
        .tcp_established_timeout(SHORT_TCP_ESTABLISHED_TIMEOUT)
        .traffic_mirroring(false)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0),
    PublishedInstanceTypeFacts::of("r9g.48xlarge")
        .dedicated_attachment_limit()
        .bandwidth_weightings(CONFIGURABLE_BANDWIDTH_WEIGHTINGS)
        .tcp_established_timeout(SHORT_TCP_ESTABLISHED_TIMEOUT)
        .traffic_mirroring(false)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0)
        .efa(NEWEST_GENERATION_EFA_INTERFACES),
    PublishedInstanceTypeFacts::of("r9g.metal-48xl")
        .dedicated_attachment_limit()
        .bandwidth_weightings(CONFIGURABLE_BANDWIDTH_WEIGHTINGS)
        .tcp_established_timeout(SHORT_TCP_ESTABLISHED_TIMEOUT)
        .traffic_mirroring(false)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0)
        .efa(NEWEST_GENERATION_EFA_INTERFACES),
    // C9gd — compute optimized with local NVMe on the newest AWS Graviton hardware, the
    // hardware generation its sibling family above sits on, so what that generation publishes
    // it publishes at every size: the bandwidth weightings a launch may ask for, which the
    // families whose network and EBS split is fixed publish none of; the shorter idle timeout
    // its connection tracking holds an established TCP connection for; no interface attachment
    // from a secondary network, so no private address at all is available on one; and no
    // copying of a session's packets under its guest. Its EBS volumes are attached to EBS
    // hardware of the type's own rather than out of the pool shared with its interfaces and
    // instance store devices, so a virtualized size reports the `dedicated` kind of limit, and
    // so does the BARE METAL size, which holds the 79 attachments of a whole host of this
    // generation. The Elastic Fabric Adapter is offered on the two sizes that hold the whole
    // host. That bare metal size's published hardware is recorded below, the host's whole 100
    // Gigabit of network among it; the network figures of the other sizes are not recorded yet,
    // so each of those rows carries only what IS published for it and the derivation answers
    // the rest. The two sizes that hold a WHOLE host — the widest virtualized one and the bare
    // metal one — are the same hardware, one host and one network card, so they publish one set
    // of network figures: the host's whole 100 Gigabit, both sustained and reached, its
    // interfaces and the addresses each of them takes, and the queues its adapter gives an
    // interface.
    PublishedInstanceTypeFacts::of("c9gd.medium")
        .dedicated_attachment_limit()
        .bandwidth_weightings(CONFIGURABLE_BANDWIDTH_WEIGHTINGS)
        .tcp_established_timeout(SHORT_TCP_ESTABLISHED_TIMEOUT)
        .traffic_mirroring(false)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0),
    PublishedInstanceTypeFacts::of("c9gd.large")
        .dedicated_attachment_limit()
        .bandwidth_weightings(CONFIGURABLE_BANDWIDTH_WEIGHTINGS)
        .tcp_established_timeout(SHORT_TCP_ESTABLISHED_TIMEOUT)
        .traffic_mirroring(false)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0),
    PublishedInstanceTypeFacts::of("c9gd.xlarge")
        .dedicated_attachment_limit()
        .bandwidth_weightings(CONFIGURABLE_BANDWIDTH_WEIGHTINGS)
        .tcp_established_timeout(SHORT_TCP_ESTABLISHED_TIMEOUT)
        .traffic_mirroring(false)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0),
    PublishedInstanceTypeFacts::of("c9gd.2xlarge")
        .dedicated_attachment_limit()
        .bandwidth_weightings(CONFIGURABLE_BANDWIDTH_WEIGHTINGS)
        .tcp_established_timeout(SHORT_TCP_ESTABLISHED_TIMEOUT)
        .traffic_mirroring(false)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0),
    PublishedInstanceTypeFacts::of("c9gd.4xlarge")
        .dedicated_attachment_limit()
        .bandwidth_weightings(CONFIGURABLE_BANDWIDTH_WEIGHTINGS)
        .tcp_established_timeout(SHORT_TCP_ESTABLISHED_TIMEOUT)
        .traffic_mirroring(false)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0),
    PublishedInstanceTypeFacts::of("c9gd.8xlarge")
        .dedicated_attachment_limit()
        .bandwidth_weightings(CONFIGURABLE_BANDWIDTH_WEIGHTINGS)
        .tcp_established_timeout(SHORT_TCP_ESTABLISHED_TIMEOUT)
        .traffic_mirroring(false)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0),
    PublishedInstanceTypeFacts::of("c9gd.12xlarge")
        .dedicated_attachment_limit()
        .bandwidth_weightings(CONFIGURABLE_BANDWIDTH_WEIGHTINGS)
        .tcp_established_timeout(SHORT_TCP_ESTABLISHED_TIMEOUT)
        .traffic_mirroring(false)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0),
    PublishedInstanceTypeFacts::of("c9gd.16xlarge")
        .dedicated_attachment_limit()
        .bandwidth_weightings(CONFIGURABLE_BANDWIDTH_WEIGHTINGS)
        .tcp_established_timeout(SHORT_TCP_ESTABLISHED_TIMEOUT)
        .traffic_mirroring(false)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0),
    PublishedInstanceTypeFacts::of("c9gd.24xlarge")
        .dedicated_attachment_limit()
        .bandwidth_weightings(CONFIGURABLE_BANDWIDTH_WEIGHTINGS)
        .tcp_established_timeout(SHORT_TCP_ESTABLISHED_TIMEOUT)
        .traffic_mirroring(false)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0),
    PublishedInstanceTypeFacts::of("c9gd.48xlarge")
        .dedicated_attachment_limit()
        .network_bandwidth(
            NEWEST_GENERATION_GRAVITON_HOST_GBPS,
            NEWEST_GENERATION_GRAVITON_HOST_GBPS,
        )
        .interfaces(WHOLE_HOST_NETWORK_INTERFACES)
        .ipv4_per_interface(WHOLE_HOST_IPV4_PER_INTERFACE)
        .ena_queues(NEWEST_GENERATION_GRAVITON_ENA_QUEUES)
        .ena_queue_limits(
            WHOLE_HOST_ENA_QUEUES_PER_CARD,
            WHOLE_HOST_ENA_QUEUES_PER_INTERFACE,
        )
        .additional_flexible_interfaces(NO_ADDITIONAL_FLEXIBLE_INTERFACES)
        .bandwidth_weightings(CONFIGURABLE_BANDWIDTH_WEIGHTINGS)
        .tcp_established_timeout(SHORT_TCP_ESTABLISHED_TIMEOUT)
        .traffic_mirroring(false)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0)
        .efa(1),
    PublishedInstanceTypeFacts::of("c9gd.metal-48xl")
        .ebs(72_000, 288_000, 72_000, 288_000)
        .dedicated_ebs_attachments(79)
        .instance_store(3, 3_800)
        .network_bandwidth(
            NEWEST_GENERATION_GRAVITON_HOST_GBPS,
            NEWEST_GENERATION_GRAVITON_HOST_GBPS,
        )
        .interfaces(WHOLE_HOST_NETWORK_INTERFACES)
        .ipv4_per_interface(WHOLE_HOST_IPV4_PER_INTERFACE)
        .ena_queues(NEWEST_GENERATION_GRAVITON_ENA_QUEUES)
        .ena_queue_limits(
            WHOLE_HOST_ENA_QUEUES_PER_CARD,
            WHOLE_HOST_ENA_QUEUES_PER_INTERFACE,
        )
        .additional_flexible_interfaces(NO_ADDITIONAL_FLEXIBLE_INTERFACES)
        .flexible_ena_queues(true)
        .bandwidth_weightings(CONFIGURABLE_BANDWIDTH_WEIGHTINGS)
        .tcp_established_timeout(SHORT_TCP_ESTABLISHED_TIMEOUT)
        .traffic_mirroring(false)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0)
        .efa(1)
        .auto_recovery(true)
        .ena_srd(true)
        .nitro_tpm(true)
        .host_features(true, true),
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
    // its 48 cores sit on two sockets, so they are configured one per socket. It takes no
    // interface attachment from a secondary network, so it publishes none of the addresses
    // such an interface would take.
    PublishedInstanceTypeFacts::of("dl2q.24xlarge")
        .memory_gib(768)
        .cores_in_steps_of(2)
        .ebs(19_000, 80_000, 19_000, 80_000)
        .ebs_attachments(19)
        .no_instance_store()
        .network("100 Gigabit")
        .processor("Intel", 3.5)
        .encryption_in_transit(true)
        .ipv4_per_secondary_interface(0)
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
    // I4i — storage optimized, 3rd generation Intel Xeon Scalable (Ice Lake) at a sustained
    // 3.5 GHz, carrying the AWS Nitro SSD devices the family is built around: the share of the
    // host's eight 3,750 GB devices each size holds, which is one device of a size of its own up
    // to the `4xlarge` and whole 3,750 GB devices above it. Its EBS figures are the family's own
    // rows of "Amazon EBS optimized instance types": the sizes below the `8xlarge` publish a
    // baseline they burst above, up to 10,000 Mbps and 40,000 IOPS, and the bandwidth is
    // dedicated from that size up, reaching 40,000 Mbps and 160,000 IOPS on the sizes that hold
    // the whole host. The EBS volumes share the Nitro attachment pool with the local devices and
    // the interfaces, so the limit each virtualized size reports is what that pool leaves once
    // its devices are counted — which the derivation above already does, so those rows leave it
    // unset — while the bare metal size holds 31 volumes, its local devices being the host's own
    // hardware. The network figures are the published ones: up to 10, 12.5 and 25 Gigabit below
    // the `8xlarge`, then the size's share of the host's 75. Neither reboot migration nor a PTP
    // hardware clock is offered on this hardware generation; ENA Express is offered from the
    // `8xlarge` up, the sizes under it carrying less than the family's dedicated network share,
    // and on the bare metal size; Dedicated Hosts are offered except
    // on the two sizes added later, the `12xlarge` and the `24xlarge`; the Elastic Fabric Adapter
    // is offered on the size that holds the whole host and on the bare metal one; and the sizes
    // whose cores sit on two processor sockets publish their core counts one per socket. The
    // bare metal size is offered the legacy BIOS boot mode alone.
    PublishedInstanceTypeFacts::of("i4i.large")
        .ebs(625, 2_500, 10_000, 40_000)
        .instance_store(1, 468)
        .network("Up to 10 Gigabit")
        .processor("Intel", 3.5)
        .host_features(false, false),
    PublishedInstanceTypeFacts::of("i4i.xlarge")
        .ebs(1_250, 5_000, 10_000, 40_000)
        .instance_store(1, 937)
        .network("Up to 10 Gigabit")
        .processor("Intel", 3.5)
        .host_features(false, false),
    PublishedInstanceTypeFacts::of("i4i.2xlarge")
        .ebs(2_500, 10_000, 10_000, 40_000)
        .instance_store(1, 1_875)
        .network("Up to 12 Gigabit")
        .processor("Intel", 3.5)
        .host_features(false, false),
    PublishedInstanceTypeFacts::of("i4i.4xlarge")
        .ebs(5_000, 20_000, 10_000, 40_000)
        .instance_store(1, 3_750)
        .network("Up to 25 Gigabit")
        .processor("Intel", 3.5)
        .host_features(false, false),
    PublishedInstanceTypeFacts::of("i4i.8xlarge")
        .ebs(10_000, 40_000, 10_000, 40_000)
        .instance_store(2, 3_750)
        .network("18.75 Gigabit")
        .processor("Intel", 3.5)
        .ena_srd(true)
        .host_features(false, false),
    PublishedInstanceTypeFacts::of("i4i.12xlarge")
        .ebs(15_000, 60_000, 15_000, 60_000)
        .instance_store(3, 3_750)
        .network("28.12 Gigabit")
        .processor("Intel", 3.5)
        .ena_srd(true)
        .dedicated_hosts(false)
        .host_features(false, false),
    PublishedInstanceTypeFacts::of("i4i.16xlarge")
        .cores_in_steps_of(2)
        .ebs(20_000, 80_000, 20_000, 80_000)
        .instance_store(4, 3_750)
        .network("37.5 Gigabit")
        .processor("Intel", 3.5)
        .ena_srd(true)
        .host_features(false, false),
    PublishedInstanceTypeFacts::of("i4i.24xlarge")
        .cores_in_steps_of(2)
        .ebs(30_000, 120_000, 30_000, 120_000)
        .instance_store(6, 3_750)
        .network("56.25 Gigabit")
        .ipv4_per_interface(30)
        .processor("Intel", 3.5)
        .ena_srd(true)
        .dedicated_hosts(false)
        .host_features(false, false),
    PublishedInstanceTypeFacts::of("i4i.32xlarge")
        .cores_in_steps_of(2)
        .ebs(40_000, 160_000, 40_000, 160_000)
        .instance_store(8, 3_750)
        .network("75 Gigabit")
        .efa(1)
        .processor("Intel", 3.5)
        .ena_srd(true)
        .host_features(false, false),
    PublishedInstanceTypeFacts::of("i4i.metal")
        .ebs(40_000, 160_000, 40_000, 160_000)
        .ebs_attachments(31)
        .instance_store(8, 3_750)
        .network("75 Gigabit")
        .efa(1)
        .processor("Intel", 3.5)
        .ena_srd(true)
        .boot_modes(LEGACY_BIOS_BOOT_MODE)
        .host_features(false, false),
    // I4g — storage optimized on AWS Graviton2 at a sustained 2.5 GHz, carrying the same AWS
    // Nitro SSD devices as the I4i family on a 64-vCPU host: the share of the host's four
    // 3,750 GB devices each size holds, which is one device of a size of its own up to the
    // `4xlarge` and whole 3,750 GB devices above it. Its EBS figures are that store's rows of
    // "Amazon EBS optimized instance types" — the sizes below the `8xlarge` publish a baseline
    // they burst above, up to 10,000 Mbps and 40,000 IOPS, and the bandwidth is dedicated from
    // that size up, reaching 20,000 Mbps and 80,000 IOPS on the size that holds the whole host —
    // and its network figures are the family's published ones: up to 10, 12 and 25 Gigabit below
    // the `8xlarge`, then the size's share of the host's 37.5. The EBS volumes share the Nitro
    // attachment pool with the local devices and the interfaces, so the limit each size reports
    // is what that pool leaves once its devices are counted, which the derivation above already
    // does. Hibernation IS offered on the family, up to the memory the hibernation prerequisites
    // bound an instance at, so the sizes from the `8xlarge` up — which carry more — are not
    // offered it; neither reboot migration nor a PTP hardware clock is offered on this hardware
    // generation, the local devices being what a reboot migration could not carry over, and
    // ENA Express is offered from the `4xlarge` up, as is the Elastic Fabric Adapter on the size
    // that holds the whole host. An enclave is offered from two vCPUs up on this architecture
    // rather than from the four an x86 type needs, so the smallest size carries one. NitroTPM is
    // NOT offered on this hardware: the published list of the instance types that present that
    // virtual device leaves the family out, though its adapter DOES encrypt in-transit traffic,
    // which the published list of the families that do names it among.
    // No private address at all is available on an interface attached from a secondary network,
    // which the family takes none of.
    PublishedInstanceTypeFacts::of("i4g.large")
        .ebs(625, 2_500, 10_000, 40_000)
        .instance_store(1, 468)
        .network("Up to 10 Gigabit")
        .processor("AWS", GRAVITON2_SUSTAINED_GHZ)
        .hibernation(true)
        .nitro_enclaves(true)
        .nitro_tpm(false)
        .host_features(false, false)
        .ipv4_per_secondary_interface(0),
    PublishedInstanceTypeFacts::of("i4g.xlarge")
        .ebs(1_250, 5_000, 10_000, 40_000)
        .instance_store(1, 937)
        .network("Up to 10 Gigabit")
        .processor("AWS", GRAVITON2_SUSTAINED_GHZ)
        .hibernation(true)
        .nitro_tpm(false)
        .host_features(false, false)
        .ipv4_per_secondary_interface(0),
    PublishedInstanceTypeFacts::of("i4g.2xlarge")
        .ebs(2_500, 10_000, 10_000, 40_000)
        .instance_store(1, 1_875)
        .network("Up to 12 Gigabit")
        .processor("AWS", GRAVITON2_SUSTAINED_GHZ)
        .hibernation(true)
        .nitro_tpm(false)
        .host_features(false, false)
        .ipv4_per_secondary_interface(0),
    PublishedInstanceTypeFacts::of("i4g.4xlarge")
        .ebs(5_000, 20_000, 10_000, 40_000)
        .instance_store(1, 3_750)
        .network("Up to 25 Gigabit")
        .processor("AWS", GRAVITON2_SUSTAINED_GHZ)
        .ena_srd(true)
        .hibernation(true)
        .nitro_tpm(false)
        .host_features(false, false)
        .ipv4_per_secondary_interface(0),
    PublishedInstanceTypeFacts::of("i4g.8xlarge")
        .ebs(10_000, 40_000, 10_000, 40_000)
        .instance_store(2, 3_750)
        .network("18.75 Gigabit")
        .processor("AWS", GRAVITON2_SUSTAINED_GHZ)
        .ena_srd(true)
        .nitro_tpm(false)
        .host_features(false, false)
        .ipv4_per_secondary_interface(0),
    PublishedInstanceTypeFacts::of("i4g.16xlarge")
        .ebs(20_000, 80_000, 20_000, 80_000)
        .instance_store(4, 3_750)
        .network("37.5 Gigabit")
        .efa(1)
        .processor("AWS", GRAVITON2_SUSTAINED_GHZ)
        .ena_srd(true)
        .nitro_tpm(false)
        .host_features(false, false)
        .ipv4_per_secondary_interface(0),
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
    // M7i — general purpose, 4th generation Intel Xeon Scalable (Sapphire Rapids) at a sustained
    // 3.2 GHz, with no local instance store. Its processor runs a hypervisor inside the guest, so
    // every size publishes nested virtualization as a processor feature. Its EBS volumes are
    // attached to EBS hardware of the type's own rather than out of the pool shared with its
    // interfaces, so every size reports the `dedicated` kind of limit: 32 volumes up to the
    // `12xlarge`, then 48, 64 and 128 as the size grows, while each bare metal size reports its
    // host's own — 79 on the whole host and 39 on the half of it the smaller one holds. The EBS
    // figures are the family's own rows of "Amazon EBS optimized instance types": the sizes below
    // the `8xlarge` publish a baseline they burst above, up to 10,000 Mbps and 40,000 IOPS, and
    // the bandwidth is dedicated from that size up, reaching 40,000 Mbps and 240,000 IOPS on the
    // sizes that hold the whole host. The network figures are the published ones, up to 12.5
    // Gigabit below the `8xlarge` and the size's share of the host's 50 from there up. The split
    // between network and EBS bandwidth is fixed on this family, so it publishes no bandwidth
    // weightings at all; the number of ENA queues cannot be changed on its adapter; and it takes
    // no interface attachment from a secondary network, so no private address at all is available
    // on one. ENA Express is offered from the `12xlarge` up, the sizes under it carrying less
    // than the family's dedicated network share. Dedicated Hosts are offered except on the sizes
    // that
    // hold most of the host — the `24xlarge`, the `48xlarge` and the smaller bare metal size — the
    // Elastic Fabric Adapter only on the `48xlarge` and the bare metal size that holds the whole
    // host, and the `48xlarge` — the size that holds the whole host — takes its cores all at
    // once, so the one count it publishes is its default. Both bare metal sizes are
    // offered CloudWatch action based recovery, reboot migration, a local PTP hardware clock and
    // ENA Express, and the legacy BIOS boot mode alone.
    PublishedInstanceTypeFacts::of("m7i.large")
        .ebs(650, 3_600, 10_000, 40_000)
        .dedicated_ebs_attachments(32)
        .network("Up to 12.5 Gigabit")
        .processor("Intel", 3.2)
        .processor_features(NESTED_VIRTUALIZATION)
        .ena_srd(false)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0)
        .flexible_ena_queues(false),
    PublishedInstanceTypeFacts::of("m7i.xlarge")
        .ebs(1_250, 6_000, 10_000, 40_000)
        .dedicated_ebs_attachments(32)
        .network("Up to 12.5 Gigabit")
        .processor("Intel", 3.2)
        .processor_features(NESTED_VIRTUALIZATION)
        .ena_srd(false)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0)
        .flexible_ena_queues(false),
    PublishedInstanceTypeFacts::of("m7i.2xlarge")
        .ebs(2_500, 12_000, 10_000, 40_000)
        .dedicated_ebs_attachments(32)
        .network("Up to 12.5 Gigabit")
        .processor("Intel", 3.2)
        .processor_features(NESTED_VIRTUALIZATION)
        .ena_srd(false)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0)
        .flexible_ena_queues(false),
    PublishedInstanceTypeFacts::of("m7i.4xlarge")
        .ebs(5_000, 20_000, 10_000, 40_000)
        .dedicated_ebs_attachments(32)
        .network("Up to 12.5 Gigabit")
        .processor("Intel", 3.2)
        .processor_features(NESTED_VIRTUALIZATION)
        .ena_srd(false)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0)
        .flexible_ena_queues(false),
    PublishedInstanceTypeFacts::of("m7i.8xlarge")
        .ebs(10_000, 40_000, 10_000, 40_000)
        .dedicated_ebs_attachments(32)
        .network("12.5 Gigabit")
        .processor("Intel", 3.2)
        .processor_features(NESTED_VIRTUALIZATION)
        .ena_srd(false)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0)
        .flexible_ena_queues(false),
    PublishedInstanceTypeFacts::of("m7i.12xlarge")
        .ebs(15_000, 60_000, 15_000, 60_000)
        .dedicated_ebs_attachments(32)
        .network("18.75 Gigabit")
        .processor("Intel", 3.2)
        .processor_features(NESTED_VIRTUALIZATION)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0)
        .flexible_ena_queues(false),
    PublishedInstanceTypeFacts::of("m7i.16xlarge")
        .ebs(20_000, 80_000, 20_000, 80_000)
        .dedicated_ebs_attachments(48)
        .network("25 Gigabit")
        .processor("Intel", 3.2)
        .processor_features(NESTED_VIRTUALIZATION)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0)
        .flexible_ena_queues(false),
    PublishedInstanceTypeFacts::of("m7i.24xlarge")
        .ebs(30_000, 120_000, 30_000, 120_000)
        .dedicated_ebs_attachments(64)
        .network("37.5 Gigabit")
        .processor("Intel", 3.2)
        .processor_features(NESTED_VIRTUALIZATION)
        .dedicated_hosts(false)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0)
        .flexible_ena_queues(false),
    PublishedInstanceTypeFacts::of("m7i.48xlarge")
        .cores_in_steps_of(96)
        .ebs(40_000, 240_000, 40_000, 240_000)
        .dedicated_ebs_attachments(128)
        .network("50 Gigabit")
        .efa(1)
        .processor("Intel", 3.2)
        .processor_features(NESTED_VIRTUALIZATION)
        .dedicated_hosts(false)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0)
        .flexible_ena_queues(false),
    PublishedInstanceTypeFacts::of("m7i.metal-24xl")
        .ebs(30_000, 120_000, 30_000, 120_000)
        .dedicated_ebs_attachments(39)
        .network("37.5 Gigabit")
        .processor("Intel", 3.2)
        .processor_features(NESTED_VIRTUALIZATION)
        .ena_srd(true)
        .dedicated_hosts(false)
        .auto_recovery(true)
        .boot_modes(LEGACY_BIOS_BOOT_MODE)
        .host_features(true, true)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0)
        .flexible_ena_queues(false),
    PublishedInstanceTypeFacts::of("m7i.metal-48xl")
        .ebs(40_000, 240_000, 40_000, 240_000)
        .dedicated_ebs_attachments(79)
        .network("50 Gigabit")
        .efa(1)
        .processor("Intel", 3.2)
        .processor_features(NESTED_VIRTUALIZATION)
        .ena_srd(true)
        .auto_recovery(true)
        .boot_modes(LEGACY_BIOS_BOOT_MODE)
        .host_features(true, true)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0)
        .flexible_ena_queues(false),
    // M7gd — general purpose with local NVMe on AWS Graviton3, at a sustained 2.6 GHz. Its
    // four EBS figures are the Graviton3 ladder's rows of "Amazon EBS optimized instance
    // types": a baseline the sizes below the `8xlarge` burst above, up to 10,000 Mbps and
    // 40,000 IOPS, then dedicated bandwidth from that size up to the host's 20,000 Mbps and
    // 80,000 IOPS. Its local store is the family's published NVMe ladder — one device per two
    // vCPUs up to the `8xlarge`, then two of the published sizes — which is not a smooth
    // function of the size: the `2xlarge` carries a 474 GB device and the `12xlarge` two of
    // 1,425 GB rather than the quarter and the three quarters of a 1,900 GB device the ladder
    // would give. The network figures are the family's published ones, the Elastic Fabric
    // Adapter is offered on the size that holds the whole host and on the bare metal size, and
    // ENA Express only from the size above the one that carries the family's whole dedicated
    // share. The hardware generation takes no configurable ENA queue count and no interface
    // attachment from a secondary network — so no address at all is available on one — while a
    // local PTP hardware clock IS carried, on the bare metal size too, that clock being the
    // host's own hardware rather than something the hypervisor provides. The local devices are
    // what a reboot migration cannot carry over, so the family is not offered it. An enclave
    // is offered from two vCPUs
    // up on this architecture rather than from the four an x86 type needs, so the `large` size
    // carries one, and the smallest size takes two interfaces of four private addresses each
    // rather than the count its step of the size ladder publishes. The bare metal size takes
    // the 31 EBS volumes published for a bare metal instance.
    PublishedInstanceTypeFacts::of("m7gd.medium")
        .ebs(315, 2_500, 10_000, 40_000)
        .instance_store(1, 59)
        .network("Up to 12.5 Gigabit")
        .interfaces(2)
        .ipv4_per_interface(4)
        .efa(0)
        .ena_srd(false)
        .flexible_ena_queues(false)
        .processor("AWS", 2.6)
        .host_features(false, true)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0),
    PublishedInstanceTypeFacts::of("m7gd.large")
        .ebs(630, 3_600, 10_000, 40_000)
        .instance_store(1, 118)
        .network("Up to 12.5 Gigabit")
        .efa(0)
        .ena_srd(false)
        .flexible_ena_queues(false)
        .nitro_enclaves(true)
        .processor("AWS", 2.6)
        .host_features(false, true)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0),
    PublishedInstanceTypeFacts::of("m7gd.xlarge")
        .ebs(1_250, 6_000, 10_000, 40_000)
        .instance_store(1, 237)
        .network("Up to 12.5 Gigabit")
        .efa(0)
        .ena_srd(false)
        .flexible_ena_queues(false)
        .processor("AWS", 2.6)
        .host_features(false, true)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0),
    PublishedInstanceTypeFacts::of("m7gd.2xlarge")
        .ebs(2_500, 12_000, 10_000, 40_000)
        .instance_store(1, 474)
        .network("Up to 15 Gigabit")
        .efa(0)
        .ena_srd(false)
        .flexible_ena_queues(false)
        .processor("AWS", 2.6)
        .host_features(false, true)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0),
    PublishedInstanceTypeFacts::of("m7gd.4xlarge")
        .ebs(5_000, 20_000, 10_000, 40_000)
        .instance_store(1, 950)
        .network("Up to 15 Gigabit")
        .efa(0)
        .ena_srd(false)
        .flexible_ena_queues(false)
        .processor("AWS", 2.6)
        .host_features(false, true)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0),
    PublishedInstanceTypeFacts::of("m7gd.8xlarge")
        .ebs(10_000, 40_000, 10_000, 40_000)
        .instance_store(1, 1_900)
        .network("15 Gigabit")
        .efa(0)
        .ena_srd(false)
        .flexible_ena_queues(false)
        .processor("AWS", 2.6)
        .host_features(false, true)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0),
    PublishedInstanceTypeFacts::of("m7gd.12xlarge")
        .ebs(15_000, 60_000, 15_000, 60_000)
        .instance_store(2, 1_425)
        .network("22.5 Gigabit")
        .efa(0)
        .flexible_ena_queues(false)
        .processor("AWS", 2.6)
        .host_features(false, true)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0),
    PublishedInstanceTypeFacts::of("m7gd.16xlarge")
        .ebs(20_000, 80_000, 20_000, 80_000)
        .instance_store(2, 1_900)
        .network("30 Gigabit")
        .efa(1)
        .flexible_ena_queues(false)
        .processor("AWS", 2.6)
        .host_features(false, true)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0),
    PublishedInstanceTypeFacts::of("m7gd.metal")
        .ebs(20_000, 80_000, 20_000, 80_000)
        .ebs_attachments(31)
        .instance_store(2, 1_900)
        .network("30 Gigabit")
        .efa(1)
        .ena_srd(true)
        .processor("AWS", 2.6)
        .host_features(false, true)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0),
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
    // R3 — memory optimized of a PREVIOUS generation, Intel Xeon E5-2670 v2 (Ivy Bridge) at a
    // sustained 2.5 GHz on the Xen hypervisor, with local SATA SSD devices: one device of a size
    // of its own up to the `4xlarge` and two 320 GB devices on the size that holds the whole
    // host. Its memory is the family's own published figure at every size rather than a whole
    // number of GiB per vCPU. EBS optimization is an OPTION of the launch on the three middle
    // sizes, each of which publishes ONE fixed figure — a pre-Nitro instance has no baseline it
    // bursts above — and is not offered at all on the smallest size nor on the one that holds the
    // whole host, which has the host's own 10 Gigabit network instead; those two therefore have
    // no EBS-optimized performance to report. The EBS volumes share their 39 attachments with the
    // interfaces and the local devices. The network figures are the published words of the "Network
    // performance" column rather than a figure in Gigabit. Dedicated Hosts and CloudWatch action
    // based recovery are offered on the family, and so is every placement group strategy,
    // including the cluster groups the generation introduced; its AMIs may be backed by either an
    // EBS volume or the local instance store. Copying the packets of a session is done by the
    // Nitro System, which this generation predates, so Traffic Mirroring is offered on no size of
    // it, and the cores of the size that spans two processor sockets are configured one per
    // socket.
    PublishedInstanceTypeFacts::of("r3.large")
        // The smallest size is the one place the family's memory is NOT its ratio: the recorded
        // `DescribeInstanceTypes` page reports a whole 15 GiB for it, where the sizes above it
        // report the 15.25 GiB per `large` unit the rows below carry.
        .memory_mib(15_360)
        .ebs_optimized(EBS_OPTIMIZED_UNSUPPORTED)
        .shared_ebs_attachments(39)
        .instance_store(1, 32)
        .network("Moderate")
        .processor("Intel", 2.5)
        .dedicated_hosts(true)
        .auto_recovery(true)
        .placement_strategies(EVERY_PLACEMENT_STRATEGY)
        .root_devices(EBS_AND_INSTANCE_STORE_ROOT_DEVICES)
        .traffic_mirroring(false),
    PublishedInstanceTypeFacts::of("r3.xlarge")
        .memory_mib(31_232)
        .ebs(500, 4_000, 500, 4_000)
        .shared_ebs_attachments(39)
        .instance_store(1, 80)
        .network("Moderate")
        .processor("Intel", 2.5)
        .dedicated_hosts(true)
        .auto_recovery(true)
        .placement_strategies(EVERY_PLACEMENT_STRATEGY)
        .root_devices(EBS_AND_INSTANCE_STORE_ROOT_DEVICES)
        .traffic_mirroring(false),
    PublishedInstanceTypeFacts::of("r3.2xlarge")
        .memory_mib(62_464)
        .ebs(1_000, 8_000, 1_000, 8_000)
        .shared_ebs_attachments(39)
        .instance_store(1, 160)
        .network("High")
        .processor("Intel", 2.5)
        .dedicated_hosts(true)
        .auto_recovery(true)
        .placement_strategies(EVERY_PLACEMENT_STRATEGY)
        .root_devices(EBS_AND_INSTANCE_STORE_ROOT_DEVICES)
        .traffic_mirroring(false),
    PublishedInstanceTypeFacts::of("r3.4xlarge")
        .memory_mib(124_928)
        .ebs(2_000, 16_000, 2_000, 16_000)
        .shared_ebs_attachments(39)
        .instance_store(1, 320)
        .network("High")
        .processor("Intel", 2.5)
        .dedicated_hosts(true)
        .auto_recovery(true)
        .placement_strategies(EVERY_PLACEMENT_STRATEGY)
        .root_devices(EBS_AND_INSTANCE_STORE_ROOT_DEVICES)
        .traffic_mirroring(false),
    PublishedInstanceTypeFacts::of("r3.8xlarge")
        .memory_mib(249_856)
        .cores_in_steps_of(2)
        .ebs_optimized(EBS_OPTIMIZED_UNSUPPORTED)
        .shared_ebs_attachments(39)
        .instance_store(2, 320)
        .network("10 Gigabit")
        .processor("Intel", 2.5)
        .dedicated_hosts(true)
        .auto_recovery(true)
        .placement_strategies(EVERY_PLACEMENT_STRATEGY)
        .root_devices(EBS_AND_INSTANCE_STORE_ROOT_DEVICES)
        .traffic_mirroring(false),
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
        .processor("Intel", XEON6_SUSTAINED_GHZ)
        .ena_srd(false)
        .tcp_established_timeout(SHORT_TCP_ESTABLISHED_TIMEOUT)
        .host_features(true, false)
        .traffic_mirroring(false)
        .secondary_network(false),
    PublishedInstanceTypeFacts::of("r8ib.xlarge")
        .ebs(3_125, 15_000, 25_000, 120_000)
        .dedicated_ebs_attachments(32)
        .processor("Intel", XEON6_SUSTAINED_GHZ)
        .ena_srd(false)
        .tcp_established_timeout(SHORT_TCP_ESTABLISHED_TIMEOUT)
        .host_features(true, false)
        .traffic_mirroring(false)
        .secondary_network(false),
    PublishedInstanceTypeFacts::of("r8ib.2xlarge")
        .ebs(6_250, 30_000, 25_000, 120_000)
        .dedicated_ebs_attachments(32)
        .processor("Intel", XEON6_SUSTAINED_GHZ)
        .ena_srd(false)
        .tcp_established_timeout(SHORT_TCP_ESTABLISHED_TIMEOUT)
        .host_features(true, false)
        .traffic_mirroring(false)
        .secondary_network(false),
    PublishedInstanceTypeFacts::of("r8ib.4xlarge")
        .ebs(12_500, 60_000, 25_000, 120_000)
        .dedicated_ebs_attachments(32)
        .processor("Intel", XEON6_SUSTAINED_GHZ)
        .ena_srd(false)
        .tcp_established_timeout(SHORT_TCP_ESTABLISHED_TIMEOUT)
        .host_features(true, false)
        .traffic_mirroring(false)
        .secondary_network(false),
    PublishedInstanceTypeFacts::of("r8ib.8xlarge")
        .ebs(25_000, 120_000, 25_000, 120_000)
        .dedicated_ebs_attachments(32)
        .processor("Intel", XEON6_SUSTAINED_GHZ)
        .tcp_established_timeout(SHORT_TCP_ESTABLISHED_TIMEOUT)
        .host_features(true, false)
        .traffic_mirroring(false)
        .secondary_network(false),
    PublishedInstanceTypeFacts::of("r8ib.12xlarge")
        .ebs(37_500, 180_000, 37_500, 180_000)
        .dedicated_ebs_attachments(32)
        .processor("Intel", XEON6_SUSTAINED_GHZ)
        .tcp_established_timeout(SHORT_TCP_ESTABLISHED_TIMEOUT)
        .host_features(true, false)
        .traffic_mirroring(false)
        .secondary_network(false),
    PublishedInstanceTypeFacts::of("r8ib.16xlarge")
        .ebs(50_000, 240_000, 50_000, 240_000)
        .dedicated_ebs_attachments(32)
        .processor("Intel", XEON6_SUSTAINED_GHZ)
        .tcp_established_timeout(SHORT_TCP_ESTABLISHED_TIMEOUT)
        .host_features(true, false)
        .traffic_mirroring(false)
        .secondary_network(false),
    PublishedInstanceTypeFacts::of("r8ib.24xlarge")
        .cores_in_steps_of(2)
        .ebs(75_000, 360_000, 75_000, 360_000)
        .dedicated_ebs_attachments(32)
        .processor("Intel", XEON6_SUSTAINED_GHZ)
        .tcp_established_timeout(SHORT_TCP_ESTABLISHED_TIMEOUT)
        .host_features(true, false)
        .traffic_mirroring(false)
        .secondary_network(false),
    PublishedInstanceTypeFacts::of("r8ib.32xlarge")
        .cores_in_steps_of(2)
        .ebs(100_000, 480_000, 100_000, 480_000)
        .dedicated_ebs_attachments(32)
        .processor("Intel", XEON6_SUSTAINED_GHZ)
        .tcp_established_timeout(SHORT_TCP_ESTABLISHED_TIMEOUT)
        .host_features(true, false)
        .traffic_mirroring(false)
        .secondary_network(false),
    PublishedInstanceTypeFacts::of("r8ib.48xlarge")
        .cores_in_steps_of(2)
        .ebs(150_000, 720_000, 150_000, 720_000)
        .dedicated_ebs_attachments(32)
        .efa(1)
        .processor("Intel", XEON6_SUSTAINED_GHZ)
        .tcp_established_timeout(SHORT_TCP_ESTABLISHED_TIMEOUT)
        .host_features(true, false)
        .traffic_mirroring(false)
        .secondary_network(false),
    PublishedInstanceTypeFacts::of("r8ib.96xlarge")
        .cores_in_steps_of(2)
        .ebs(300_000, 1_200_000, 300_000, 1_200_000)
        .dedicated_ebs_attachments(32)
        .efa(1)
        .processor("Intel", XEON6_SUSTAINED_GHZ)
        .tcp_established_timeout(SHORT_TCP_ESTABLISHED_TIMEOUT)
        .host_features(true, false)
        .traffic_mirroring(false)
        .secondary_network(false),
    PublishedInstanceTypeFacts::of("r8ib.metal-48xl")
        .cores_in_steps_of(2)
        .ebs(150_000, 720_000, 150_000, 720_000)
        .dedicated_ebs_attachments(32)
        .efa(1)
        .processor("Intel", XEON6_SUSTAINED_GHZ)
        .tcp_established_timeout(SHORT_TCP_ESTABLISHED_TIMEOUT)
        .host_features(true, false)
        .traffic_mirroring(false)
        .secondary_network(false),
    PublishedInstanceTypeFacts::of("r8ib.metal-96xl")
        .cores_in_steps_of(2)
        .ebs(300_000, 1_200_000, 300_000, 1_200_000)
        .dedicated_ebs_attachments(32)
        .efa(1)
        .processor("Intel", XEON6_SUSTAINED_GHZ)
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
    // R8g — memory optimized on AWS Graviton4, at a sustained 2.8 GHz, with no local instance
    // store. Its network and EBS bandwidth is CONFIGURABLE: every size publishes the bandwidth
    // weightings a launch may ask for, which the families whose split is fixed publish none of.
    // The EBS figures are the Graviton4 memory ladder's rows of "Amazon EBS optimized instance
    // types": the sizes below the `8xlarge` publish a baseline they burst above, up to 10,000
    // Mbps and 40,000 IOPS, and the bandwidth is dedicated from that size up, reaching 40,000
    // Mbps and 240,000 IOPS on the sizes that hold the whole host. Those volumes are attached to
    // EBS hardware of the type's own rather than out of the pool shared with the interfaces, so
    // the limit every size reports is the EBS devices' own and is of the `dedicated` kind: 32
    // volumes up to the `12xlarge`, then 48, 64 and 128 as the size grows. The network figures
    // are the family's published ones — up to 12.5 and up to 15 Gigabit below the `8xlarge`,
    // then its share of the host's 50 — and the smallest size takes two interfaces of four
    // private addresses each rather than the count its step of the size ladder publishes. The
    // family takes no interface attachment from a secondary network, so no private address at
    // all is available on one, and the Elastic Fabric Adapter is offered on the two sizes that
    // hold the whole host and on both bare metal ones, which are also offered CloudWatch action
    // based recovery, reboot migration and a local PTP hardware clock, and whose EBS attachment
    // limit is their host's own — 79 volumes on the whole host, 39 on the half of it the smaller
    // bare metal size holds — rather than a rung of the ladder above. The sizes that carry the
    // cores of TWO processor sockets publish the lower sustained frequency of that part,
    // 2.7 GHz, where every size that sits on one publishes 2.8. ENA Express is offered from the
    // `8xlarge` up, the sizes under it carrying less dedicated network bandwidth than its
    // transport needs; the number of ENA queues cannot be changed on this family's adapter, the
    // packets of a session cannot be copied under its guest, and an enclave is offered from two
    // vCPUs up on this architecture rather than from the four an x86 type needs, so the `large`
    // size carries one.
    PublishedInstanceTypeFacts::of("r8g.medium")
        .ebs(315, 2_500, 10_000, 40_000)
        .dedicated_ebs_attachments(32)
        .network("Up to 12.5 Gigabit")
        .bandwidth_weightings(CONFIGURABLE_BANDWIDTH_WEIGHTINGS)
        .interfaces(2)
        .ipv4_per_interface(4)
        .processor("AWS", 2.8)
        .ena_srd(false)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0)
        .flexible_ena_queues(false)
        .traffic_mirroring(false),
    PublishedInstanceTypeFacts::of("r8g.large")
        .ebs(630, 3_600, 10_000, 40_000)
        .dedicated_ebs_attachments(32)
        .network("Up to 12.5 Gigabit")
        .bandwidth_weightings(CONFIGURABLE_BANDWIDTH_WEIGHTINGS)
        .processor("AWS", 2.8)
        .ena_srd(false)
        .nitro_enclaves(true)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0)
        .flexible_ena_queues(false)
        .traffic_mirroring(false),
    PublishedInstanceTypeFacts::of("r8g.xlarge")
        .ebs(1_250, 6_000, 10_000, 40_000)
        .dedicated_ebs_attachments(32)
        .network("Up to 12.5 Gigabit")
        .bandwidth_weightings(CONFIGURABLE_BANDWIDTH_WEIGHTINGS)
        .processor("AWS", 2.8)
        .ena_srd(false)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0)
        .flexible_ena_queues(false)
        .traffic_mirroring(false),
    PublishedInstanceTypeFacts::of("r8g.2xlarge")
        .ebs(2_500, 12_000, 10_000, 40_000)
        .dedicated_ebs_attachments(32)
        .network("Up to 15 Gigabit")
        .bandwidth_weightings(CONFIGURABLE_BANDWIDTH_WEIGHTINGS)
        .processor("AWS", 2.8)
        .ena_srd(false)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0)
        .flexible_ena_queues(false)
        .traffic_mirroring(false),
    PublishedInstanceTypeFacts::of("r8g.4xlarge")
        .ebs(5_000, 20_000, 10_000, 40_000)
        .dedicated_ebs_attachments(32)
        .network("Up to 15 Gigabit")
        .bandwidth_weightings(CONFIGURABLE_BANDWIDTH_WEIGHTINGS)
        .processor("AWS", 2.8)
        .ena_srd(false)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0)
        .flexible_ena_queues(false)
        .traffic_mirroring(false),
    PublishedInstanceTypeFacts::of("r8g.8xlarge")
        .ebs(10_000, 40_000, 10_000, 40_000)
        .dedicated_ebs_attachments(32)
        .network("15 Gigabit")
        .bandwidth_weightings(CONFIGURABLE_BANDWIDTH_WEIGHTINGS)
        .processor("AWS", 2.8)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0)
        .flexible_ena_queues(false)
        .traffic_mirroring(false),
    PublishedInstanceTypeFacts::of("r8g.12xlarge")
        .ebs(15_000, 60_000, 15_000, 60_000)
        .dedicated_ebs_attachments(32)
        .network("22.5 Gigabit")
        .bandwidth_weightings(CONFIGURABLE_BANDWIDTH_WEIGHTINGS)
        .processor("AWS", 2.8)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0)
        .flexible_ena_queues(false)
        .traffic_mirroring(false),
    PublishedInstanceTypeFacts::of("r8g.16xlarge")
        .ebs(20_000, 80_000, 20_000, 80_000)
        .dedicated_ebs_attachments(48)
        .network("30 Gigabit")
        .bandwidth_weightings(CONFIGURABLE_BANDWIDTH_WEIGHTINGS)
        .processor("AWS", 2.8)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0)
        .flexible_ena_queues(false)
        .traffic_mirroring(false),
    PublishedInstanceTypeFacts::of("r8g.24xlarge")
        .ebs(30_000, 120_000, 30_000, 120_000)
        .dedicated_ebs_attachments(64)
        .network("40 Gigabit")
        .bandwidth_weightings(CONFIGURABLE_BANDWIDTH_WEIGHTINGS)
        .efa(1)
        .processor("AWS", 2.8)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0)
        .flexible_ena_queues(false)
        .traffic_mirroring(false),
    PublishedInstanceTypeFacts::of("r8g.48xlarge")
        .ebs(40_000, 240_000, 40_000, 240_000)
        .dedicated_ebs_attachments(128)
        .network("50 Gigabit")
        .bandwidth_weightings(CONFIGURABLE_BANDWIDTH_WEIGHTINGS)
        .efa(1)
        .processor("AWS", 2.7)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0)
        .flexible_ena_queues(false)
        .traffic_mirroring(false),
    PublishedInstanceTypeFacts::of("r8g.metal-24xl")
        .ebs(30_000, 120_000, 30_000, 120_000)
        .dedicated_ebs_attachments(39)
        .network("40 Gigabit")
        .bandwidth_weightings(CONFIGURABLE_BANDWIDTH_WEIGHTINGS)
        .efa(1)
        .processor("AWS", 2.8)
        .ena_srd(true)
        .auto_recovery(true)
        .host_features(true, true)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0)
        .flexible_ena_queues(false)
        .traffic_mirroring(false),
    PublishedInstanceTypeFacts::of("r8g.metal-48xl")
        .ebs(40_000, 240_000, 40_000, 240_000)
        .dedicated_ebs_attachments(79)
        .network("50 Gigabit")
        .bandwidth_weightings(CONFIGURABLE_BANDWIDTH_WEIGHTINGS)
        .efa(1)
        .processor("AWS", 2.7)
        .ena_srd(true)
        .auto_recovery(true)
        .host_features(true, true)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0)
        .flexible_ena_queues(false)
        .traffic_mirroring(false),
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
    // C7i-flex — the C7i hardware offered with a FLEXIBLE share of it: the same 4th generation
    // Intel Xeon Scalable (Sapphire Rapids) processor at a sustained 3.2 GHz, publishing nested
    // virtualization as a processor feature the way its C7i sibling does, and the same
    // `dedicated` kind of EBS attachment limit — 32 volumes up to the `12xlarge`, then 48. What
    // the `flex` changes is what the size is GUARANTEED. Its published EBS baseline is a flat
    // 625 Mbps per `xlarge` of size — half of what the C7i ladder gives the same size from the
    // `xlarge` up, and half of the 625 the line itself carries at `large` rather than half of
    // the 650 C7i publishes for that one irregular row, so the smallest size sustains a figure
    // that is not a whole number of Mbps — while the maximum it bursts to is the C7i row's own.
    // The IOPS beside each baseline is the figure the published table carries at THAT bandwidth
    // whatever size holds it: 2,500 at 312.5 Mbps, 3,600 at 625, then 6,000, 12,000, 20,000,
    // 30,000 and 40,000. Its network is a burst ceiling rather than a dedicated share at every
    // size, so each row reads "Up to". Because the share is not a fixed allocation, the family is
    // offered neither a Dedicated Host nor a cluster placement group, and the Nitro features its
    // generation otherwise carries — Nitro Enclaves, ENA Express, a configurable number of ENA
    // queues and interface attachments from a secondary network, of which no private address at
    // all is available — are not offered on it either.
    PublishedInstanceTypeFacts::of("c7i-flex.large")
        .ebs_in_half_mbps(625, 2_500, 10_000, 40_000)
        .dedicated_ebs_attachments(32)
        .network("Up to 12.5 Gigabit")
        .processor("Intel", 3.2)
        .processor_features(NESTED_VIRTUALIZATION)
        .dedicated_hosts(false)
        .nitro_enclaves(false)
        .placement_strategies(PARTITION_AND_SPREAD_STRATEGIES)
        .ena_srd(false)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0)
        .flexible_ena_queues(false),
    PublishedInstanceTypeFacts::of("c7i-flex.xlarge")
        .ebs(625, 3_600, 10_000, 40_000)
        .dedicated_ebs_attachments(32)
        .network("Up to 12.5 Gigabit")
        .processor("Intel", 3.2)
        .processor_features(NESTED_VIRTUALIZATION)
        .dedicated_hosts(false)
        .nitro_enclaves(false)
        .placement_strategies(PARTITION_AND_SPREAD_STRATEGIES)
        .ena_srd(false)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0)
        .flexible_ena_queues(false),
    PublishedInstanceTypeFacts::of("c7i-flex.2xlarge")
        .ebs(1_250, 6_000, 10_000, 40_000)
        .dedicated_ebs_attachments(32)
        .network("Up to 12.5 Gigabit")
        .processor("Intel", 3.2)
        .processor_features(NESTED_VIRTUALIZATION)
        .dedicated_hosts(false)
        .nitro_enclaves(false)
        .placement_strategies(PARTITION_AND_SPREAD_STRATEGIES)
        .ena_srd(false)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0)
        .flexible_ena_queues(false),
    PublishedInstanceTypeFacts::of("c7i-flex.4xlarge")
        .ebs(2_500, 12_000, 10_000, 40_000)
        .dedicated_ebs_attachments(32)
        .network("Up to 12.5 Gigabit")
        .processor("Intel", 3.2)
        .processor_features(NESTED_VIRTUALIZATION)
        .dedicated_hosts(false)
        .nitro_enclaves(false)
        .placement_strategies(PARTITION_AND_SPREAD_STRATEGIES)
        .ena_srd(false)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0)
        .flexible_ena_queues(false),
    PublishedInstanceTypeFacts::of("c7i-flex.8xlarge")
        .ebs(5_000, 20_000, 10_000, 40_000)
        .dedicated_ebs_attachments(32)
        .network("Up to 12.5 Gigabit")
        .processor("Intel", 3.2)
        .processor_features(NESTED_VIRTUALIZATION)
        .dedicated_hosts(false)
        .nitro_enclaves(false)
        .placement_strategies(PARTITION_AND_SPREAD_STRATEGIES)
        .ena_srd(false)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0)
        .flexible_ena_queues(false),
    PublishedInstanceTypeFacts::of("c7i-flex.12xlarge")
        .ebs(7_500, 30_000, 15_000, 60_000)
        .dedicated_ebs_attachments(32)
        .network("Up to 18.75 Gigabit")
        .processor("Intel", 3.2)
        .processor_features(NESTED_VIRTUALIZATION)
        .dedicated_hosts(false)
        .nitro_enclaves(false)
        .placement_strategies(PARTITION_AND_SPREAD_STRATEGIES)
        .ena_srd(false)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0)
        .flexible_ena_queues(false),
    PublishedInstanceTypeFacts::of("c7i-flex.16xlarge")
        .ebs(10_000, 40_000, 20_000, 80_000)
        .dedicated_ebs_attachments(48)
        .network("Up to 25 Gigabit")
        .processor("Intel", 3.2)
        .processor_features(NESTED_VIRTUALIZATION)
        .dedicated_hosts(false)
        .nitro_enclaves(false)
        .placement_strategies(PARTITION_AND_SPREAD_STRATEGIES)
        .ena_srd(false)
        .secondary_network(false)
        .ipv4_per_secondary_interface(0)
        .flexible_ena_queues(false),
    // M1 — the general purpose family of AWS's FIRST generation, on AWS's "Previous generation
    // instances" page, running paravirtual and HVM AMIs on the Xen hypervisor with a magnetic
    // local instance store. Nothing about its ladder follows the modern ratios: its published
    // memory is 1.7, 3.7, 7.5 and 15 GiB, its instance store is one 160 GB device at the
    // smallest size, one of 410 GB at the next and then 420 GB devices two and four at a time,
    // and its hardware runs ONE thread per core, so a size's vCPUs are that many cores. Its
    // smallest two sizes take two network interfaces rather than the three their step of the
    // published "IP addresses per network interface per instance type" table gives. Amazon EBS
    // encryption IS supported on the family, while Traffic Mirroring — which copies a session's
    // packets under the guest, on the Nitro System this generation predates — is not, and its
    // EBS volumes share one attachment total of 39 with its other devices. Only its two largest
    // sizes have an EBS-optimized mode at all, at the 500 and 1,000 Mbps their rows of "Amazon
    // EBS optimized instance types" publish (a pre-Nitro type does not burst above its figure).
    PublishedInstanceTypeFacts::of("m1.small")
        .memory_mib(1_740)
        .threads_per_core(1)
        .instance_store(1, 160)
        .network("Low")
        .interfaces(2)
        .shared_ebs_attachments(39)
        .ebs_encryption(true)
        .traffic_mirroring(false),
    PublishedInstanceTypeFacts::of("m1.medium")
        .memory_mib(3_788)
        .threads_per_core(1)
        .instance_store(1, 410)
        .network("Moderate")
        .interfaces(2)
        .shared_ebs_attachments(39)
        .ebs_encryption(true)
        .traffic_mirroring(false),
    PublishedInstanceTypeFacts::of("m1.large")
        .memory_mib(7_680)
        .threads_per_core(1)
        .instance_store(2, 420)
        .network("Moderate")
        .shared_ebs_attachments(39)
        .ebs_encryption(true)
        .ebs_optimized("supported")
        .ebs(500, 4_000, 500, 4_000)
        .traffic_mirroring(false),
    PublishedInstanceTypeFacts::of("m1.xlarge")
        .memory_mib(15_360)
        .threads_per_core(1)
        .instance_store(4, 420)
        .network("High")
        .shared_ebs_attachments(39)
        .ebs_encryption(true)
        .ebs_optimized("supported")
        .ebs(1_000, 8_000, 1_000, 8_000)
        .traffic_mirroring(false),
    // M2 — the memory optimized family of the SECOND generation, on the same previous
    // generation page and the same Xen hardware as M1 above: paravirtual and HVM AMIs, a
    // magnetic instance store, one thread per core, Amazon EBS encryption supported, no Traffic
    // Mirroring and the same shared attachment total of 39. Its published sizes are 2, 4 and 8
    // vCPUs, and it carries one local device of 420 and 850 GB at the two smaller sizes, then
    // two of 840 GB. Its
    // `2xlarge` takes 30 private addresses per interface rather than the 15 its step of the
    // published table gives, and the two larger sizes have an EBS-optimized mode at 500 and
    // 1,000 Mbps while the smallest has none. Its published memory is 17.1, 34.2 and 68.4 GiB —
    // a ratio no family letter carries.
    PublishedInstanceTypeFacts::of("m2.xlarge")
        .memory_mib(17_510)
        .vcpus(2)
        .threads_per_core(1)
        .instance_store(1, 420)
        .network("Moderate")
        .shared_ebs_attachments(39)
        .ebs_encryption(true)
        .traffic_mirroring(false),
    PublishedInstanceTypeFacts::of("m2.2xlarge")
        .memory_mib(35_020)
        .vcpus(4)
        .threads_per_core(1)
        .instance_store(1, 850)
        .network("Moderate")
        .ipv4_per_interface(30)
        .shared_ebs_attachments(39)
        .ebs_encryption(true)
        .ebs_optimized("supported")
        .ebs(500, 4_000, 500, 4_000)
        .traffic_mirroring(false),
    PublishedInstanceTypeFacts::of("m2.4xlarge")
        .memory_mib(70_041)
        .vcpus(8)
        .threads_per_core(1)
        .instance_store(2, 840)
        .network("High")
        .shared_ebs_attachments(39)
        .ebs_encryption(true)
        .ebs_optimized("supported")
        .ebs(1_000, 8_000, 1_000, 8_000)
        .traffic_mirroring(false),
    // U-6tb1 — a High Memory instance: 6 TiB of memory on an eight-socket Intel Xeon Platinum
    // (Skylake) server at a sustained 2.1 GHz, with no local instance store. Its name states the
    // memory rather than a generation, and its two sizes are the SAME server with its second
    // thread per core shipped on and off: 448 vCPUs on 224 cores, and 224 vCPUs on those same
    // 224 cores. So both publish the server's own figures — a dedicated 38,000 Mbps and 160,000
    // IOPS of EBS, 27 volumes attached out of the pool it shares with its interfaces, and 100
    // Gigabit of network — and both take their cores all at once, so the one count each
    // publishes is its default. The family is offered On-Demand capacity only, boots in the
    // legacy BIOS mode alone, and is offered neither Nitro Enclaves nor a trusted platform
    // module.
    PublishedInstanceTypeFacts::of("u-6tb1.56xlarge")
        .threads_per_core(1)
        .cores_in_steps_of(224)
        .ebs(38_000, 160_000, 38_000, 160_000)
        .ebs_attachments(27)
        .network("100 Gigabit")
        .processor("Intel", 2.1)
        .usage_classes(ON_DEMAND_USAGE_CLASS)
        .boot_modes(LEGACY_BIOS_BOOT_MODE)
        .nitro_enclaves(false)
        .nitro_tpm(false),
    PublishedInstanceTypeFacts::of("u-6tb1.112xlarge")
        .cores_in_steps_of(224)
        .ebs(38_000, 160_000, 38_000, 160_000)
        .ebs_attachments(27)
        .network("100 Gigabit")
        .processor("Intel", 2.1)
        .usage_classes(ON_DEMAND_USAGE_CLASS)
        .boot_modes(LEGACY_BIOS_BOOT_MODE)
        .nitro_enclaves(false)
        .nitro_tpm(false),
];

/// Every published row the catalog holds, whichever table it sits in.
///
/// The rows of the families above, then the rows each child module holds for the families it
/// groups — the AMD EPYC families, the accelerated ones, the network optimized ones, the
/// previous generation ones, the burstable families on Nitro and the one on pre-Nitro
/// hardware, the dense HDD storage ones, the eighth generation Intel compute optimized ones,
/// the fifth generation Intel general purpose one, the fifth generation network optimized
/// ones, those of them that carry local NVMe
/// devices, the general purpose families with local
/// NVMe devices, the families whose accelerators are Neuron devices, the compute optimized
/// families on the newest AWS Graviton hardware, the storage optimized Graviton ones, the
/// fourth generation families on the Xen hypervisor and the memory optimized ones with local
/// NVMe devices, the seventh generation graphics family, the block storage optimized memory
/// one on the newest AWS Graviton hardware, the two storage optimized families on the Xen
/// hypervisor, the two Intel storage optimized ones, the two high-memory families on the
/// Xen hypervisor, the sixth generation AWS Graviton general purpose one, the first generation
/// AWS Graviton general purpose one and the EC2 Mac hosts. A row is reached by its instance
/// type alone, so which table it is written in is nothing but where the figures of one group
/// of families are kept together.
fn published_rows() -> impl Iterator<Item = &'static PublishedInstanceTypeFacts> {
    PUBLISHED_INSTANCE_TYPE_FACTS
        .iter()
        .chain(amd_epyc_rows::AMD_EPYC_INSTANCE_TYPE_FACTS)
        .chain(burstable_rows::BURSTABLE_INSTANCE_TYPE_FACTS)
        .chain(gpu_rows::GPU_INSTANCE_TYPE_FACTS)
        .chain(intel_nvme_compute_rows::INTEL_NVME_COMPUTE_INSTANCE_TYPE_FACTS)
        .chain(network_optimized_rows::NETWORK_OPTIMIZED_INSTANCE_TYPE_FACTS)
        .chain(neuron_accelerated_rows::NEURON_ACCELERATED_INSTANCE_TYPE_FACTS)
        .chain(
            newest_generation_graviton_rows::NEWEST_GENERATION_GRAVITON_INSTANCE_TYPE_FACTS,
        )
        .chain(previous_generation_rows::PREVIOUS_GENERATION_INSTANCE_TYPE_FACTS)
        .chain(burstable_xen_rows::BURSTABLE_XEN_INSTANCE_TYPE_FACTS)
        .chain(dense_hdd_storage_rows::DENSE_HDD_STORAGE_INSTANCE_TYPE_FACTS)
        .chain(eighth_generation_intel_rows::EIGHTH_GENERATION_INTEL_INSTANCE_TYPE_FACTS)
        .chain(fifth_generation_intel_rows::FIFTH_GENERATION_INTEL_INSTANCE_TYPE_FACTS)
        .chain(
            fifth_generation_intel_compute_rows::FIFTH_GENERATION_INTEL_COMPUTE_INSTANCE_TYPE_FACTS,
        )
        .chain(
            fifth_generation_intel_nvme_compute_rows::FIFTH_GENERATION_INTEL_NVME_COMPUTE_INSTANCE_TYPE_FACTS,
        )
        .chain(
            first_generation_graviton_rows::FIRST_GENERATION_GRAVITON_INSTANCE_TYPE_FACTS,
        )
        .chain(
            fifth_generation_network_optimized_rows::FIFTH_GENERATION_NETWORK_OPTIMIZED_INSTANCE_TYPE_FACTS,
        )
        .chain(fourth_generation_xen_rows::FOURTH_GENERATION_XEN_INSTANCE_TYPE_FACTS)
        .chain(general_purpose_nvme_rows::GENERAL_PURPOSE_NVME_INSTANCE_TYPE_FACTS)
        .chain(memory_optimized_nvme_rows::MEMORY_OPTIMIZED_NVME_INSTANCE_TYPE_FACTS)
        .chain(
            storage_optimized_graviton_rows::STORAGE_OPTIMIZED_GRAVITON_INSTANCE_TYPE_FACTS,
        )
        .chain(seventh_generation_graphics_rows::SEVENTH_GENERATION_GRAPHICS_INSTANCE_TYPE_FACTS)
        .chain(
            block_storage_optimized_graviton_memory_rows::BLOCK_STORAGE_OPTIMIZED_GRAVITON_MEMORY_INSTANCE_TYPE_FACTS,
        )
        .chain(xen_storage_optimized_rows::XEN_STORAGE_OPTIMIZED_INSTANCE_TYPE_FACTS)
        .chain(intel_storage_optimized_rows::INTEL_STORAGE_OPTIMIZED_INSTANCE_TYPE_FACTS)
        .chain(xen_high_memory_rows::XEN_HIGH_MEMORY_INSTANCE_TYPE_FACTS)
        .chain(sixth_generation_graviton_rows::SIXTH_GENERATION_GRAVITON_INSTANCE_TYPE_FACTS)
        .chain(mac_host_rows::MAC_HOST_INSTANCE_TYPE_FACTS)
}

/// The published facts of `instance_type`, or `None` where the catalog holds no row for it
/// and every member is the derivation's.
pub fn published_instance_type_facts(
    instance_type: &str,
) -> Option<&'static PublishedInstanceTypeFacts> {
    published_rows().find(|facts| facts.instance_type == instance_type)
}

/// Every instance type the table above holds a row for.
///
/// A row is reached by its instance type ([`published_instance_type_facts`]), so a key that
/// names no type of the catalog would be a row nothing ever reads; this is what lets a test
/// hold every key to being one.
pub fn published_instance_types() -> impl Iterator<Item = &'static str> {
    published_rows().map(|facts| facts.instance_type)
}

/// The local instance store of one instance type: the devices `facts` publishes for it, or
/// the `derived` store of its name where its row publishes none.
///
/// A row that publishes devices says how many there are and how big they are, which is the
/// half of the fact no name yields; the device MODEL — magnetic or solid state, NVMe or not —
/// comes from the derived store of the same type where its name yields one, and from the name
/// itself where it does not. A row that publishes NO devices says the type carries no instance
/// store at all, which overrides the derivation rather than falling back to it: a family whose
/// name reads as having local devices can still have a size that carries none.
///
/// Whether the store is ENCRYPTED is a published fact of its own, so a row carries it apart
/// from the devices and from the device model: the key the host holds is applied to the store
/// of the types AWS publishes it for, whether or not those devices are presented as NVMe ones
/// — and a row that publishes nothing about it keeps the derived answer, which is that the
/// NVMe stores are the encrypted ones.
pub(super) fn published_instance_storage(
    facts: Option<&'static PublishedInstanceTypeFacts>,
    name: &InstanceTypeName<'_>,
    derived: Option<InstanceStorageFacts>,
) -> Option<InstanceStorageFacts> {
    let published_encryption = facts.and_then(|facts| facts.instance_store_encrypted);
    let Some((devices, gb_each)) = facts.and_then(|facts| facts.instance_store_devices) else {
        return derived.map(|store| InstanceStorageFacts {
            encrypted: published_encryption.unwrap_or(store.encrypted),
            ..store
        });
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
        encrypted: published_encryption.unwrap_or(nvme),
    })
}
