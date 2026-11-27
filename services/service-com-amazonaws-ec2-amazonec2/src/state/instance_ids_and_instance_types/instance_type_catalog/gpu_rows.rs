// SPDX-License-Identifier: CC0-1.0

//! The published rows of the accelerated families whose accelerators are GPUs, read through
//! [`super::published_instance_type_facts`] like every other row of the catalog.
//!
//! What a GPU family publishes that no size multiplier yields: the accelerators themselves
//! (`InstanceTypeInfo$GpuInfo` — how many devices, which device and how much memory each
//! carries), the local NVMe devices the family ships with, and the host features an
//! accelerated type is NOT offered — CloudWatch action based recovery cannot carry the data of
//! a local instance store over, and the family is not offered on a Dedicated Host.
//!
//! The figures come from the family's own instance-type specification page (accelerators,
//! vCPUs, memory, local devices, network performance), the "Amazon EBS optimized instance
//! types" table (the four EBS figures) and "Instance volume limits" (how many volumes a type
//! holds attached and whether that limit is the volumes' own).

use super::*;

/// The `GpuDeviceInfo$Name` of the accelerator the sixth generation graphics families carry,
/// and its published memory in MiB: a 24 GB device.
const L4_ACCELERATOR: (&str, i32) = ("L4", 22_888);

/// The accelerator the sixth generation graphics family's `e` variant carries, and its
/// published memory in MiB: a 48 GB device, twice the one above.
const L40S_ACCELERATOR: (&str, i32) = ("L40S", 45_776);

/// The accelerator the fifth generation x86 graphics family carries, and its published memory
/// in MiB: the same 24 GB of the device above, this one the generation before it.
const A10G_ACCELERATOR: (&str, i32) = ("A10G", 22_888);

/// The accelerator the AWS Graviton graphics family carries, and its published memory in MiB:
/// a 16 GB device.
const T4G_ACCELERATOR: (&str, i32) = ("T4g", 16_384);

/// The `GpuDeviceInfo$Manufacturer` of every accelerator here.
const NVIDIA: &str = "NVIDIA";

/// The ENA queues each interface of a memory-rich graphics instance is given by default
/// (`NetworkCardInfo$DefaultEnaQueueCountPerInterface`): the queue count of the adapter that
/// family's network card carries, which both of its sizes publish whatever their vCPUs.
const GR6_ENA_QUEUES_PER_INTERFACE: i32 = 8;

