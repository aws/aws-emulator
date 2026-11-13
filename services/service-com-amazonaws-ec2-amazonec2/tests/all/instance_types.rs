// SPDX-License-Identifier: CC0-1.0

//! `DescribeInstanceTypes` end-to-end: what the catalog reports about each instance
//! type, and how a caller narrows and pages the types.
//!
//! Every call goes over the wire the service actually speaks, through the shared
//! `super::ec2_query` harness: an `application/x-www-form-urlencoded` ec2Query POST
//! dispatched through the same `MultiServiceRouter` the emulator serves with.
//!
//! The rules under test:
//!
//! 1. Every fact reported about an instance type follows from its NAME — the family, the
//!    generation, the capability letters and the size AWS's naming conventions define —
//!    read against the family's published ratios: two vCPUs per `large` unit of size,
//!    the family's memory-to-vCPU ratio, one thread per core on Graviton and Apple
//!    silicon and two on Intel and AMD, a `d` size or a storage optimized family
//!    carrying local devices, `nitro` from the fifth generation on and `xen` before it,
//!    and a bare metal size being the whole host its family's largest virtualized size
//!    is carved out of. So the rows below span families, generations, capability letters
//!    and sizes rather than one recorded type.
//! 2. The vCPU options a type offers are consistent with its default: its cores times
//!    its threads per core are its vCPUs, `ValidCores` runs from one core to that
//!    default, and `ValidThreadsPerCore` offers disabling multithreading exactly where
//!    there is a second thread to disable.
//! 3. Every instance type of the catalog `DescribeInstanceTypeOfferings` offers is
//!    described here, and described only once.
//! 4. `Filters` are ANDed with one another and ORed over each filter's values, with
//!    EC2's `*` and `?` wildcards; a filter the operation does not document is refused
//!    with `InvalidParameterValue`, and one naming a member this emulator reports absent
//!    compares against no value, so it selects nothing rather than everything.
//! 5. `MaxResults` and `NextToken` page the matching types: each page holds at most
//!    `MaxResults` items, the walk reports every matching type exactly once, and the
//!    last page carries no token. A token this service did not mint is refused with
//!    `InvalidPaginationToken`, and `MaxResults` outside the modelled 5..100 range is
//!    refused at the boundary — as is an instance type outside the modelled enum.
//! 6. `DryRun` performs nothing and answers 412 `DryRunOperation`.
//! 7. The describe is a read: repeating it answers the same types.
//!
//! Type ORDER within a response is deliberately not asserted — EC2 says the order of
//! elements in a response might vary — so every comparison here is over a sorted set.
//! What the pagination test relies on is that the order is STABLE across calls, which is
//! what makes a cursor token see each type exactly once.

use std::collections::BTreeSet;

use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, elements, error_code, filters, indexed, unescaped};

/// One `DescribeInstanceTypes` call signed for `region` and carrying `params`, as its
/// HTTP status and XML body.
async fn describe(
    router: &MultiServiceRouter,
    region: &str,
    params: &[(String, String)],
) -> (u16, String) {
    super::ec2_query::call(router, region, "DescribeInstanceTypes", params).await
}

/// The `InstanceTypes` form fields naming one instance type.
fn named(instance_type: &str) -> Vec<(String, String)> {
    indexed("InstanceType", &[instance_type])
}

/// Every instance type `<item>` of a `DescribeInstanceTypes` body, in the order the
/// response carried them.
///
/// The list members INSIDE one instance type (`<supportedUsageClasses>`, `<validCores>`,
/// …) are `<item>` elements too, so the split is depth aware: only the items directly
/// under `<instanceTypeSet>` are instance types. The list's first element also carries an
/// `xmlns` attribute, so an opening tag is read up to its `>`.
fn type_items(body: &str) -> Vec<String> {
    let list = element(body, "instanceTypeSet").unwrap_or_default();
    const CLOSE: &str = "</item>";
    let mut items: Vec<String> = Vec::new();
    let mut depth = 0usize;
    let mut opened_at = 0usize;
    let mut index = 0usize;
    while index < list.len() {
        let tail = &list[index..];
        let open = tail.find("<item");
        let close = tail.find(CLOSE);
        match (open, close) {
            (Some(open_offset), Some(close_offset)) if open_offset < close_offset => {
                let tag_at = index + open_offset;
                let after_tag = list[tag_at..]
                    .find('>')
                    .map_or(list.len(), |end| tag_at + end + 1);
                if depth == 0 {
                    opened_at = after_tag;
                }
                depth += 1;
                index = after_tag;
            }
            (_, Some(close_offset)) => {
                let close_at = index + close_offset;
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    items.push(list[opened_at..close_at].to_string());
                }
                index = close_at + CLOSE.len();
            }
            _ => break,
        }
    }
    items
}

/// The one instance type a request that named one reports, as its `<item>` body.
async fn describe_one(router: &MultiServiceRouter, case: &str, instance_type: &str) -> String {
    let (status, body) = describe(router, "us-east-1", &named(instance_type)).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 for {instance_type}, got {status} {body}"
    );
    let items = type_items(&body);
    assert_eq!(
        items.len(),
        1,
        "case {case:?}: naming {instance_type} reports exactly that type, got {body}"
    );
    items.into_iter().next().unwrap_or_default()
}

/// The text of the `name` member of one instance type item.
fn member(case: &str, item: &str, name: &str) -> String {
    element(item, name).unwrap_or_else(|| {
        panic!("case {case:?}: no <{name}> in the reported instance type {item:?}")
    })
}

/// The `name` member read as an integer.
fn integer(case: &str, item: &str, name: &str) -> i64 {
    let text = member(case, item, name);
    text.parse().unwrap_or_else(|_| {
        panic!("case {case:?}: <{name}> should be an integer, got {text:?}")
    })
}

