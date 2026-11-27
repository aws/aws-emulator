// SPDX-License-Identifier: CC0-1.0

//! The published rows of the fifth generation Intel compute optimized family, read through
//! [`super::published_instance_type_facts`] like every other row of the catalog.
//!
//! The family that moved the compute optimized ladder onto the AWS Nitro System, and what it
//! publishes per size follows no ratio: its EBS bandwidth is a baseline the three narrowest
//! sizes burst above and a dedicated figure from the `4xlarge` up, with two sizes sharing each
//! figure above that one; its network bandwidth is the figure published for the size rather
//! than its share of the host, two sizes carved out of the same half of a host reporting one
//! figure; and the sizes are built on two processor generations, so the sustained frequency
//! differs along the ladder.
//!
//! The figures come from the family's own specification page (the network bandwidth each size
//! sustains and the ceiling it bursts to, and the ENA queues one interface is given), the
//! "Amazon EBS optimized instance types" table (the four EBS figures per size) and "Boot
//! modes" (the modes a bare metal size of this generation is offered).

use super::*;

/// The published hardware facts of the fifth generation Intel compute optimized family's
/// instance types, one row per type.
pub(super) static FIFTH_GENERATION_INTEL_COMPUTE_INSTANCE_TYPE_FACTS: &[PublishedInstanceTypeFacts] = &[
    // C5 — compute optimized on the AWS Nitro System, Intel Xeon Platinum: the 8124M
    // (Skylake) at a sustained 3.4 GHz on most of the ladder, and the 8275CL (Cascade Lake) at
    // a sustained 3.6 GHz on the sizes built on it (`12xlarge`, `24xlarge` and the bare metal
    // size). The family is NOT the latest generation of its family, and no size carries a
    // local instance store. Its EBS figures are the family's own rows of "Amazon EBS optimized
    // instance types": the three smallest sizes publish a baseline they burst above, up to
    // 4,750 Mbps and 20,000 IOPS, and the family's bandwidth is dedicated from the `4xlarge`
    // up, two sizes sharing each of the figures above that one. Its network figures are the
    // published ones: the four narrowest sizes sustain the family's own baseline ladder and
    // burst to 10 Gigabit, and from the `9xlarge` up each size reports the dedicated figure
    // published for it. Each of its interfaces is given the eight ENA queues its adapter
    // carries, whatever the size. The published core counts go up in twos from the `xlarge`
    // and start at four on the `18xlarge`, whose published list begins above that step, while
    // the bare metal size publishes every count. Dedicated Hosts are offered on the family
    // EXCEPT on the two sizes that hold the whole host, and the bare metal size is offered the
    // legacy BIOS boot mode alone, supports CloudWatch action based recovery — which the sizes
    // with no instance store already do — and supports reboot migration, the move to another
    // host being the HOST's to make and not the hypervisor's that a bare metal instance has
    // none of. That size's EBS volumes are attached beside the shared pool rather than out of
    // it, so it holds the volumes a whole host carries, and the packets of a session are copied
    // by the host's own card, which it still has.
    PublishedInstanceTypeFacts::of("c5.large")
        .previous_generation()
        .ebs(650, 4_000, 4_750, 20_000)
        .network_bandwidth(0.75, 10.0)
        .ena_queues(C5_ENA_QUEUES_PER_INTERFACE)
        .processor("Intel", 3.4)
        .dedicated_hosts(true),
    PublishedInstanceTypeFacts::of("c5.xlarge")
        .previous_generation()
        .cores_in_steps_of(2)
        .ebs(1_150, 6_000, 4_750, 20_000)
        .network_bandwidth(1.25, 10.0)
        .ena_queues(C5_ENA_QUEUES_PER_INTERFACE)
        .processor("Intel", 3.4)
        .dedicated_hosts(true),
    PublishedInstanceTypeFacts::of("c5.2xlarge")
        .previous_generation()
        .cores_in_steps_of(2)
        .ebs(2_300, 10_000, 4_750, 20_000)
        .network_bandwidth(2.5, 10.0)
        .ena_queues(C5_ENA_QUEUES_PER_INTERFACE)
        .processor("Intel", 3.4)
        .dedicated_hosts(true),
    PublishedInstanceTypeFacts::of("c5.4xlarge")
        .previous_generation()
        .cores_in_steps_of(2)
        .ebs(4_750, 20_000, 4_750, 20_000)
        .network_bandwidth(5.0, 10.0)
        .ena_queues(C5_ENA_QUEUES_PER_INTERFACE)
        .processor("Intel", 3.4)
        .dedicated_hosts(true),
    PublishedInstanceTypeFacts::of("c5.9xlarge")
        .previous_generation()
        .cores_in_steps_of(2)
        .ebs(9_500, 40_000, 9_500, 40_000)
        // The two sizes carved out of the same half of the host report the same figure, which
        // is the one the API publishes for them rather than the round number of the family's
        // specification page — and it is DEDICATED, so the size reports it as both the
        // bandwidth it sustains and the bandwidth it reaches.
        .network_bandwidth(12.0, 12.0)
        .ena_queues(C5_ENA_QUEUES_PER_INTERFACE)
        .processor("Intel", 3.4)
        .dedicated_hosts(true),
    PublishedInstanceTypeFacts::of("c5.12xlarge")
        .previous_generation()
        .cores_in_steps_of(2)
        .ebs(9_500, 40_000, 9_500, 40_000)
        .network_bandwidth(12.0, 12.0)
        .ena_queues(C5_ENA_QUEUES_PER_INTERFACE)
        .processor("Intel", 3.6)
        .dedicated_hosts(true),
    PublishedInstanceTypeFacts::of("c5.18xlarge")
        .previous_generation()
        .cores_in_steps_of(2)
        // The size whose cores sit on two sockets of 18 publishes its counts from FOUR, not
        // from the two its step would start at: the published list of this size begins above
        // its step, as the same host's NVMe sibling does at the same size.
        .cores_from(4)
        .ebs(19_000, 80_000, 19_000, 80_000)
        .network_bandwidth(25.0, 25.0)
        .ena_queues(C5_ENA_QUEUES_PER_INTERFACE)
        .processor("Intel", 3.4)
        .dedicated_hosts(true),
    PublishedInstanceTypeFacts::of("c5.24xlarge")
        .previous_generation()
        .cores_in_steps_of(2)
        .ebs(19_000, 80_000, 19_000, 80_000)
        .network_bandwidth(25.0, 25.0)
        .ena_queues(C5_ENA_QUEUES_PER_INTERFACE)
        .processor("Intel", 3.6)
        .dedicated_hosts(false),
    PublishedInstanceTypeFacts::of("c5.metal")
        .previous_generation()
        .ebs(19_000, 80_000, 19_000, 80_000)
        // A bare metal instance's volumes come out of the larger attachment pool the whole
        // host carries, and it has no hypervisor passing anything else through to draw on it,
        // so it holds the 31 volumes "Instance volume limits" publishes for one.
        .ebs_attachments(31)
        .network_bandwidth(25.0, 25.0)
        .ena_queues(C5_ENA_QUEUES_PER_INTERFACE)
        .processor("Intel", 3.6)
        .dedicated_hosts(false)
        .auto_recovery(true)
        // The packets of a session are copied by the host's own card, which a bare metal
        // instance still has, so Traffic Mirroring is offered on it even though the
        // hypervisor-backed rule would deny it.
        .traffic_mirroring(true)
        .host_features(true, false)
        .boot_modes(LEGACY_BIOS_BOOT_MODE),
];

/// The ENA queues each interface of a C5 instance is given by default
/// (`NetworkCardInfo$DefaultEnaQueueCountPerInterface`): the queue count of the adapter the
/// family's network card carries, which every size of it publishes whatever its vCPUs.
const C5_ENA_QUEUES_PER_INTERFACE: i32 = 8;