/// The published hardware facts of the GPU families' instance types, one row per type.
pub(super) static GPU_INSTANCE_TYPE_FACTS: &[PublishedInstanceTypeFacts] = &[
    // Gr6 — the graphics family's memory-rich pair, 3rd generation AMD EPYC at a sustained
    // 3.6 GHz with ONE L4 accelerator whatever the size, 8 GiB of system memory per vCPU and
    // local NVMe devices whose count and size are the type's own: one 600 GB device on the
    // `4xlarge` and two 450 GB ones on the `8xlarge`. Its EBS volumes are attached to EBS
    // hardware of the type's own rather than out of the pool shared with its interfaces, so
    // the 32 attachments each size reports are the volumes' own; the `4xlarge` publishes
    // 8,000 Mbps and 32,000 IOPS and the `8xlarge` twice that. Neither size is offered on a
    // Dedicated Host, ENA Express is not offered on the family, and the Elastic Fabric Adapter
    // is offered on the larger size alone, which also has the whole 25 Gigabit of its share of
    // the host's network rather than bursting up to it. The smaller size sustains 10 Gbps of
    // that 25 — the figure published for it, which is neither its share of the host nor the
    // ceiling it bursts to — and each interface of either size is given the eight ENA queues
    // the family's adapter carries.
    PublishedInstanceTypeFacts::of("gr6.4xlarge")
        .ebs(8_000, 32_000, 8_000, 32_000)
        .dedicated_ebs_attachments(32)
        .instance_store(1, 600)
        .network("Up to 25 Gigabit")
        .network_bandwidth(10.0, 25.0)
        .ena_queues(GR6_ENA_QUEUES_PER_INTERFACE)
        .gpus(1, L4_ACCELERATOR.0, NVIDIA, L4_ACCELERATOR.1, ML_AI_WORKLOAD)
        .processor("AMD", 3.6)
        .dedicated_hosts(false)
        .ena_srd(false),
    PublishedInstanceTypeFacts::of("gr6.8xlarge")
        .ebs(16_000, 64_000, 16_000, 64_000)
        .dedicated_ebs_attachments(32)
        .instance_store(2, 450)
        .network("25 Gigabit")
        .ena_queues(GR6_ENA_QUEUES_PER_INTERFACE)
        .efa(1)
        .gpus(1, L4_ACCELERATOR.0, NVIDIA, L4_ACCELERATOR.1, ML_AI_WORKLOAD)
        .processor("AMD", 3.6)
        .dedicated_hosts(false)
        .ena_srd(false),
    // Gr6f — the memory-rich graphics family's fractional size: the same 3rd generation AMD
    // EPYC host and the same L4 accelerator, published with the `4xlarge`'s own 8,000 Mbps and
    // 32,000 IOPS of dedicated EBS bandwidth and one 450 GB local NVMe device rather than the
    // 600 GB one of the `gr6` size of that name. Its EBS volumes are attached to EBS hardware
    // of the type's own, so its 32 attachments are the volumes' own; it is not offered on a
    // Dedicated Host, ENA Express is not offered on the family, it is not migrated to another
    // host on a user-initiated reboot and carries no local clock, and it takes no interface
    // attachment from a secondary network, so it publishes none of the addresses such an
    // interface would take.
    PublishedInstanceTypeFacts::of("gr6f.4xlarge")
        .ebs(8_000, 32_000, 8_000, 32_000)
        .dedicated_ebs_attachments(32)
        .instance_store(1, 450)
        .gpus(1, L4_ACCELERATOR.0, NVIDIA, L4_ACCELERATOR.1, ML_AI_WORKLOAD)
        .processor("AMD", 3.6)
        .dedicated_hosts(false)
        .ena_srd(false)
        .host_features(false, false)
        .ipv4_per_secondary_interface(0),
    // G6 — the sixth generation graphics family: NVIDIA L4 accelerators on a 3rd generation
    // AMD EPYC at a sustained 3.4 GHz, with 4 GiB of system memory per vCPU (the general
    // purpose ratio its family letter gives, which is why no row below publishes its memory)
    // and local NVMe devices whose count and size are each size's own rather than a multiple
    // of the family's smallest. Its EBS volumes are attached to EBS hardware of the type's own
    // rather than out of the pool shared with its interfaces and local devices, so every size
    // publishes a limit of its own, and that limit grows with the share of the host the size
    // holds: a quarter of it and below publish 32, a third 48, a half 64 and the whole host
    // 128 — the three largest rungs of that ladder being DERIVED from it rather than read off
    // the published table, with the half-host rung confirmed by the sibling `g6e` family's
    // recorded figure for the same share. Every EBS figure of the family holds four I/O
    // operations per second for each Mbps of bandwidth, and the bandwidth is 500 Mbps per vCPU
    // up to the `8xlarge` and the host's own rate above it — dedicated from the `4xlarge` up,
    // while the two sizes below it burst above the share they sustain to 5,000 Mbps and the
    // 20,000 IOPS that go with it. The sustained figure of those two sizes is DERIVED from the
    // family's rate too: AWS publishes one per type and the recorded figures for these two are
    // not among the ones this catalog has.
    // The family is not offered on a Dedicated Host, no size of it is migrated to another host
    // on a user-initiated reboot and none carries a local clock; the Elastic Fabric Adapter is
    // offered from the size that holds a dedicated share of the host's network up, while ENA
    // Express is offered on the size that holds the whole host alone. No size takes an
    // interface attachment from a secondary network, so each publishes none of the addresses
    // such an interface would take.
    PublishedInstanceTypeFacts::of("g6.xlarge")
        .ebs(2_000, 8_000, 5_000, 20_000)
        .dedicated_ebs_attachments(32)
        .instance_store(1, 250)
        .network("Up to 10 Gigabit")
        .gpus(1, L4_ACCELERATOR.0, NVIDIA, L4_ACCELERATOR.1, ML_AI_WORKLOAD)
        .processor("AMD", 3.4)
        .dedicated_hosts(false)
        .ena_srd(false)
        .host_features(false, false)
        .ipv4_per_secondary_interface(0),
    PublishedInstanceTypeFacts::of("g6.2xlarge")
        .ebs(4_000, 16_000, 5_000, 20_000)
        .dedicated_ebs_attachments(32)
        .instance_store(1, 450)
        .network("Up to 10 Gigabit")
        .gpus(1, L4_ACCELERATOR.0, NVIDIA, L4_ACCELERATOR.1, ML_AI_WORKLOAD)
        .processor("AMD", 3.4)
        .dedicated_hosts(false)
        .ena_srd(false)
        .host_features(false, false)
        .ipv4_per_secondary_interface(0),
    PublishedInstanceTypeFacts::of("g6.4xlarge")
        .ebs(8_000, 32_000, 8_000, 32_000)
        .dedicated_ebs_attachments(32)
        .instance_store(1, 600)
        .network("Up to 25 Gigabit")
        .gpus(1, L4_ACCELERATOR.0, NVIDIA, L4_ACCELERATOR.1, ML_AI_WORKLOAD)
        .processor("AMD", 3.4)
        .dedicated_hosts(false)
        .ena_srd(false)
        .host_features(false, false)
        .ipv4_per_secondary_interface(0),
    PublishedInstanceTypeFacts::of("g6.8xlarge")
        .ebs(16_000, 64_000, 16_000, 64_000)
        .dedicated_ebs_attachments(32)
        .instance_store(2, 450)
        .network_bandwidth(25.0, 25.0)
        .efa(1)
        .gpus(1, L4_ACCELERATOR.0, NVIDIA, L4_ACCELERATOR.1, ML_AI_WORKLOAD)
        .processor("AMD", 3.4)
        .dedicated_hosts(false)
        .ena_srd(false)
        .host_features(false, false)
        .ipv4_per_secondary_interface(0),
    PublishedInstanceTypeFacts::of("g6.12xlarge")
        .ebs(20_000, 80_000, 20_000, 80_000)
        .dedicated_ebs_attachments(32)
        .instance_store(4, 940)
        .network_bandwidth(40.0, 40.0)
        .efa(1)
        .gpus(4, L4_ACCELERATOR.0, NVIDIA, L4_ACCELERATOR.1, ML_AI_WORKLOAD)
        .processor("AMD", 3.4)
        .dedicated_hosts(false)
        .ena_srd(false)
        .host_features(false, false)
        .ipv4_per_secondary_interface(0),
    PublishedInstanceTypeFacts::of("g6.16xlarge")
        .ebs(20_000, 80_000, 20_000, 80_000)
        // Half again the volumes of the quarter-host sizes: this size holds a THIRD of the
        // accelerated host's vCPUs, and its published limit sits between their 32 and the 64 of
        // the half-host size below. DERIVED from that ladder, not from the published table.
        .dedicated_ebs_attachments(48)
        .instance_store(2, 940)
        .network_bandwidth(25.0, 25.0)
        .efa(1)
        .gpus(1, L4_ACCELERATOR.0, NVIDIA, L4_ACCELERATOR.1, ML_AI_WORKLOAD)
        .processor("AMD", 3.4)
        .dedicated_hosts(false)
        .ena_srd(false)
        .host_features(false, false)
        .ipv4_per_secondary_interface(0),
    PublishedInstanceTypeFacts::of("g6.24xlarge")
        .ebs(30_000, 120_000, 30_000, 120_000)
        // Twice the volumes of the quarter-host sizes: this size holds half the accelerated
        // host, and the same share of the sibling `g6e` family — the one whose published figure
        // for it is recorded — publishes twice their 32.
        .dedicated_ebs_attachments(64)        .instance_store(4, 940)
        .network_bandwidth(50.0, 50.0)
        .efa(1)
        .gpus(4, L4_ACCELERATOR.0, NVIDIA, L4_ACCELERATOR.1, ML_AI_WORKLOAD)
        .processor("AMD", 3.4)
        .dedicated_hosts(false)
        .ena_srd(false)
        .host_features(false, false)
        .ipv4_per_secondary_interface(0),
    PublishedInstanceTypeFacts::of("g6.48xlarge")
        .ebs(60_000, 240_000, 60_000, 240_000)
        // The size that holds the WHOLE accelerated host carries the whole host's EBS
        // hardware, which is twice what the half-host size above publishes. DERIVED from that
        // ladder, not from the published table.
        .dedicated_ebs_attachments(128)
        .instance_store(8, 940)
        .network_bandwidth(100.0, 100.0)
        .efa(1)
        .gpus(8, L4_ACCELERATOR.0, NVIDIA, L4_ACCELERATOR.1, ML_AI_WORKLOAD)
        .processor("AMD", 3.4)
        .dedicated_hosts(false)
        // The whole host's adapter is this size's own, which is the hardware ENA Express runs
        // on, so this size alone of the family is offered it.
        .ena_srd(true)
        .host_features(false, false)
        .ipv4_per_secondary_interface(0),
    // G6e — the sixth generation graphics family's `e` variant: NVIDIA L40S accelerators of 48
    // GB on the same 3rd generation AMD EPYC the G6 family runs on and at the same sustained
    // 3.4 GHz, with 8 GiB of system memory per vCPU — TWICE the ratio its family letter gives,
    // which is why every row below publishes its memory. Its local NVMe devices are the
    // published ones, count and size each the type's own: one device up to the `4xlarge`, two
    // from the `8xlarge` up — of 450, 1,900 and 950 GB as the ladder climbs — and four of the
    // family's 1,900 GB device on the size that holds the whole host.
    // It reads its own rows of "Amazon EBS optimized instance types", four I/O operations per
    // second for each Mbps of bandwidth throughout: 500 Mbps per vCPU on the `4xlarge` and the
    // `8xlarge`, the host's own 312.5 Mbps per vCPU above them, and half the family's dedicated
    // rate on the two narrowest sizes, which burst above what they sustain to the 5,000 Mbps
    // their published column states. Its volumes are attached to EBS hardware of the type's own
    // rather than out of the pool shared with its interfaces and local devices, so every size
    // publishes a limit of its own that grows with the share of the host it holds: 32 up to a
    // quarter of it, 48 for a third, the recorded 64 for a half and 128 for the whole host, the
    // three rungs beside that recorded one being DERIVED from it the way the G6 family's are.
    // The network figure is the family's own published column, well above the general purpose
    // host's: the two narrowest sizes burst up to 20 Gigabit, and from the `4xlarge` up each
    // size holds a dedicated share — one adapter's 100 Gigabit on the `12xlarge`, and the two and
    // four adapters of the wider sizes together, which is why those two publish several network
    // cards. The Elastic Fabric Adapter is offered from the size that holds a dedicated share
    // up, ENA Express on the sizes that carry several of the host's accelerators and the adapter
    // that goes with them, and a Dedicated Host only on the two sizes that hold half the host's
    // accelerators or more. No size is migrated to another host on a user-initiated reboot and
    // none carries a local clock; none takes an interface attachment from a secondary network,
    // so each publishes none of the addresses such an interface would take. The cores of the
    // `8xlarge` sit on both of the host's processor sockets and are taken one per socket, so it
    // publishes the even counts alone, while a size wider still holds whole accelerators with
    // the vCPUs pinned to them and takes its cores ALL at once, so the one count it publishes is
    // its default; the narrower sizes publish every count.
    // The interface limits of the three sizes that hold more than a quarter of the host are NOT
    // recorded here: each publishes a figure of its own that its step of the "IP addresses per
    // network interface per instance type" ladder does not give, so those sizes keep the
    // ladder's figure until the published one is read in.
    PublishedInstanceTypeFacts::of("g6e.xlarge")
        .memory_gib(32)
        .ebs(1_000, 4_000, 5_000, 20_000)
        .dedicated_ebs_attachments(32)
        .instance_store(1, 250)
        .network("Up to 20 Gigabit")
        .gpus(
            1,
            L40S_ACCELERATOR.0,
            NVIDIA,
            L40S_ACCELERATOR.1,
            ML_AI_WORKLOAD,
        )
        .processor("AMD", 3.4)
        .dedicated_hosts(false)
        .ena_srd(false)
        .host_features(false, false)
        .ipv4_per_secondary_interface(0),
    PublishedInstanceTypeFacts::of("g6e.2xlarge")
        .memory_gib(64)
        .ebs(2_000, 8_000, 5_000, 20_000)
        .dedicated_ebs_attachments(32)
        .instance_store(1, 450)
        .network("Up to 20 Gigabit")
        .gpus(
            1,
            L40S_ACCELERATOR.0,
            NVIDIA,
            L40S_ACCELERATOR.1,
            ML_AI_WORKLOAD,
        )
        .processor("AMD", 3.4)
        .dedicated_hosts(false)
        .ena_srd(false)
        .host_features(false, false)
        .ipv4_per_secondary_interface(0),
    PublishedInstanceTypeFacts::of("g6e.4xlarge")
        .memory_gib(128)
        .ebs(8_000, 32_000, 8_000, 32_000)
        .dedicated_ebs_attachments(32)
        .instance_store(1, 600)
        .network_bandwidth(20.0, 20.0)
        .gpus(
            1,
            L40S_ACCELERATOR.0,
            NVIDIA,
            L40S_ACCELERATOR.1,
            ML_AI_WORKLOAD,
        )
        .processor("AMD", 3.4)
        .dedicated_hosts(false)
        .ena_srd(false)
        .host_features(false, false)
        .ipv4_per_secondary_interface(0),
    PublishedInstanceTypeFacts::of("g6e.8xlarge")
        .memory_gib(256)
        .cores_in_steps_of(2)
        .ebs(16_000, 64_000, 16_000, 64_000)
        .dedicated_ebs_attachments(32)
        .instance_store(2, 450)
        .network_bandwidth(25.0, 25.0)
        .efa(1)
        .gpus(
            1,
            L40S_ACCELERATOR.0,
            NVIDIA,
            L40S_ACCELERATOR.1,
            ML_AI_WORKLOAD,
        )
        .processor("AMD", 3.4)
        .dedicated_hosts(false)
        .ena_srd(false)
        .host_features(false, false)
        .ipv4_per_secondary_interface(0),
    PublishedInstanceTypeFacts::of("g6e.12xlarge")
        .memory_gib(384)
        .cores_all_at_once()
        .ebs(20_000, 80_000, 20_000, 80_000)
        .dedicated_ebs_attachments(32)
        .instance_store(2, 1_900)
        .network_bandwidth(100.0, 100.0)
        .efa(1)
        .gpus(
            4,
            L40S_ACCELERATOR.0,
            NVIDIA,
            L40S_ACCELERATOR.1,
            ML_AI_WORKLOAD,
        )
        .processor("AMD", 3.4)
        .dedicated_hosts(false)
        .ena_srd(true)
        .host_features(false, false)
        .ipv4_per_secondary_interface(0),
    PublishedInstanceTypeFacts::of("g6e.16xlarge")
        .memory_gib(512)
        .cores_all_at_once()
        .ebs(20_000, 80_000, 20_000, 80_000)
        // A third of the accelerated host, whose published limit sits between the 32 of the
        // quarter-host sizes and the 64 of the half-host one. DERIVED from that ladder.
        .dedicated_ebs_attachments(48)
        .instance_store(2, 950)
        .network_bandwidth(35.0, 35.0)
        .efa(1)
        .gpus(
            1,
            L40S_ACCELERATOR.0,
            NVIDIA,
            L40S_ACCELERATOR.1,
            ML_AI_WORKLOAD,
        )
        .processor("AMD", 3.4)
        .dedicated_hosts(false)
        .ena_srd(false)
        .host_features(false, false)
        .ipv4_per_secondary_interface(0),
    PublishedInstanceTypeFacts::of("g6e.24xlarge")
        .memory_gib(768)
        .cores_all_at_once()
        .ebs(30_000, 120_000, 30_000, 120_000)
        .dedicated_ebs_attachments(64)
        .instance_store(2, 1_900)
        .network_bandwidth(200.0, 200.0)
        .efa(1)
        .gpus(
            4,
            L40S_ACCELERATOR.0,
            NVIDIA,
            L40S_ACCELERATOR.1,
            ML_AI_WORKLOAD,
        )
        .processor("AMD", 3.4)
        .ena_srd(true)
        .host_features(false, false)
        .ipv4_per_secondary_interface(0),
    PublishedInstanceTypeFacts::of("g6e.48xlarge")
        .memory_gib(1_536)
        .cores_all_at_once()
        .ebs(60_000, 240_000, 60_000, 240_000)
        // The size that holds the WHOLE accelerated host carries the whole host's EBS hardware,
        // which is twice what the half-host size above publishes. DERIVED from that ladder.
        .dedicated_ebs_attachments(128)
        .instance_store(4, 1_900)
        .network_bandwidth(400.0, 400.0)
        .efa(1)
        .gpus(
            8,
            L40S_ACCELERATOR.0,
            NVIDIA,
            L40S_ACCELERATOR.1,
            ML_AI_WORKLOAD,
        )
        .processor("AMD", 3.4)
        // The four adapters of the whole host are this size's own, which is the hardware ENA
        // Express runs on.
        .ena_srd(true)
        .host_features(false, false)
        .ipv4_per_secondary_interface(0),
    // G5 — the fifth generation graphics family, NVIDIA A10G accelerators on a 2nd generation
    // AMD EPYC (7R32) at a sustained 3.3 GHz, 4 GiB of system memory per vCPU and ONE local
    // NVMe device at every size but the largest, whose size is the type's own rather than a
    // multiple of the family's `large`. It is NOT the latest generation of its family, and the
    // Dedicated Host it is offered on carries the whole host's accelerators, so only the two
    // sizes that hold four or more of them are offered one. Its adapter encrypts in-transit
    // traffic between instances, and the Elastic Fabric Adapter is offered from the size that
    // holds a dedicated share of the host's network up. Its EBS volumes come out of the shared
    // Nitro attachment pool, which the accelerators themselves also draw on, so the published
    // limit is below what the interfaces and the local devices alone would leave. The two
    // sizes with four accelerators and the one with eight take their interfaces from the
    // host's own share rather than their step of the size ladder. No size of the family is
    // migrated to another host on a user-initiated reboot, and none carries a local clock.
    // The family is NOT the latest generation of its family: the recorded catalog reports
    // every size of it with that flag false, the G ladder having moved past it, which is why
    // each row below says so and why the family is named among the previous generation ones
    // that are still offered IPv6 addresses per interface.
    PublishedInstanceTypeFacts::of("g5.xlarge")
        .previous_generation()
        .ebs(700, 3_000, 3_500, 15_000)
        .ebs_attachments(25)
        .instance_store(1, 250)
        .network("Up to 10 Gigabit")
        .gpus(
            1,
            A10G_ACCELERATOR.0,
            NVIDIA,
            A10G_ACCELERATOR.1,
            ML_AI_WORKLOAD,
        )
        .processor("AMD", 3.3)
        .encryption_in_transit(true)
        .host_features(false, false),
    PublishedInstanceTypeFacts::of("g5.2xlarge")
        .previous_generation()
        .ebs(850, 3_500, 3_500, 15_000)
        .ebs_attachments(25)
        .instance_store(1, 450)
        .network("Up to 10 Gigabit")
        .gpus(
            1,
            A10G_ACCELERATOR.0,
            NVIDIA,
            A10G_ACCELERATOR.1,
            ML_AI_WORKLOAD,
        )
        .processor("AMD", 3.3)
        .encryption_in_transit(true)
        .host_features(false, false),
    PublishedInstanceTypeFacts::of("g5.4xlarge")
        .previous_generation()
        .ebs(4_750, 20_000, 4_750, 20_000)
        .ebs_attachments(25)
        .instance_store(1, 600)
        .network("Up to 25 Gigabit")
        .gpus(
            1,
            A10G_ACCELERATOR.0,
            NVIDIA,
            A10G_ACCELERATOR.1,
            ML_AI_WORKLOAD,
        )
        .processor("AMD", 3.3)
        .encryption_in_transit(true)
        .host_features(false, false),
    PublishedInstanceTypeFacts::of("g5.8xlarge")
        .previous_generation()
        .ebs(16_000, 65_000, 16_000, 65_000)
        .ebs_attachments(25)
        .instance_store(1, 900)
        .network_bandwidth(25.0, 25.0)
        .efa(1)
        .gpus(
            1,
            A10G_ACCELERATOR.0,
            NVIDIA,
            A10G_ACCELERATOR.1,
            ML_AI_WORKLOAD,
        )
        .processor("AMD", 3.3)
        .encryption_in_transit(true)
        .host_features(false, false),
    PublishedInstanceTypeFacts::of("g5.12xlarge")
        .previous_generation()
        .ebs(16_000, 65_000, 16_000, 65_000)
        .ebs_attachments(22)
        .instance_store(1, 3_800)
        .network_bandwidth(40.0, 40.0)
        .interfaces(15)
        .ipv4_per_interface(50)
        .efa(1)
        .gpus(
            4,
            A10G_ACCELERATOR.0,
            NVIDIA,
            A10G_ACCELERATOR.1,
            ML_AI_WORKLOAD,
        )
        .processor("AMD", 3.3)
        .encryption_in_transit(true)
        .host_features(false, false),
    PublishedInstanceTypeFacts::of("g5.16xlarge")
        .previous_generation()
        .ebs(16_000, 65_000, 16_000, 65_000)
        .ebs_attachments(25)
        .instance_store(1, 1_900)
        .network_bandwidth(25.0, 25.0)
        .interfaces(8)
        .ipv4_per_interface(30)
        .efa(1)
        .gpus(
            1,
            A10G_ACCELERATOR.0,
            NVIDIA,
            A10G_ACCELERATOR.1,
            ML_AI_WORKLOAD,
        )
        .processor("AMD", 3.3)
        .encryption_in_transit(true)
        .host_features(false, false),
    PublishedInstanceTypeFacts::of("g5.24xlarge")
        .previous_generation()
        .ebs(19_000, 80_000, 19_000, 80_000)
        .ebs_attachments(22)
        .instance_store(1, 3_800)
        .network_bandwidth(50.0, 50.0)
        .efa(1)
        .gpus(
            4,
            A10G_ACCELERATOR.0,
            NVIDIA,
            A10G_ACCELERATOR.1,
            ML_AI_WORKLOAD,
        )
        .processor("AMD", 3.3)
        .dedicated_hosts(true)
        .encryption_in_transit(true)
        .host_features(false, false),
    PublishedInstanceTypeFacts::of("g5.48xlarge")
        .previous_generation()
        .ebs(19_000, 80_000, 19_000, 80_000)
        // Eight accelerators and two local devices, each drawing on the same pool the volumes
        // come out of, which is what leaves this size the fewest volumes of the family. This
        // is the family's own rule applied to this size — the rule the recorded figures of
        // every other size of this family and of the Graviton one below confirm — and NOT the
        // recorded figure for this one size, which that rule does not yield: AWS publishes a
        // limit of its own for the size that holds the whole accelerated host, and no figure
        // derivable from the other rows produces it.
        .ebs_attachments(17)
        .instance_store(2, 3_800)
        .network_bandwidth(100.0, 100.0)
        .interfaces(7)
        .efa(1)
        .gpus(
            8,
            A10G_ACCELERATOR.0,
            NVIDIA,
            A10G_ACCELERATOR.1,
            ML_AI_WORKLOAD,
        )
        .processor("AMD", 3.3)
        .dedicated_hosts(true)
        .encryption_in_transit(true)
        .host_features(false, false),
    // G5g — the graphics family on AWS Graviton2 at a sustained 2.5 GHz with NVIDIA T4g
    // accelerators: 2 GiB of system memory per vCPU, HALF the general purpose ratio its family
    // letter would give, and NO local instance store at any size, which is also why CloudWatch
    // action based recovery is offered on every virtualized size. Its two largest sizes carry
    // two accelerators where the rest carry one. The Dedicated Host carries the whole host, so
    // only the sizes that hold it are offered one. Its EBS volumes come out of the shared
    // Nitro attachment pool, which the accelerators draw on too.
    PublishedInstanceTypeFacts::of("g5g.xlarge")
        .memory_gib(8)
        .no_instance_store()
        .ebs(1_188, 6_000, 4_750, 20_000)
        .ebs_attachments(26)
        .network("Up to 10 Gigabit")
        .gpus(
            1,
            T4G_ACCELERATOR.0,
            NVIDIA,
            T4G_ACCELERATOR.1,
            ML_AI_WORKLOAD,
        )
        .processor("AWS", 2.5)
        .dedicated_hosts(false),
    PublishedInstanceTypeFacts::of("g5g.2xlarge")
        .memory_gib(16)
        .no_instance_store()
        .ebs(2_375, 12_000, 4_750, 20_000)
        .ebs_attachments(26)
        .network("Up to 10 Gigabit")
        .gpus(
            1,
            T4G_ACCELERATOR.0,
            NVIDIA,
            T4G_ACCELERATOR.1,
            ML_AI_WORKLOAD,
        )
        .processor("AWS", 2.5)
        .dedicated_hosts(false),
    PublishedInstanceTypeFacts::of("g5g.4xlarge")
        .memory_gib(32)
        .no_instance_store()
        .ebs(4_750, 20_000, 4_750, 20_000)
        .ebs_attachments(26)
        .network("Up to 10 Gigabit")
        .gpus(
            1,
            T4G_ACCELERATOR.0,
            NVIDIA,
            T4G_ACCELERATOR.1,
            ML_AI_WORKLOAD,
        )
        .processor("AWS", 2.5)
        .dedicated_hosts(false),
    PublishedInstanceTypeFacts::of("g5g.8xlarge")
        .memory_gib(64)
        .no_instance_store()
        .ebs(9_500, 40_000, 9_500, 40_000)
        .ebs_attachments(26)
        .network_bandwidth(12.0, 12.0)
        .gpus(
            1,
            T4G_ACCELERATOR.0,
            NVIDIA,
            T4G_ACCELERATOR.1,
            ML_AI_WORKLOAD,
        )
        .processor("AWS", 2.5)
        .dedicated_hosts(false),
    PublishedInstanceTypeFacts::of("g5g.16xlarge")
        .memory_gib(128)
        .no_instance_store()
        .ebs(19_000, 80_000, 19_000, 80_000)
        .ebs_attachments(25)
        .network_bandwidth(25.0, 25.0)
        .gpus(
            2,
            T4G_ACCELERATOR.0,
            NVIDIA,
            T4G_ACCELERATOR.1,
            ML_AI_WORKLOAD,
        )
        .processor("AWS", 2.5),
    PublishedInstanceTypeFacts::of("g5g.metal")
        .memory_gib(128)
        .no_instance_store()
        .ebs(19_000, 80_000, 19_000, 80_000)
        // A bare metal instance has no hypervisor passing an accelerator or a local device
        // through to it, so neither draws on the attachment pool its volumes come out of, and
        // the pool itself is the larger one the whole host carries.
        .ebs_attachments(31)
        .network_bandwidth(25.0, 25.0)
        // The packets of a session are copied by the host's own card, which a bare metal
        // instance still has, so this family's whole-host size is offered Traffic Mirroring.
        .traffic_mirroring(true)
        .gpus(
            2,
            T4G_ACCELERATOR.0,
            NVIDIA,
            T4G_ACCELERATOR.1,
            ML_AI_WORKLOAD,
        )
        .processor("AWS", 2.5)
        // The move to another host is the HOST's to make on a bare metal instance, which has
        // no hypervisor to make it, and there is no local clock on this hardware generation.
        .host_features(true, false),
];
