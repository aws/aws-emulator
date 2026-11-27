// SPDX-License-Identifier: CC0-1.0

//! Which instance type the Amazon Web Services Free Tier covers, driven over `ec2Query`
//! (README.md / OPERATIONS.md "Wire protocol").
//!
//! The rule under test: `InstanceTypeInfo$FreeTierEligible` — "indicates whether the instance
//! type is eligible for the free tier"
//! (`smithy_query(query="doc",shape="InstanceTypeInfo",member="FreeTierEligible")`) — is true
//! for the ONE type the Free Tier covers in the Region and false for every other type of the
//! catalog. The Free Tier covers 750 hours a month of `t2.micro` usage, and `t3.micro` only in
//! the Regions where `t2.micro` is unavailable; every Region this emulator serves offers the
//! whole burstable Xen ladder, so `t2.micro` is the covered type in each of them and no other
//! size, family or generation is covered — a type one step up the same ladder is not, and
//! neither is the Nitro burstable `micro` of a later generation.
//!
//! The rows span what the rule generalises over: the covered type, the next size of its own
//! ladder, the `micro` size of three other burstable families (two Nitro, one of them
//! Graviton) and a non-burstable type — asked about in two Regions. The last test holds the
//! rule over the WHOLE catalog, and through the filter that selects by it, so nothing but that
//! one type can answer true.

use super::ec2_query::{call, elements, filters, indexed};

/// One row: an instance type, and whether the Free Tier covers it.
const ROWS: [(&str, bool); 7] = [
    ("t2.micro", true),
    ("t2.nano", false),
    ("t2.small", false),
    ("t3.micro", false),
    ("t3a.micro", false),
    ("t4g.micro", false),
    ("m5.large", false),
];

/// The Regions the table is driven in: the Free Tier type is the same in each.
const REGIONS: [&str; 2] = ["us-west-2", "eu-central-1"];

/// Rule: exactly the covered type reports `freeTierEligible` true.
#[tokio::test]
async fn the_free_tier_covers_one_instance_type() {
    let (router, _engine) = super::ec2_query::emulator();

    for region in REGIONS {
        for (instance_type, covered) in ROWS {
            let case = format!("{instance_type} in {region}");
            let (status, body) = call(
                &router,
                region,
                "DescribeInstanceTypes",
                &indexed("InstanceType", &[instance_type]),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with body {body}"
            );
            let reported = elements(&body, "freeTierEligible");
            assert_eq!(
                reported,
                vec![covered.to_string()],
                "case {case:?}: expected freeTierEligible {covered}, got {reported:?}"
            );
        }
    }
}

/// Rule, over the whole catalog: one type is covered, and the `free-tier-eligible` filter
/// selects exactly that one.
#[tokio::test]
async fn the_catalog_and_the_filter_agree_on_the_covered_type() {
    let (router, _engine) = super::ec2_query::emulator();
    let region = REGIONS[0];

    let case = "the whole catalog";
    let (status, body) = call(&router, region, "DescribeInstanceTypes", &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}"
    );
    let covered = elements(&body, "freeTierEligible")
        .into_iter()
        .filter(|flag| flag == "true")
        .count();
    assert_eq!(
        covered, 1,
        "case {case:?}: expected exactly one free-tier eligible instance type, got {covered}"
    );

    for (case, value, expected) in [
        ("selected as eligible", "true", vec!["t2.micro".to_string()]),
        ("selected as not eligible", "false", Vec::new()),
    ] {
        let (status, body) = call(
            &router,
            region,
            "DescribeInstanceTypes",
            &filters(&[("free-tier-eligible", &[value])]),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        let selected = elements(&body, "instanceType");
        if expected.is_empty() {
            assert!(
                selected.len() > 1,
                "case {case:?}: every other type of the catalog is not eligible, got \
                 {selected:?}"
            );
            assert!(
                !selected.contains(&"t2.micro".to_string()),
                "case {case:?}: the covered type must not be selected, got {selected:?}"
            );
        } else {
            assert_eq!(
                selected, expected,
                "case {case:?}: expected the covered type alone, got {selected:?}"
            );
        }
    }
}