/// Every `<item>` of a list member, as integers.
fn integer_list(case: &str, item: &str, structure: &str, name: &str) -> Vec<i64> {
    let inner = element(item, structure).unwrap_or_else(|| {
        panic!("case {case:?}: no <{structure}> in the reported instance type {item:?}")
    });
    let list = element(&inner, name)
        .unwrap_or_else(|| panic!("case {case:?}: no <{name}> in {inner:?}"));
    elements(&list, "item")
        .iter()
        .map(|value| {
            value
                .parse()
                .unwrap_or_else(|_| panic!("case {case:?}: <{name}> holds {value:?}"))
        })
        .collect()
}

/// Every instance type name a request reports over a whole `NextToken` walk, with the
/// number of pages it took. Each page is asserted to hold at most `page_size` types when
/// the request names one.
async fn walk(
    router: &MultiServiceRouter,
    case: &str,
    params: &[(String, String)],
    page_size: Option<usize>,
) -> (Vec<String>, usize) {
    let mut seen: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0usize;
    loop {
        let mut call_params = params.to_vec();
        if let Some(page_size) = page_size {
            call_params.push(("MaxResults".to_string(), page_size.to_string()));
        }
        if let Some(token) = &token {
            call_params.push(("NextToken".to_string(), token.clone()));
        }
        let (status, body) = describe(router, "us-east-1", &call_params).await;
        assert_eq!(
            status,
            200,
            "case {case:?}: expected 200 on page {}, got {status} {body}",
            pages + 1
        );
        let page = type_items(&body);
        if let Some(page_size) = page_size {
            assert!(
                page.len() <= page_size,
                "case {case:?}: a page holds at most {page_size} types, got {}",
                page.len()
            );
        }
        for item in &page {
            seen.push(member(case, item, "instanceType"));
        }
        pages += 1;
        token = element(&body, "nextToken");
        if token.is_none() {
            break;
        }
        assert!(
            pages < 64,
            "case {case:?}: the walk should end, got {pages} pages"
        );
    }
    (seen, pages)
}

/// One row of the facts table: the case, the instance type, its vCPUs, cores, threads
/// per core and memory in MiB, whether it is bare metal, its hypervisor (none when it
/// is), and whether it is burstable, can hibernate and has local instance storage.
type FactsRow = (
    &'static str,
    &'static str,
    i64,
    i64,
    i64,
    i64,
    bool,
    Option<&'static str>,
    bool,
    bool,
    bool,
);

/// One size of a family's ladder: the size, and its size in sixteenths of a `large`.
type LadderSize = (&'static str, i64);

/// One row of the ladder table: the family, its MiB of memory per vCPU, and its sizes.
type LadderRow = (&'static str, i64, &'static [LadderSize]);

/// One row of the filter table: the case, the `Filters` form fields, the member every
/// reported type must hold, and the values it may hold (empty when the row checks
/// something else).
type FilterRow = (
    &'static str,
    Vec<(String, String)>,
    &'static str,
    &'static [&'static str],
);

// ---------------------------------------------------------------------------
// Rule 1: every fact follows from the instance type's name
// ---------------------------------------------------------------------------

