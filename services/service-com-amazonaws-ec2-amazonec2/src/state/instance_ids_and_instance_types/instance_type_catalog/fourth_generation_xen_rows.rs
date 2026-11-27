// SPDX-License-Identifier: CC0-1.0

//! The published rows of the fourth generation families that run on the Xen hypervisor, read
//! through [`super::published_instance_type_facts`] like every other row of the catalog.
//!
//! These families came before the AWS Nitro System, and almost nothing about them follows
//! from their size: EBS optimization gives ONE fixed figure per size rather than a baseline
//! an instance bursts above, the EBS volumes share one attachment total with the interfaces
//! and the local devices rather than having a limit of their own, the memory of the compute
//! optimized and memory optimized families is not a whole number of GiB per vCPU, enhanced
//! networking with the
//! Elastic Network Adapter reaches only the sizes whose hardware carries that adapter, the
//! network performance column is published as WORDS on the general purpose and compute
//! optimized families, the
//! interface and address limits of the smallest and largest sizes are the type's own, and the
//! core counts a two-socket size can be configured with go up one per socket. None of the
//! three families is the latest generation of its family, while all are offered on a Dedicated
//! Host and all are migrated to another host on a user-initiated reboot.
//!
//! The figures come from each family's own specification page (vCPUs, memory, network
//! performance), the "Amazon EBS optimized instance types" table (the EBS figures),
//! "Instance volume limits" (the shared attachment total), "Enable enhanced networking with
//! ENA" (which sizes carry that adapter) and the "IP addresses per network interface per
//! instance type" table (the interface and address limits).
//!
//! The two network figures the compute optimized family states beside its words are its
//! hardware's own: the share of its host's [`C4_HOST_NETWORK_GBPS`] that the size sustains,
//! and the host's whole figure, which every size of the family can reach. No reading of the
//! size multiplier yields the words, and neither figure follows from the generic share a later
//! generation's host is divided into. The queues one interface of the family's adapter is
//! given ([`C4_ENA_QUEUES_PER_INTERFACE`]) are a figure of that adapter rather than of the
//! size.

use super::*;

