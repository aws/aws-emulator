// SPDX-License-Identifier: CC0-1.0

//! The published rows of the FIRST generation AWS Graviton general purpose family, read
//! through [`super::published_instance_type_facts`] like every other row of the catalog.
//!
//! The family sits on a 16-vCPU Graviton host whose EBS hardware is far narrower than the one
//! every later generation carries: its widest size sustains 3,500 Mbps and 20,000 IOPS, and
//! that figure is also the ceiling the smaller sizes burst to — so neither the baseline nor
//! the maximum of a size follows from the size ladder, and both are below what the derivation
//! over a name computes for a Nitro type. The sustained figures do not scale by the size
//! multiplier either: the `xlarge` sustains 800 Mbps where a quarter of the widest size's
//! figure would be 875.
//!
//! Its network adapter is a 10 Gigabit one, and that one figure is what every size of the
//! ladder publishes — the specification's "Network performance" column reads `Up to 10 Gigabit`
//! at the `medium` and still reads it at the `4xlarge` and the bare metal size, which hold the
//! WHOLE host. No share of a later generation's 25 Gigabit host is involved at any step: the
//! figure a size can reach is the adapter's whole 10 Gigabit at EVERY size, and the figure it
//! sustains is its own step of the published baseline ladder — half a Gigabit at the `medium`,
//! and doubling from the `xlarge` up without ever reaching the adapter's own figure, which is
//! why the column reads `Up to` at every size.
//!
//! Beyond those figures per size, the facts recorded here are the ones that do not follow
//! from the family's generation NUMBER, which is the oldest of any family on the Nitro System:
//! its volumes can be encrypted (the EBS encryption every Nitro type offers, which the
//! generation below the first encrypting x86 generation would otherwise answer `unsupported`),
//! it is offered on a Dedicated Host, and it is offered every placement group strategy — while
//! its hardware carries neither a virtual trusted platform module nor an enclave.
//!
//! The figures come from the family's own instance-type specification page (the network
//! performance column, the vCPUs of each size, and the processor and its sustained frequency),
//! the "Amazon EBS optimized instance types" table (the four EBS figures per size), the "IP
//! addresses per network interface per instance type" table, whose row for the smallest size
//! publishes fewer interfaces and addresses than that step of the generic size ladder gives,
//! and the published lists of the instance types offered Dedicated Hosts, NitroTPM and Nitro
//! Enclaves, each of which names this family among the types NOT offered the feature.

use super::*;

/// The bandwidth, in Mbps, every size of the family can reach: the figure its widest size
/// sustains, which is the one ceiling the whole ladder bursts to.
const A1_MAXIMUM_EBS_MBPS: i32 = 3_500;

/// The I/O operations per second every size of the family can reach, beside that bandwidth.
const A1_MAXIMUM_EBS_IOPS: i32 = 20_000;

/// The sustained all-core frequency, in GHz, of the first AWS Graviton processor the family is
/// built on.
const GRAVITON1_SUSTAINED_GHZ: f64 = 2.3;