/// What one instance type reports about its CPU, memory, virtualization and local
/// storage.
///
/// The rows span the whole shape of a type name: the burstable families (current and
/// previous generation), general purpose, compute optimized, memory optimized, storage
/// optimized, HPC and High Memory families, Intel, AMD-era, Graviton and Apple silicon
/// processors, the `d` capability letter, a `-flex` sub-model, the sizes from `nano` to
/// `112xlarge`, and both bare metal forms (`metal` and `metal-48xl`).
///
/// A family whose published memory-to-vCPU ratio differs from its family letter's has
/// its own entry in the catalog's ratio table, which is why the `i3.large` row expects
/// the published 15.25 GiB rather than its letter's 16 (see
/// `crate::state::memory_mib_per_size_unit`).
#[tokio::test]
async fn the_reported_facts_follow_the_instance_type_name() {
    let (router, _engine) = super::ec2_query::emulator();

    // (case, instance type, vCPUs, cores, threads per core, memory MiB, bare metal,
    //  hypervisor, burstable, hibernation, instance storage)
    let cases: [FactsRow; 20] = [
        (
            "burstable nano",
            "t3.nano",
            2,
            1,
            2,
            512,
            false,
            Some("nitro"),
            true,
            true,
            false,
        ),
        (
            "previous generation burstable",
            "t2.small",
            1,
            1,
            1,
            2_048,
            false,
            Some("xen"),
            true,
            true,
            false,
        ),
        (
            "general purpose large",
            "m5.large",
            2,
            1,
            2,
            8_192,
            false,
            Some("nitro"),
            false,
            true,
            false,
        ),
        (
            "general purpose xlarge",
            "m5.xlarge",
            4,
            2,
            2,
            16_384,
            false,
            Some("nitro"),
            false,
            true,
            false,
        ),
        (
            "general purpose 24xlarge, too much RAM to hibernate",
            "m5.24xlarge",
            96,
            48,
            2,
            393_216,
            false,
            Some("nitro"),
            false,
            false,
            false,
        ),
        (
            "bare metal whole host",
            "m5.metal",
            96,
            48,
            2,
            393_216,
            true,
            None,
            false,
            false,
            false,
        ),
        (
            "sized bare metal",
            "c7i.metal-48xl",
            192,
            96,
            2,
            393_216,
            true,
            None,
            false,
            false,
            false,
        ),
        (
            "compute optimized large",
            "c5.large",
            2,
            1,
            2,
            4_096,
            false,
            Some("nitro"),
            false,
            true,
            false,
        ),
        (
            "graviton compute optimized medium",
            "c6g.medium",
            1,
            1,
            1,
            2_048,
            false,
            Some("nitro"),
            false,
            true,
            false,
        ),
        (
            "memory optimized 2xlarge",
            "r5.2xlarge",
            8,
            4,
            2,
            65_536,
            false,
            Some("nitro"),
            false,
            true,
            false,
        ),
        (
            "graviton memory optimized medium with local storage",
            "x2gd.medium",
            1,
            1,
            1,
            16_384,
            false,
            Some("nitro"),
            false,
            false,
            true,
        ),
        (
            "the d capability letter, whose local storage does not stop it hibernating",
            "m5d.large",
            2,
            1,
            2,
            8_192,
            false,
            Some("nitro"),
            false,
            true,
            true,
        ),
        (
            "storage optimized, pre-Nitro",
            "i3.large",
            2,
            1,
            2,
            15_616,
            false,
            Some("xen"),
            false,
            false,
            true,
        ),
        (
            "graviton general purpose",
            "m7g.large",
            2,
            2,
            1,
            8_192,
            false,
            Some("nitro"),
            false,
            true,
            false,
        ),
        (
            "a -flex sub-model",
            "m7i-flex.large",
            2,
            1,
            2,
            8_192,
            false,
            Some("nitro"),
            false,
            true,
            false,
        ),
        (
            "HPC, multithreading disabled",
            "hpc7g.16xlarge",
            64,
            64,
            1,
            131_072,
            false,
            Some("nitro"),
            false,
            false,
            false,
        ),
        (
            "HPC on hardware whose second thread is disabled",
            "hpc6a.48xlarge",
            96,
            96,
            1,
            393_216,
            false,
            Some("nitro"),
            false,
            false,
            false,
        ),
        (
            "High Memory, memory stated by the name",
            "u-6tb1.112xlarge",
            448,
            224,
            2,
            6_291_456,
            false,
            Some("nitro"),
            false,
            false,
            false,
        ),
        (
            "an Intel Mac host, whose cores are hyperthreaded",
            "mac1.metal",
            12,
            6,
            2,
            32_768,
            true,
            None,
            false,
            false,
            false,
        ),
        (
            "an Apple silicon Mac host",
            "mac2.metal",
            8,
            8,
            1,
            16_384,
            true,
            None,
            false,
            false,
            false,
        ),
    ];

    for (
        case,
        instance_type,
        vcpus,
        cores,
        threads,
        memory_mib,
        bare_metal,
        hypervisor,
        burstable,
        hibernation,
        instance_storage,
    ) in cases
    {
        let item = describe_one(&router, case, instance_type).await;
        assert_eq!(
            member(case, &item, "instanceType"),
            instance_type,
            "case {case:?}: the reported type must be the one named, got {item:?}"
        );
        for (name, expected) in [
            ("defaultVCpus", vcpus),
            ("defaultCores", cores),
            ("defaultThreadsPerCore", threads),
        ] {
            let inner = element(&item, "vCpuInfo").unwrap_or_default();
            let actual = integer(case, &inner, name);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {name} {expected} for {instance_type}, got {actual}"
            );
        }
        let actual_memory = integer(case, &element(&item, "memoryInfo").unwrap_or_default(), "sizeInMiB");
        assert_eq!(
            actual_memory, memory_mib,
            "case {case:?}: expected {memory_mib} MiB for {instance_type}, got {actual_memory}"
        );
        for (name, expected) in [
            ("bareMetal", bare_metal),
            ("burstablePerformanceSupported", burstable),
            ("hibernationSupported", hibernation),
            ("instanceStorageSupported", instance_storage),
        ] {
            let actual = member(case, &item, name);
            assert_eq!(
                actual,
                expected.to_string(),
                "case {case:?}: expected {name} {expected} for {instance_type}, got {actual:?}"
            );
        }
        // "The hypervisor for the instance type": a bare metal instance runs the
        // caller's own software on the host, with no hypervisor under it to report.
        let actual_hypervisor = element(&item, "hypervisor");
        assert_eq!(
            actual_hypervisor.as_deref(),
            hypervisor,
            "case {case:?}: expected hypervisor {hypervisor:?} for {instance_type}, \
             got {actual_hypervisor:?}"
        );
        // A type with local devices reports them, and only such a type does.
        let storage = element(&item, "instanceStorageInfo");
        assert_eq!(
            storage.is_some(),
            instance_storage,
            "case {case:?}: instanceStorageInfo is reported exactly for a type with \
             local storage, got {storage:?}"
        );
        if instance_storage {
            let total = integer(case, &item, "totalSizeInGB");
            let disks = element(&item, "disks").unwrap_or_default();
            let count = integer(case, &disks, "count");
            let size = integer(case, &disks, "sizeInGB");
            assert!(
                total > 0 && count > 0 && size * count == total,
                "case {case:?}: {instance_type}'s {count} disks of {size} GB must sum \
                 to its {total} GB total"
            );
        }
    }
}

/// A bare metal size is the WHOLE host the family's virtualized sizes are carved out of,
/// so it reports the vCPUs and memory of that family's largest virtualized size — and no
/// hypervisor, since there is none between the caller and the hardware.
///
/// The rows differ in where the family's ladder ends: at `24xlarge`, at `16xlarge` and at
/// `12xlarge`.
#[tokio::test]
async fn a_bare_metal_size_reports_the_whole_host() {
    let (router, _engine) = super::ec2_query::emulator();
    let cases: [(&str, &str, &str); 4] = [
        ("general purpose", "m5.metal", "m5.24xlarge"),
        ("compute optimized", "c5.metal", "c5.24xlarge"),
        ("graviton", "c6g.metal", "c6g.16xlarge"),
        ("high frequency", "z1d.metal", "z1d.12xlarge"),
    ];

    for (case, metal, largest) in cases {
        let bare = describe_one(&router, case, metal).await;
        let virtualized = describe_one(&router, case, largest).await;
        for name in ["defaultVCpus", "defaultCores", "defaultThreadsPerCore"] {
            let bare_value = integer(case, &element(&bare, "vCpuInfo").unwrap_or_default(), name);
            let virtual_value =
                integer(case, &element(&virtualized, "vCpuInfo").unwrap_or_default(), name);
            assert_eq!(
                bare_value, virtual_value,
                "case {case:?}: {metal} must report the {name} of {largest}, got \
                 {bare_value} and {virtual_value}"
            );
        }
        let bare_memory = integer(case, &element(&bare, "memoryInfo").unwrap_or_default(), "sizeInMiB");
        let virtual_memory = integer(
            case,
            &element(&virtualized, "memoryInfo").unwrap_or_default(),
            "sizeInMiB",
        );
        assert_eq!(
            bare_memory, virtual_memory,
            "case {case:?}: {metal} must report the memory of {largest}, got \
             {bare_memory} and {virtual_memory} MiB"
        );
        assert_eq!(
            member(case, &bare, "bareMetal"),
            "true",
            "case {case:?}: {metal} is a bare metal instance type"
        );
        assert!(
            element(&bare, "hypervisor").is_none(),
            "case {case:?}: {metal} reports no hypervisor, got {bare:?}"
        );
    }
}