/// The published hardware facts of the fourth generation Xen families' instance types, one row
/// per type.
pub(super) static FOURTH_GENERATION_XEN_INSTANCE_TYPE_FACTS: &[PublishedInstanceTypeFacts] = &[
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
    // cores are configured one per socket. Its network is its share of the 10 Gigabit its host
    // carries — the whole of it on the size that IS the host — beside the host's own figure,
    // which every size of the family can reach; its adapter gives every interface the same
    // eight ENA queues.
    PublishedInstanceTypeFacts::of("c4.large")
        .previous_generation()
        .memory_mib(3_840)
        .ebs(500, 4_000, 500, 4_000)
        .shared_ebs_attachments(40)
        .network("Moderate")
        .network_bandwidth(0.625, C4_HOST_NETWORK_GBPS)
        .processor("Intel", 2.9)
        .ena(ENA_UNSUPPORTED)
        .ena_queues(C4_ENA_QUEUES_PER_INTERFACE)
        .dedicated_hosts(true)
        .host_features(true, false),
    PublishedInstanceTypeFacts::of("c4.xlarge")
        .previous_generation()
        .memory_mib(7_680)
        .ebs(750, 6_000, 750, 6_000)
        .shared_ebs_attachments(40)
        .network("High")
        .network_bandwidth(1.25, C4_HOST_NETWORK_GBPS)
        .processor("Intel", 2.9)
        .ena(ENA_UNSUPPORTED)
        .ena_queues(C4_ENA_QUEUES_PER_INTERFACE)
        .dedicated_hosts(true)
        .host_features(true, false),
    PublishedInstanceTypeFacts::of("c4.2xlarge")
        .previous_generation()
        .memory_mib(15_360)
        .ebs(1_000, 8_000, 1_000, 8_000)
        .shared_ebs_attachments(40)
        .network("High")
        .network_bandwidth(2.5, C4_HOST_NETWORK_GBPS)
        .processor("Intel", 2.9)
        .ena(ENA_UNSUPPORTED)
        .ena_queues(C4_ENA_QUEUES_PER_INTERFACE)
        .dedicated_hosts(true)
        .host_features(true, false),
    PublishedInstanceTypeFacts::of("c4.4xlarge")
        .previous_generation()
        .memory_mib(30_720)
        .ebs(2_000, 16_000, 2_000, 16_000)
        .shared_ebs_attachments(40)
        .network("High")
        .network_bandwidth(5.0, C4_HOST_NETWORK_GBPS)
        .processor("Intel", 2.9)
        .ena(ENA_UNSUPPORTED)
        .ena_queues(C4_ENA_QUEUES_PER_INTERFACE)
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
        .network_bandwidth(C4_HOST_NETWORK_GBPS, C4_HOST_NETWORK_GBPS)
        .processor("Intel", 2.9)
        .ena(ENA_UNSUPPORTED)
        .ena_queues(C4_ENA_QUEUES_PER_INTERFACE)
        .dedicated_hosts(true)
        .host_features(true, false),
    // M4 — general purpose, Intel Xeon E5-2676 v3 (Haswell) at a sustained 2.4 GHz, with no
    // local instance store at any size. Each size publishes ONE EBS figure, the family's own
    // row of the published table rather than a share of its host. The smallest size takes two
    // interfaces where its step of the size ladder publishes three, and the largest takes
    // eight of thirty addresses where that step publishes fifteen of fifty. Enhanced
    // networking with ENA reaches the largest size alone; every smaller size is offered the
    // older adapter instead, which the member reports as no ENA at all. The two sizes whose
    // cores sit on two processor sockets take them one per socket. The Dedicated Host carries
    // the whole host, so the size that IS the whole host is not offered one.
    PublishedInstanceTypeFacts::of("m4.large")
        .previous_generation()
        .ebs(450, 3_600, 450, 3_600)
        .shared_ebs_attachments(40)
        .network("Moderate")
        .interfaces(2)
        .ena(ENA_UNSUPPORTED)
        .processor("Intel", 2.4)
        .dedicated_hosts(true)
        .host_features(true, false),
    PublishedInstanceTypeFacts::of("m4.xlarge")
        .previous_generation()
        .ebs(750, 6_000, 750, 6_000)
        .shared_ebs_attachments(40)
        .network("High")
        .ena(ENA_UNSUPPORTED)
        .processor("Intel", 2.4)
        .dedicated_hosts(true)
        .host_features(true, false),
    PublishedInstanceTypeFacts::of("m4.2xlarge")
        .previous_generation()
        .ebs(1_000, 8_000, 1_000, 8_000)
        .shared_ebs_attachments(40)
        .network("High")
        .ena(ENA_UNSUPPORTED)
        .processor("Intel", 2.4)
        .dedicated_hosts(true)
        .host_features(true, false),
    PublishedInstanceTypeFacts::of("m4.4xlarge")
        .previous_generation()
        .ebs(2_000, 16_000, 2_000, 16_000)
        .shared_ebs_attachments(40)
        .network("High")
        .ena(ENA_UNSUPPORTED)
        .processor("Intel", 2.4)
        .dedicated_hosts(true)
        .host_features(true, false),
    PublishedInstanceTypeFacts::of("m4.10xlarge")
        .previous_generation()
        .cores_in_steps_of(2)
        .ebs(4_000, 32_000, 4_000, 32_000)
        .shared_ebs_attachments(40)
        .network_bandwidth(10.0, 10.0)
        .ena(ENA_UNSUPPORTED)
        .processor("Intel", 2.4)
        .dedicated_hosts(true)
        .host_features(true, false),
    PublishedInstanceTypeFacts::of("m4.16xlarge")
        .previous_generation()
        .cores_in_steps_of(2)
        .ebs(10_000, 65_000, 10_000, 65_000)
        .shared_ebs_attachments(40)
        .network_bandwidth(25.0, 25.0)
        .interfaces(8)
        .ipv4_per_interface(30)
        // The size that holds the whole host is built on the Broadwell processor, a sustained
        // 2.3 GHz, where every size below it is built on the Haswell one.
        .processor("Intel", 2.3)
        .dedicated_hosts(false)
        .host_features(true, false),
    // R4 — memory optimized, Intel Xeon E5-2686 v4 (Broadwell) at a sustained 2.3 GHz, with no
    // local instance store. Its published memory is 15.25 GiB per `large` unit of size, which
    // is not a whole number of GiB at any size, and each size publishes ONE EBS figure of its
    // own. Enhanced networking with ENA is offered on the whole family, so no row says
    // otherwise, and each of its interfaces is given the eight ENA queues that adapter carries,
    // whatever the size. Its network figures are the published ones: the four narrowest sizes
    // sustain the family's own baseline ladder and burst to 10 Gigabit, while the `8xlarge`
    // holds a dedicated 10 and the size that IS the host its whole 25. The largest size's cores
    // sit on two processor sockets and are taken one per socket.
    PublishedInstanceTypeFacts::of("r4.large")
        .previous_generation()
        .memory_mib(15_616)
        .ebs(425, 3_000, 425, 3_000)
        .shared_ebs_attachments(40)
        .network("Up to 10 Gigabit")
        .network_bandwidth(0.75, 10.0)
        .ena_queues(R4_ENA_QUEUES_PER_INTERFACE)
        .processor("Intel", 2.3)
        .dedicated_hosts(true)
        .host_features(true, false),
    PublishedInstanceTypeFacts::of("r4.xlarge")
        .previous_generation()
        .memory_mib(31_232)
        .ebs(850, 6_000, 850, 6_000)
        .shared_ebs_attachments(40)
        .network("Up to 10 Gigabit")
        .network_bandwidth(1.25, 10.0)
        .ena_queues(R4_ENA_QUEUES_PER_INTERFACE)
        .processor("Intel", 2.3)
        .dedicated_hosts(true)
        .host_features(true, false),
    PublishedInstanceTypeFacts::of("r4.2xlarge")
        .previous_generation()
        .memory_mib(62_464)
        .ebs(1_700, 12_000, 1_700, 12_000)
        .shared_ebs_attachments(40)
        .network("Up to 10 Gigabit")
        .network_bandwidth(2.5, 10.0)
        .ena_queues(R4_ENA_QUEUES_PER_INTERFACE)
        .processor("Intel", 2.3)
        .dedicated_hosts(true)
        .host_features(true, false),
    PublishedInstanceTypeFacts::of("r4.4xlarge")
        .previous_generation()
        .memory_mib(124_928)
        .ebs(3_500, 18_750, 3_500, 18_750)
        .shared_ebs_attachments(40)
        .network("Up to 10 Gigabit")
        .network_bandwidth(5.0, 10.0)
        .ena_queues(R4_ENA_QUEUES_PER_INTERFACE)
        .processor("Intel", 2.3)
        .dedicated_hosts(true)
        .host_features(true, false),
    PublishedInstanceTypeFacts::of("r4.8xlarge")
        .previous_generation()
        .memory_mib(249_856)
        .ebs(7_000, 37_500, 7_000, 37_500)
        .shared_ebs_attachments(40)
        .network("10 Gigabit")
        // The published figure of the size that holds half the host is 12 Gbps, sustained and
        // reached alike — the same figure the half-host sizes of the next generation publish,
        // and not the round 10 Gigabit of the column published beside it.
        .network_bandwidth(12.0, 12.0)
        .ena_queues(R4_ENA_QUEUES_PER_INTERFACE)
        .processor("Intel", 2.3)
        .dedicated_hosts(true)
        .host_features(true, false),
    PublishedInstanceTypeFacts::of("r4.16xlarge")
        .previous_generation()
        .memory_mib(499_712)
        .cores_in_steps_of(2)
        .ebs(14_000, 75_000, 14_000, 75_000)
        .shared_ebs_attachments(40)
        .network_bandwidth(25.0, 25.0)
        .ena_queues(R4_ENA_QUEUES_PER_INTERFACE)
        .processor("Intel", 2.3)
        .dedicated_hosts(true)
        .host_features(true, false),
];

/// The Gigabit the network of the compute optimized family's host carries: the figure the size
/// that IS that host publishes, which every smaller size of the family holds a share of and
/// every size of it can reach.
const C4_HOST_NETWORK_GBPS: f64 = 10.0;

/// The ENA queues one interface of that family is given by default: a figure of the adapter
/// its hardware carries, which is the same at every size of it.
const C4_ENA_QUEUES_PER_INTERFACE: i32 = 8;

/// The ENA queues one interface of the memory optimized family is given by default: a figure
/// of the Elastic Network Adapter its hardware carries, the same at every size of it.
const R4_ENA_QUEUES_PER_INTERFACE: i32 = 8;
