// SPDX-License-Identifier: CC0-1.0

//! The published rows of the fifth generation Intel compute optimized family that carries
//! local NVMe devices, read through [`super::published_instance_type_facts`] like every other
//! row of the catalog.
//!
//! The `d` variant of that generation's compute family sits on the SAME hardware as the plain
//! one ([`super::fifth_generation_intel_compute_rows`]) with local NVMe SSD devices added, so
//! the two publish one set of EBS and network figures and one adapter: the network bandwidth of
//! a size is the figure published for it rather than its share of the host, two sizes carved
//! out of the same half of a host reporting the same figure, and each interface is given the
//! eight ENA queues the adapter carries whatever the size. What the devices change is the
//! type's own hardware: they take their share of the shared Nitro attachment pool, and no size
//! is offered reboot migration, because the move to another host cannot carry the local data
//! over.
//!
//! The figures come from the family's own specification page (the network bandwidth each size
//! sustains and the ceiling it bursts to, the ENA queues one interface is given and the local
//! devices per size), the "Amazon EBS optimized instance types" table (the four EBS figures per
//! size), "Instance volume limits" (the attachments a bare metal size holds) and "Boot modes"
//! (the modes a bare metal size of this generation is offered).

use super::*;

/// The published hardware facts of the fifth generation Intel NVMe compute family's instance
/// types, one row per type.
pub(super) static FIFTH_GENERATION_INTEL_NVME_COMPUTE_INSTANCE_TYPE_FACTS:
    &[PublishedInstanceTypeFacts] = &[
    // C5d — the C5 family's hardware with local NVMe SSD devices, on the same Intel Xeon
    // Platinum processors: the 8124M (Skylake) at a sustained 3.4 GHz on most of the ladder,
    // and the 8275CL (Cascade Lake) at a sustained 3.6 GHz on the sizes built on it
    // (`12xlarge`, `24xlarge` and the bare metal size). Its EBS figures are the family's own
    // rows of "Amazon EBS optimized instance types" — the three smallest sizes publish a
    // baseline they burst above, up to 4,750 Mbps and 20,000 IOPS, and the bandwidth is
    // dedicated from the `4xlarge` up, two sizes sharing each of the figures above that one —
    // and its network figures are the published ones: the four narrowest sizes sustain the
    // family's own baseline ladder and burst to 10 Gigabit, and from the `9xlarge` up each
    // size reports the dedicated figure published for it, which the two sizes carved out of
    // one half of a host share. Its local devices are the ones its specification page
    // publishes per size: ONE device that doubles with the size up to 900 GB, then two and
    // four of that device, which is a step of its own rather than a share of the size. Those
    // devices come out of the shared Nitro attachment pool on every virtualized size, so the
    // EBS volume limit each of those reports is what the pool leaves once they are counted and
    // those rows leave it unset; the BARE METAL size publishes the limit of a bare metal
    // instance instead — its local devices are the host's own hardware rather than attachments
    // a hypervisor presents, so they take nothing from it — which is the 31 volumes "Instance
    // volume limits" publishes for one. Reboot migration moves a running instance to another
    // host, which cannot carry the local devices' data, so no size of the family is offered it,
    // and a PTP hardware clock is not on this hardware generation. The published core counts go
    // up in twos from the `xlarge` and start at four on the `18xlarge`, whose published list
    // begins above that step, while the bare metal size publishes every count. Dedicated Hosts
    // are offered on the family except on the bare metal size, which holds the whole host; that
    // size is offered the legacy BIOS boot mode alone, and the packets of a session CAN be
    // mirrored on it.
    PublishedInstanceTypeFacts::of("c5d.large")
        .ebs(650, 4_000, 4_750, 20_000)
        .instance_store(1, 50)
        .network_bandwidth(0.75, 10.0)
        .ena_queues(C5D_ENA_QUEUES_PER_INTERFACE)
        .processor("Intel", 3.4)
        .host_features(false, false),
    PublishedInstanceTypeFacts::of("c5d.xlarge")
        .cores_in_steps_of(2)
        .ebs(1_150, 6_000, 4_750, 20_000)
        .instance_store(1, 100)
        .network_bandwidth(1.25, 10.0)
        .ena_queues(C5D_ENA_QUEUES_PER_INTERFACE)
        .processor("Intel", 3.4)
        .host_features(false, false),
    PublishedInstanceTypeFacts::of("c5d.2xlarge")
        .cores_in_steps_of(2)
        .ebs(2_300, 10_000, 4_750, 20_000)
        .instance_store(1, 200)
        .network_bandwidth(2.5, 10.0)
        .ena_queues(C5D_ENA_QUEUES_PER_INTERFACE)
        .processor("Intel", 3.4)
        .host_features(false, false),
    PublishedInstanceTypeFacts::of("c5d.4xlarge")
        .cores_in_steps_of(2)
        .ebs(4_750, 20_000, 4_750, 20_000)
        .instance_store(1, 400)
        .network_bandwidth(5.0, 10.0)
        .ena_queues(C5D_ENA_QUEUES_PER_INTERFACE)
        .processor("Intel", 3.4)
        .host_features(false, false),
    PublishedInstanceTypeFacts::of("c5d.9xlarge")
        .cores_in_steps_of(2)
        .ebs(9_500, 40_000, 9_500, 40_000)
        .instance_store(1, 900)
        // The two sizes carved out of the same half of the host report the same figure, which
        // is the one the API publishes for them rather than the round number of the family's
        // specification page — and it is DEDICATED, so the size reports it as both the
        // bandwidth it sustains and the bandwidth it reaches.
        .network_bandwidth(12.0, 12.0)
        .ena_queues(C5D_ENA_QUEUES_PER_INTERFACE)
        .processor("Intel", 3.4)
        .host_features(false, false),
    PublishedInstanceTypeFacts::of("c5d.12xlarge")
        .cores_in_steps_of(2)
        .ebs(9_500, 40_000, 9_500, 40_000)
        .instance_store(2, 900)
        .network_bandwidth(12.0, 12.0)
        .ena_queues(C5D_ENA_QUEUES_PER_INTERFACE)
        .processor("Intel", 3.6)
        .host_features(false, false),
    PublishedInstanceTypeFacts::of("c5d.18xlarge")
        .cores_in_steps_of(2)
        .cores_from(4)
        .ebs(19_000, 80_000, 19_000, 80_000)
        .instance_store(2, 900)
        .network_bandwidth(25.0, 25.0)
        .ena_queues(C5D_ENA_QUEUES_PER_INTERFACE)
        .processor("Intel", 3.4)
        .host_features(false, false),
    PublishedInstanceTypeFacts::of("c5d.24xlarge")
        .cores_in_steps_of(2)
        .ebs(19_000, 80_000, 19_000, 80_000)
        .instance_store(4, 900)
        .network_bandwidth(25.0, 25.0)
        .ena_queues(C5D_ENA_QUEUES_PER_INTERFACE)
        .processor("Intel", 3.6)
        .host_features(false, false),
    PublishedInstanceTypeFacts::of("c5d.metal")
        .ebs(19_000, 80_000, 19_000, 80_000)
        .ebs_attachments(31)
        .instance_store(4, 900)
        .network_bandwidth(25.0, 25.0)
        .ena_queues(C5D_ENA_QUEUES_PER_INTERFACE)
        .processor("Intel", 3.6)
        .dedicated_hosts(false)
        .traffic_mirroring(true)
        .host_features(false, false)
        .boot_modes(LEGACY_BIOS_BOOT_MODE),
];

/// The ENA queues each interface of a C5d instance is given by default
/// (`NetworkCardInfo$DefaultEnaQueueCountPerInterface`): the queue count of the adapter this
/// hardware's network card carries, which every size of the family publishes whatever its
/// vCPUs — the same adapter, and so the same count, as the family's non-NVMe sibling.
const C5D_ENA_QUEUES_PER_INTERFACE: i32 = 8;