/// Along one family's size ladder, the vCPUs and the memory scale with the size: two
/// vCPUs per `large` unit, and the family's own memory-to-vCPU ratio at every size.
///
/// The rows are families with different ratios — 4 GiB per vCPU for general purpose, 2
/// for compute optimized, 8 for memory optimized and 16 for the `x` family — and each
/// row's sizes span the ladder from a fraction of a `large` up to `16xlarge`.
#[tokio::test]
async fn memory_and_vcpus_follow_the_size_ladder() {
    let (router, _engine) = super::ec2_query::emulator();
    // (family, MiB of memory per vCPU, [(size, size in sixteenths of a `large`)])
    let cases: [LadderRow; 5] = [
        (
            "m5",
            4_096,
            &[
                ("large", 16),
                ("xlarge", 32),
                ("2xlarge", 64),
                ("4xlarge", 128),
                ("8xlarge", 256),
                ("16xlarge", 512),
            ],
        ),
        (
            "c5",
            2_048,
            &[("large", 16), ("xlarge", 32), ("4xlarge", 128), ("9xlarge", 288)],
        ),
        (
            "r5",
            8_192,
            &[("large", 16), ("2xlarge", 64), ("12xlarge", 384)],
        ),
        (
            "m6g",
            4_096,
            &[("medium", 8), ("large", 16), ("4xlarge", 128)],
        ),
        (
            "x2gd",
            16_384,
            &[("medium", 8), ("large", 16), ("8xlarge", 256)],
        ),
    ];

    for (family, mib_per_vcpu, ladder) in cases {
        for (size, sixteenths) in ladder {
            let instance_type = format!("{family}.{size}");
            let case = format!("{instance_type} at {sixteenths}/16 of a large");
            let item = describe_one(&router, &case, &instance_type).await;
            let vcpus = integer(&case, &element(&item, "vCpuInfo").unwrap_or_default(), "defaultVCpus");
            let memory = integer(&case, &element(&item, "memoryInfo").unwrap_or_default(), "sizeInMiB");
            // Two vCPUs per `large` unit, floored at the one vCPU a `medium` has.
            let expected_vcpus = (2 * sixteenths / 16).max(1);
            assert_eq!(
                vcpus, expected_vcpus,
                "case {case:?}: expected {expected_vcpus} vCPUs, got {vcpus}"
            );
            // The family's own ratio, at every size of its ladder.
            let expected_memory = mib_per_vcpu * expected_vcpus;
            assert_eq!(
                memory, expected_memory,
                "case {case:?}: expected {expected_memory} MiB, got {memory}"
            );
        }
    }
}