/// The published hardware facts of the first generation AWS Graviton family's instance types,
/// one row per type.
pub(super) static FIRST_GENERATION_GRAVITON_INSTANCE_TYPE_FACTS: &[PublishedInstanceTypeFacts] = &[
    // A1 — general purpose on the first AWS Graviton processor at a sustained 2.3 GHz, with no
    // local instance store. The four EBS figures per size are its rows of "Amazon EBS
    // optimized instance types": a baseline per size, bursting to the 3,500 Mbps and 20,000
    // IOPS the two widest sizes sustain outright. Every size publishes the same network
    // performance column ([`A1_NETWORK_PERFORMANCE`]) and the same ceiling
    // ([`A1_ADAPTER_PEAK_GBPS`]); they differ only in the figure they sustain, which is the
    // ladder's own step rather than a share of a host — and which stays below the adapter's
    // figure even on the two sizes that hold every vCPU of the host, so the column reads `Up
    // to` there too. The smallest size takes TWO interfaces with four private addresses each,
    // where that step of the generic ladder publishes three interfaces and ten addresses.
    PublishedInstanceTypeFacts::of("a1.medium")
        .ebs(300, 2_500, A1_MAXIMUM_EBS_MBPS, A1_MAXIMUM_EBS_IOPS)
        .network(A1_NETWORK_PERFORMANCE)
        .network_bandwidth(0.5, A1_ADAPTER_PEAK_GBPS)
        .interfaces(2)
        .ipv4_per_interface(4)
        .processor("AWS", GRAVITON1_SUSTAINED_GHZ)
        .ebs_encryption(true)
        .dedicated_hosts(true)
        .nitro_enclaves(false)
        .nitro_tpm(false)
        .placement_strategies(EVERY_PLACEMENT_STRATEGY),
    PublishedInstanceTypeFacts::of("a1.large")
        .ebs(525, 4_000, A1_MAXIMUM_EBS_MBPS, A1_MAXIMUM_EBS_IOPS)
        .network(A1_NETWORK_PERFORMANCE)
        .network_bandwidth(0.75, A1_ADAPTER_PEAK_GBPS)
        .processor("AWS", GRAVITON1_SUSTAINED_GHZ)
        .ebs_encryption(true)
        .dedicated_hosts(true)
        .nitro_enclaves(false)
        .nitro_tpm(false)
        .placement_strategies(EVERY_PLACEMENT_STRATEGY),
    PublishedInstanceTypeFacts::of("a1.xlarge")
        .ebs(800, 6_000, A1_MAXIMUM_EBS_MBPS, A1_MAXIMUM_EBS_IOPS)
        .network(A1_NETWORK_PERFORMANCE)
        .network_bandwidth(1.25, A1_ADAPTER_PEAK_GBPS)
        .processor("AWS", GRAVITON1_SUSTAINED_GHZ)
        .ebs_encryption(true)
        .dedicated_hosts(true)
        .nitro_enclaves(false)
        .nitro_tpm(false)
        .placement_strategies(EVERY_PLACEMENT_STRATEGY),
    PublishedInstanceTypeFacts::of("a1.2xlarge")
        .ebs(1_750, 10_000, A1_MAXIMUM_EBS_MBPS, A1_MAXIMUM_EBS_IOPS)
        .network(A1_NETWORK_PERFORMANCE)
        .network_bandwidth(2.5, A1_ADAPTER_PEAK_GBPS)
        .processor("AWS", GRAVITON1_SUSTAINED_GHZ)
        .ebs_encryption(true)
        .dedicated_hosts(true)
        .nitro_enclaves(false)
        .nitro_tpm(false)
        .placement_strategies(EVERY_PLACEMENT_STRATEGY),
    // The two widest sizes have the host's whole EBS allocation, so their sustained figures ARE
    // the ceiling above.
    PublishedInstanceTypeFacts::of("a1.4xlarge")
        .ebs(
            A1_MAXIMUM_EBS_MBPS,
            A1_MAXIMUM_EBS_IOPS,
            A1_MAXIMUM_EBS_MBPS,
            A1_MAXIMUM_EBS_IOPS,
        )
        .network(A1_NETWORK_PERFORMANCE)
        .network_bandwidth(5.0, A1_ADAPTER_PEAK_GBPS)
        .processor("AWS", GRAVITON1_SUSTAINED_GHZ)
        .ebs_encryption(true)
        .dedicated_hosts(true)
        .nitro_enclaves(false)
        .nitro_tpm(false)
        .placement_strategies(EVERY_PLACEMENT_STRATEGY),
    PublishedInstanceTypeFacts::of("a1.metal")
        .ebs(
            A1_MAXIMUM_EBS_MBPS,
            A1_MAXIMUM_EBS_IOPS,
            A1_MAXIMUM_EBS_MBPS,
            A1_MAXIMUM_EBS_IOPS,
        )
        .network(A1_NETWORK_PERFORMANCE)
        .network_bandwidth(5.0, A1_ADAPTER_PEAK_GBPS)
        .processor("AWS", GRAVITON1_SUSTAINED_GHZ)
        .ebs_encryption(true)
        .dedicated_hosts(true)
        .nitro_enclaves(false)
        .nitro_tpm(false)
        .placement_strategies(EVERY_PLACEMENT_STRATEGY),
];

/// "The network performance" every size of the family publishes: the specification's column
/// reads the same at every step of the ladder, the whole family sharing one adapter figure.
const A1_NETWORK_PERFORMANCE: &str = "Up to 10 Gigabit";

/// The Gigabit the family's network adapter carries, which is the figure every size can
/// reach.
const A1_ADAPTER_PEAK_GBPS: f64 = 10.0;