/// RULE: the size component of an instance type name is read into sixteenths of a `large`
/// only where it names a size AWS could offer — the ladder words, and a multiple of
/// `xlarge` up to the ceiling the module publishes. Anything else, including a multiple
/// above that ceiling, is answered `None`, so no caller-supplied multiple is multiplied out
/// (an unbounded one would overflow the conversion rather than report a size).
///
/// The rows are the ladder's own words, the largest size AWS publishes (`112xlarge`, in
/// `u7i-12tb.112xlarge`), the module's ceiling and the step past it, and the multiples no
/// arithmetic may be done on: `u32::MAX`, a value above it, and the bare metal form of each.
#[test]
fn a_size_component_is_read_into_sixteenths_only_within_the_published_ladder() {
    let ceiling = ec2::state::MAX_SIZE_MULTIPLE;
    let past = ceiling + 1;
    // (case, the size component, the sixteenths it reads as)
    let cases: Vec<(String, String, Option<u32>)> = vec![
        ("a nano".to_string(), "nano".to_string(), Some(1)),
        ("a medium".to_string(), "medium".to_string(), Some(8)),
        ("a large".to_string(), "large".to_string(), Some(16)),
        ("an xlarge".to_string(), "xlarge".to_string(), Some(32)),
        ("a 2xlarge".to_string(), "2xlarge".to_string(), Some(64)),
        (
            "a 112xlarge".to_string(),
            "112xlarge".to_string(),
            Some(112 * 32),
        ),
        (
            "the ceiling".to_string(),
            format!("{ceiling}xlarge"),
            Some(ceiling * 32),
        ),
        (
            "the ceiling, bare metal".to_string(),
            format!("metal-{ceiling}xl"),
            Some(ceiling * 32),
        ),
        (
            "one past the ceiling".to_string(),
            format!("{past}xlarge"),
            None,
        ),
        (
            "one past the ceiling, bare metal".to_string(),
            format!("metal-{past}xl"),
            None,
        ),
        (
            "every bit of a u32".to_string(),
            format!("{}xlarge", u32::MAX),
            None,
        ),
        (
            "a multiple above a u32".to_string(),
            format!("{}xlarge", u64::from(u32::MAX) + 1),
            None,
        ),
        // A plain `metal` names the whole host, whose size the name does not give.
        ("a plain metal".to_string(), "metal".to_string(), None),
        ("not a size at all".to_string(), "enormous".to_string(), None),
    ];

    for (case, size, expected) in cases {
        let read = ec2::state::instance_size_sixteenths(&size);
        assert_eq!(
            read, expected,
            "case {case:?}: expected {size:?} to read as {expected:?} sixteenths, got {read:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 2: the vCPU options are consistent with the default
// ---------------------------------------------------------------------------

/// The vCPU options one type offers are consistent with its default: its cores times its
/// threads per core are its vCPUs, `ValidCores` runs from one core up to that default,
/// and `ValidThreadsPerCore` offers one thread per core only where there is a second
/// thread to disable.
///
/// The rows span single-vCPU, single-thread (Graviton, Apple silicon) and two-thread
/// (Intel, AMD-era) types, at sizes from `nano` to `metal`.
#[tokio::test]
async fn the_vcpu_options_are_consistent_with_the_default() {
    let (router, _engine) = super::ec2_query::emulator();
    let cases = [
        "t3.nano",
        "t2.micro",
        "m5.large",
        "m5.xlarge",
        "m5.4xlarge",
        "m5.metal",
        "c6g.medium",
        "m7g.large",
        "mac1.metal",
        "hpc7g.16xlarge",
    ];

    for instance_type in cases {
        let case = format!("{instance_type} vCPU options");
        let item = describe_one(&router, &case, instance_type).await;
        let vcpu_info = element(&item, "vCpuInfo").unwrap_or_default();
        let vcpus = integer(&case, &vcpu_info, "defaultVCpus");
        let cores = integer(&case, &vcpu_info, "defaultCores");
        let threads = integer(&case, &vcpu_info, "defaultThreadsPerCore");
        assert_eq!(
            cores * threads,
            vcpus,
            "case {case:?}: {cores} cores of {threads} threads must be the {vcpus} vCPUs"
        );
        let valid_cores = integer_list(&case, &item, "vCpuInfo", "validCores");
        assert_eq!(
            valid_cores,
            (1..=cores).collect::<Vec<i64>>(),
            "case {case:?}: valid cores run from one to the default of {cores}"
        );
        let valid_threads = integer_list(&case, &item, "vCpuInfo", "validThreadsPerCore");
        assert_eq!(
            valid_threads,
            (1..=threads).collect::<Vec<i64>>(),
            "case {case:?}: valid threads per core run from one to the default of {threads}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 3: the catalog is described whole, and consistently with the offerings
// ---------------------------------------------------------------------------

/// Every instance type of the modelled catalog is described exactly once, and the types
/// described are the same ones `DescribeInstanceTypeOfferings` reports as offered.
#[tokio::test]
async fn every_catalogued_instance_type_is_described_once() {
    let (router, _engine) = super::ec2_query::emulator();
    let catalog: BTreeSet<String> = ec2::ops::types::INSTANCE_TYPE_VALUES
        .iter()
        .map(|value| (*value).to_string())
        .collect();
    assert!(
        catalog.len() > 100,
        "case catalog: the modelled InstanceType enum should carry the whole fleet, got {}",
        catalog.len()
    );

    let (walked, pages) = walk(&router, "whole catalog", &[], Some(100)).await;
    assert!(
        pages > 1,
        "case whole catalog: {} types in pages of 100 take several pages, got {pages}",
        catalog.len()
    );
    let described: BTreeSet<String> = walked.iter().cloned().collect();
    assert_eq!(
        described.len(),
        walked.len(),
        "case whole catalog: a type is described once, got {} of {} unique",
        walked.len(),
        described.len()
    );
    assert_eq!(
        described, catalog,
        "case whole catalog: every catalogued type is described, and only those"
    );

    // The same narrowing asked of both operations must answer the same set of types:
    // one catalog serves the describe and the offerings.
    let narrowing = filters(&[("instance-type", &["m5*"])]);
    let (described_family, _pages) = walk(&router, "m5 family", &narrowing, Some(100)).await;
    let (status, body) = super::ec2_query::call(
        &router,
        "us-east-1",
        "DescribeInstanceTypeOfferings",
        &narrowing,
    )
    .await;
    assert_eq!(
        status, 200,
        "case m5 family: expected 200 from the offerings, got {status} {body}"
    );
    let offered: BTreeSet<String> = elements(
        &element(&body, "instanceTypeOfferingSet").unwrap_or_default(),
        "instanceType",
    )
    .into_iter()
    .collect();
    assert_eq!(
        described_family.iter().cloned().collect::<BTreeSet<String>>(),
        offered,
        "case m5 family: the types described must be the types offered"
    );
}

// ---------------------------------------------------------------------------
// Rule 4: Filters
// ---------------------------------------------------------------------------

/// Each documented filter narrows the catalog to the types that hold its value, filters
/// are ANDed with one another, and a filter's values are ORed.
///
/// The rows span a filter on the type's own name (with a wildcard), on a boolean member,
/// on a numeric member, on a value of a list member, on a member of a nested structure,
/// two filters ANDed, and one filter with two values ORed.
#[tokio::test]
async fn filters_narrow_the_described_types() {
    let (router, _engine) = super::ec2_query::emulator();
    // (case, filters, the member every reported type must hold, the values it may hold)
    let cases: [FilterRow; 8] = [
        (
            "instance-type with a wildcard",
            filters(&[("instance-type", &["m5.*"])]),
            "instanceType",
            &[],
        ),
        (
            "bare-metal",
            filters(&[("bare-metal", &["true"])]),
            "bareMetal",
            &["true"],
        ),
        (
            "burstable-performance-supported",
            filters(&[("burstable-performance-supported", &["true"])]),
            "burstablePerformanceSupported",
            &["true"],
        ),
        (
            "hibernation-supported",
            filters(&[("hibernation-supported", &["true"])]),
            "hibernationSupported",
            &["true"],
        ),
        (
            "a numeric member",
            filters(&[("vcpu-info.default-vcpus", &["2"])]),
            "defaultVCpus",
            &["2"],
        ),
        (
            "a list member's value",
            filters(&[("processor-info.supported-architecture", &["arm64"])]),
            "manufacturer",
            &["AWS"],
        ),
        (
            "two filters ANDed",
            filters(&[
                ("instance-type", &["c6g.*"]),
                ("vcpu-info.default-vcpus", &["1"]),
            ]),
            "instanceType",
            &["c6g.medium"],
        ),
        (
            "one filter's values ORed",
            filters(&[("instance-type", &["m5.large", "t3.nano"])]),
            "instanceType",
            &["m5.large", "t3.nano"],
        ),
    ];

    for (case, params, name, allowed) in cases {
        let (status, body) = describe(&router, "us-east-1", &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {body}"
        );
        let items = type_items(&body);
        assert!(
            !items.is_empty(),
            "case {case:?}: the filter should select some type, got none"
        );
        for item in &items {
            let instance_type = member(case, item, "instanceType");
            let held = member(case, item, name);
            if !allowed.is_empty() {
                assert!(
                    allowed.contains(&held.as_str()),
                    "case {case:?}: {instance_type} reports {name} {held:?}, not one of \
                     {allowed:?}"
                );
            }
            if case.contains("wildcard") {
                assert!(
                    instance_type.starts_with("m5."),
                    "case {case:?}: m5.* selects only the m5 sizes, got {instance_type}"
                );
            }
        }
        if allowed.len() > 1 {
            let reported: BTreeSet<String> = items
                .iter()
                .map(|item| member(case, item, "instanceType"))
                .collect();
            let wanted: BTreeSet<String> =
                allowed.iter().map(|value| (*value).to_string()).collect();
            if name == "instanceType" {
                assert_eq!(
                    reported, wanted,
                    "case {case:?}: every ORed value must be reported"
                );
            }
        }
    }
}

/// A filter name this operation does not document is refused with HTTP 400
/// `InvalidParameterValue` naming the filter, rather than ignored or answered with an
/// empty list.
///
/// The rows differ in how the name is wrong: a member name instead of a filter name, a
/// near-miss plural, a filter of another operation, an unanchored prefix, and no name at
/// all.
#[tokio::test]
async fn an_undocumented_filter_is_refused() {
    let (router, _engine) = super::ec2_query::emulator();
    let cases = [
        ("a member name", "instanceType"),
        ("a near-miss plural", "instance-types"),
        ("another operation's filter", "location"),
        ("an unanchored prefix", "vcpu-info"),
        ("no name", ""),
    ];

    for (case, name) in cases {
        let (status, body) = describe(&router, "us-east-1", &filters(&[(name, &["2"])])).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 for the filter {name:?}, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValue",
            "case {case:?}: expected InvalidParameterValue, got {body}"
        );
        let message = unescaped(&element(&body, "Message").unwrap_or_default());
        assert!(
            message.contains(name) || name.is_empty(),
            "case {case:?}: the message must name the filter {name:?}, got {message:?}"
        );
    }
}

/// A filter naming a member this emulator reports ABSENT selects nothing, rather than
/// everything — and a filter naming one it REPORTS selects exactly the types carrying the
/// value asked for.
///
/// The accelerator inventories are per-type hardware data neither the model nor the name
/// yields, so they are reported absent rather than invented, and EC2's rule for a filter on an
/// attribute a resource does not carry is that it matches no value: each such filter is
/// accepted (it is a documented name) and answers an empty set. The members whose value the
/// catalog or the type's name DOES yield — the bandwidth weightings a type publishes, its
/// network cards and their performance, its processor's clock speed, its Nitro features and
/// the EBS attachments its pool leaves it — are reported, so the second half of the table
/// asserts the same filters select the type that reports the value and only it.
#[tokio::test]
async fn a_filter_on_a_member_reported_absent_selects_nothing() {
    let (router, _engine) = super::ec2_query::emulator();
    let absent: [(&str, &str, &str); 1] = [(
        "an EBS-optimized figure no type carries",
        "ebs-info.ebs-optimized-info.baseline-iops",
        "7",
    )];

    for (case, name, value) in absent {
        let (status, body) = describe(&router, "us-east-1", &filters(&[(name, &[value])])).await;
        assert_eq!(
            status, 200,
            "case {case:?}: {name} is a documented filter, got {status} {body}"
        );
        let items = type_items(&body);
        assert!(
            items.is_empty(),
            "case {case:?}: {name} compares against no value, so it selects nothing, \
             got {} types",
            items.len()
        );
    }

    // The rows differ along the member filtered on and the kind of value it carries: a
    // Gigabit string, a card count, a clock speed, a Nitro feature's enum member, an
    // attachment count and a published list of bandwidth weightings.
    let reported: [(&str, &str, &str, &str); 6] = [
        (
            "the bandwidth weightings",
            "network-info.bandwidth-weightings",
            "default",
            "c9gd.2xlarge",
        ),
        (
            "the network performance",
            "network-info.network-performance",
            "Up to 10 Gigabit",
            "m6gd.4xlarge",
        ),
        (
            "the network card count",
            "network-info.maximum-network-cards",
            "4",
            "p4d.24xlarge",
        ),
        (
            "the clock speed",
            "processor-info.sustained-clock-speed-in-ghz",
            "2.6",
            "m7g.xlarge",
        ),
        (
            "reboot migration",
            "reboot-migration-support",
            "supported",
            "m5.large",
        ),
        (
            "the EBS attachments",
            "ebs-info.maximum-ebs-attachments",
            "27",
            "m5.large",
        ),
    ];

    for (case, name, value, instance_type) in reported {
        let mut params = filters(&[(name, &[value])]);
        params.extend(named(instance_type));
        let (status, body) = describe(&router, "us-east-1", &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: {name} is a documented filter, got {status} {body}"
        );
        assert_eq!(
            type_items(&body).len(),
            1,
            "case {case:?}: {instance_type} reports {name} {value:?}, so the filter selects \
             it, got {body}"
        );

        // The same filter with a value the type does not carry selects nothing, so the
        // match is on the value rather than on the member being present.
        let mut params = filters(&[(name, &["an-impossible-value"])]);
        params.extend(named(instance_type));
        let (status, body) = describe(&router, "us-east-1", &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: got {status} {body}"
        );
        assert!(
            type_items(&body).is_empty(),
            "case {case:?}: {instance_type} does not carry that value, got {body}"
        );
    }
}

/// `EncryptionInTransitSupported` is reported for every type and is the SAME fact
/// `GetVpcResourcesBlockingEncryptionEnforcement` reports a VPC's instances against, and
/// the filter on it selects exactly the types that carry it.
///
/// The rows differ along what the rule generalises over: a type of a generation from which
/// every Nitro type encrypts, two of the earlier families AWS names one by one, a Nitro
/// type of an earlier generation, a type that runs on the Xen hypervisor, and a family
/// whose digits are a memory size rather than a generation — the high-memory `U-*` family,
/// which that same list names ("Encryption in transit", Amazon EC2 User Guide).
#[tokio::test]
async fn in_transit_encryption_is_reported_and_filterable() {
    let (router, _engine) = super::ec2_query::emulator();
    let cases: [(&str, &str, bool); 8] = [
        ("a 6th-generation type", "m6i.large", true),
        ("a 7th-generation type", "c7a.2xlarge", true),
        ("a named earlier family", "c5n.large", true),
        ("a named earlier AMD family", "c5a.large", true),
        ("a named earlier dense-storage family", "d3.xlarge", true),
        ("a Nitro type of the 5th generation", "m5.large", false),
        ("a Xen type", "t2.micro", false),
        ("a high-memory family carrying no generation", "u-6tb1.metal", true),
    ];

    for (case, instance_type, encrypts) in cases {
        let item = describe_one(&router, case, instance_type).await;
        assert_eq!(
            member(case, &item, "encryptionInTransitSupported"),
            encrypts.to_string(),
            "case {case:?}: {instance_type} encrypts in transit: {encrypts}, got {item:?}"
        );

        // The filter reads the same member, so a type is selected by exactly the value it
        // reports and by no other.
        for value in [true, false] {
            let mut params = filters(&[(
                "network-info.encryption-in-transit-supported",
                &[value.to_string().as_str()],
            )]);
            params.extend(named(instance_type));
            let (status, body) = describe(&router, "us-east-1", &params).await;
            assert_eq!(status, 200, "case {case:?}: got {status} {body}");
            assert_eq!(
                type_items(&body).len(),
                usize::from(value == encrypts),
                "case {case:?}: filtering {instance_type} on {value} selects it only when it \
                 reports {value}, got {body}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 5: MaxResults / NextToken / the modelled enum
// ---------------------------------------------------------------------------

/// A `NextToken` walk reports every matching type exactly once, in pages of at most
/// `MaxResults`, and the last page carries no token.
///
/// The rows are page sizes spanning the modelled `DITMaxResults` range 5..100
/// (`smithy_query(query="shape",shape="DITMaxResults")`): its lower bound, a size that
/// does not divide the result count, and its upper bound. The expectation is the un-paged
/// answer to the same request, so paging is asserted to change only the packaging.
#[tokio::test]
async fn a_next_token_walk_reports_every_type_once() {
    let (router, _engine) = super::ec2_query::emulator();
    let narrowing = filters(&[("instance-type", &["m5*"])]);

    let (unpaged, pages) = walk(&router, "unpaged", &narrowing, None).await;
    assert_eq!(
        pages, 1,
        "case unpaged: a request naming no MaxResults is answered in one page, got {pages}"
    );
    let expected: BTreeSet<String> = unpaged.iter().cloned().collect();
    assert!(
        expected.len() > 10,
        "case unpaged: the m5 families should carry many sizes, got {}",
        expected.len()
    );
    assert_eq!(
        expected.len(),
        unpaged.len(),
        "case unpaged: a type is reported once, got {unpaged:?}"
    );

    for page_size in [5usize, 7, 100] {
        let case = format!("MaxResults={page_size}");
        let (seen, pages) = walk(&router, &case, &narrowing, Some(page_size)).await;
        assert_eq!(
            seen.iter().cloned().collect::<BTreeSet<String>>(),
            expected,
            "case {case:?}: the walk must report the same types as the un-paged call"
        );
        assert_eq!(
            seen.len(),
            expected.len(),
            "case {case:?}: a type is reported exactly once over the walk, got {}",
            seen.len()
        );
        let expected_pages = expected.len().div_ceil(page_size);
        assert_eq!(
            pages, expected_pages,
            "case {case:?}: expected {expected_pages} pages for {} types, got {pages}",
            expected.len()
        );
    }
}

/// `MaxResults` outside the modelled 5..100 range is refused, each published bound is
/// accepted, and the value one past each bound is not.
#[tokio::test]
async fn max_results_is_held_to_its_modelled_range() {
    let (router, _engine) = super::ec2_query::emulator();
    // (case, MaxResults, is it inside the modelled range)
    let cases: [(&str, &str, bool); 6] = [
        ("one below the minimum", "4", false),
        ("the minimum", "5", true),
        ("inside", "50", true),
        ("the maximum", "100", true),
        ("one above the maximum", "101", false),
        ("zero", "0", false),
    ];

    for (case, value, inside) in cases {
        let params = vec![("MaxResults".to_string(), value.to_string())];
        let (status, body) = describe(&router, "us-east-1", &params).await;
        if inside {
            assert_eq!(
                status, 200,
                "case {case:?}: MaxResults={value} is inside 5..100, got {status} {body}"
            );
            let limit: usize = value.parse().expect("a numeric MaxResults");
            let page = type_items(&body);
            assert!(
                page.len() <= limit,
                "case {case:?}: a page holds at most {limit} types, got {}",
                page.len()
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: MaxResults={value} is outside 5..100, got {status} {body}"
            );
            assert!(
                element(&body, "instanceTypeSet").is_none(),
                "case {case:?}: a refused request reports no types, got {body}"
            );
        }
    }
}

/// A `NextToken` this service did not mint is refused with HTTP 400
/// `InvalidPaginationToken` rather than read as a cursor.
#[tokio::test]
async fn a_forged_next_token_is_refused() {
    let (router, _engine) = super::ec2_query::emulator();
    let cases = [
        ("an instance type", "m5.large"),
        ("not base64", "!!!!"),
        ("empty", ""),
        ("another service's token", "eyJvZmZzZXQiOiAxMH0="),
    ];

    for (case, token) in cases {
        let params = vec![("NextToken".to_string(), token.to_string())];
        let (status, body) = describe(&router, "us-east-1", &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 for NextToken {token:?}, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidPaginationToken",
            "case {case:?}: expected InvalidPaginationToken, got {body}"
        );
    }
}

/// An `InstanceTypes` value outside the modelled `InstanceType` enum is refused with HTTP
/// 400 rather than answered with an empty list.
///
/// The rows differ in how the name is wrong: an unknown size of a known family, an
/// unknown family, a family with no size, and the empty string.
#[tokio::test]
async fn an_instance_type_outside_the_catalog_is_refused() {
    let (router, _engine) = super::ec2_query::emulator();
    let cases = [
        ("an unknown size", "m5.enormous"),
        ("an unknown family", "zz9.large"),
        ("no size", "m5"),
        ("empty", ""),
    ];

    for (case, instance_type) in cases {
        let (status, body) = describe(&router, "us-east-1", &named(instance_type)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 for {instance_type:?}, got {status} {body}"
        );
        assert!(
            element(&body, "instanceTypeSet").is_none(),
            "case {case:?}: a refused request reports no types, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 6: DryRun
// ---------------------------------------------------------------------------

/// A `DryRun` request performs nothing and is answered 412 `DryRunOperation`, while the
/// same call without it describes the types.
#[tokio::test]
async fn a_dry_run_is_refused_with_dry_run_operation() {
    let (router, _engine) = super::ec2_query::emulator();
    for case in ["DryRun alone", "DryRun beside a named type"] {
        let mut params = vec![("DryRun".to_string(), "true".to_string())];
        if case.contains("named") {
            params.extend(named("m5.large"));
        }
        let (status, body) = describe(&router, "us-east-1", &params).await;
        assert_eq!(
            status, 412,
            "case {case:?}: expected 412, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "DryRunOperation",
            "case {case:?}: expected DryRunOperation, got {body}"
        );
        assert!(
            element(&body, "instanceTypeSet").is_none(),
            "case {case:?}: a dry run reports no types, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 7: the describe is a read
// ---------------------------------------------------------------------------

/// Describing an instance type changes nothing a later caller sees: the same request
/// answers the same body, in the same Region and in another one, and
/// `IncludeUnsupportedInRegion` adds nothing, every catalogued type being supported in
/// every Region this emulator serves.
#[tokio::test]
async fn the_describe_is_a_read() {
    let (router, _engine) = super::ec2_query::emulator();
    let cases: [(&str, Vec<(String, String)>); 3] = [
        ("one named type", named("m5.large")),
        ("a filtered set", filters(&[("instance-type", &["c6g.*"])])),
        ("one page", {
            let mut params = vec![("MaxResults".to_string(), "5".to_string())];
            params.extend(filters(&[("instance-type", &["m5*"])]));
            params
        }),
    ];

    for (case, params) in cases {
        let (first_status, first_body) = describe(&router, "us-east-1", &params).await;
        // The response envelope carries a per-call `RequestId`, so the comparison is of
        // the TYPES reported, which is what a later caller sees.
        let first = element(&first_body, "instanceTypeSet");
        assert_eq!(
            first_status, 200,
            "case {case:?}: expected 200, got {first_status} {first_body}"
        );
        let (second_status, second_body) = describe(&router, "us-east-1", &params).await;
        assert_eq!(
            (second_status, element(&second_body, "instanceTypeSet")),
            (first_status, first.clone()),
            "case {case:?}: repeating the describe must answer the same types"
        );
        let (other_status, other_body) = describe(&router, "ap-northeast-2", &params).await;
        assert_eq!(
            (other_status, element(&other_body, "instanceTypeSet")),
            (first_status, first.clone()),
            "case {case:?}: an instance type is a fact of the catalog, not of a Region"
        );

        let mut including = params.clone();
        including.push(("IncludeUnsupportedInRegion".to_string(), "true".to_string()));
        let (included_status, included_body) = describe(&router, "us-east-1", &including).await;
        assert_eq!(
            (included_status, element(&included_body, "instanceTypeSet")),
            (first_status, first),
            "case {case:?}: every catalogued type is supported in the Region, so \
             IncludeUnsupportedInRegion adds none"
        );
        for item in type_items(&first_body) {
            let instance_type = member(case, &item, "instanceType");
            assert_eq!(
                member(case, &item, "supportedInRegion"),
                "true",
                "case {case:?}: {instance_type} is supported in the current Region"
            );
        }
    }
}

/// The size ladder reader answers a size in sixteenths of a `large` for every size AWS's
/// naming conventions define, and NOTHING for a size component that is not one of them —
/// including a multiple so large that no size could carry it, which is a name a request
/// can supply and so must be refused rather than wrapped or panicked on.
///
/// The ladder itself: `large` is the unit, it doubles at each `Nxlarge` step, and the
/// sizes below it are fractions of it (`medium` a half, `small` a quarter, `micro` an
/// eighth, `nano` a sixteenth) — "Amazon EC2 instance type naming conventions", the source
/// the catalog's own per-family ratios are read against.
#[test]
fn the_size_ladder_reader_refuses_a_size_no_name_could_carry() {
    /// One row: the size component, and the sixteenths it names, if any.
    const CASES: [(&str, Option<u32>); 15] = [
        // The fractions of a `large`, each half of the one above it.
        ("nano", Some(1)),
        ("micro", Some(2)),
        ("small", Some(4)),
        ("medium", Some(8)),
        ("large", Some(16)),
        // The doubling ladder, at its first step, a middle step and the largest step the
        // published naming conventions use.
        ("xlarge", Some(32)),
        ("2xlarge", Some(64)),
        ("48xlarge", Some(1_536)),
        // A bare metal size that names its virtualized equivalent is on the ladder; a
        // plain `metal` names the whole host, whose size the NAME does not give.
        ("metal-48xl", Some(1_536)),
        ("metal", None),
        // Not sizes: a word off the ladder, an empty component, and a multiple with no
        // digits.
        ("enormous", None),
        ("", None),
        ("xxlarge", None),
        // A multiple no size carries: one past what a u32 product of the ladder admits,
        // and one past u32 altogether. Both are "not a size", the same answer any other
        // unrecognised component gets.
        ("134217728xlarge", None),
        ("99999999999999999999xlarge", None),
    ];

    for (size, expected) in CASES {
        let actual = ec2::state::instance_size_sixteenths(size);
        assert_eq!(
            actual, expected,
            "case {size:?}: expected {expected:?} sixteenths of a large, got {actual:?}"
        );
    }
}
