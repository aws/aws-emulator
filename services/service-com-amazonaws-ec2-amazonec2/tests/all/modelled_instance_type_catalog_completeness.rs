// SPDX-License-Identifier: CC0-1.0

//! Every instance type the model declares is DESCRIBED by `DescribeInstanceTypes`, driven over
//! `ec2Query` (README.md / OPERATIONS.md "Wire protocol").
//!
//! The rule under test: the catalog this operation reports is the modelled `InstanceType`
//! enum's own value list (`smithy_query(query="shape",shape="InstanceType")`), and
//! "by default, all instance types for the current Region are described"
//! (`smithy_query(query="doc",shape="DescribeInstanceTypes")`) — so a request naming none
//! reports EVERY type the model declares, and a request naming a family's types reports that
//! family's whole ladder. A type the catalog holds no published hardware row for is still
//! described, by the derivation over its name, rather than silently left out of the page: a
//! client enumerating the catalog can see every type the model offers.
//!
//! The rows span what the rule generalises over: the catalog as a whole, and three families
//! named one by one — an accelerated family whose sizes carry local devices, a fractional-GPU
//! one whose ladder is four sizes long, and the newest graphics one — each asked for as a
//! named list and as a wildcard filter, which must report the same ladder.

use super::ec2_query::{call, elements, filters, indexed};
use service_com_amazonaws_ec2_amazonec2 as ec2;

/// The Region every call here is signed for; the catalog is the same in each.
const REGION: &str = "us-west-2";

/// The instance types a reported body names, in the order it reports them.
fn reported_types(body: &str) -> Vec<String> {
    elements(body, "instanceType")
}

/// Rule: a request naming no instance type describes every type the model declares.
#[tokio::test]
async fn the_whole_modelled_catalog_is_described() {
    let (router, _engine) = super::ec2_query::emulator();
    let case = "the whole catalog";
    let (status, body) = call(&router, REGION, "DescribeInstanceTypes", &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}"
    );
    let reported = reported_types(&body);
    let missing: Vec<&str> = ec2::ops::types::INSTANCE_TYPE_VALUES
        .iter()
        .copied()
        .filter(|value| !reported.iter().any(|name| name == value))
        .collect();
    assert!(
        missing.is_empty(),
        "case {case:?}: expected every modelled instance type to be described, {} of {} are \
         missing: {missing:?}",
        missing.len(),
        ec2::ops::types::INSTANCE_TYPE_VALUES.len()
    );
}

/// Rule: a family's whole ladder is reported, whether the request names its types or selects
/// them with a wildcard — and the two agree.
#[tokio::test]
async fn a_familys_whole_ladder_is_described_however_it_is_selected() {
    let (router, _engine) = super::ec2_query::emulator();

    // (the family, its sizes as the model declares them)
    let rows: [(&str, &[&str]); 3] = [
        (
            "g4dn",
            &[
                "g4dn.xlarge",
                "g4dn.2xlarge",
                "g4dn.4xlarge",
                "g4dn.8xlarge",
                "g4dn.12xlarge",
                "g4dn.16xlarge",
                "g4dn.metal",
            ],
        ),
        ("g6f", &["g6f.large", "g6f.xlarge", "g6f.2xlarge", "g6f.4xlarge"]),
        (
            "g7",
            &[
                "g7.2xlarge",
                "g7.4xlarge",
                "g7.8xlarge",
                "g7.12xlarge",
                "g7.24xlarge",
                "g7.48xlarge",
            ],
        ),
    ];

    for (family, sizes) in rows {
        let case = format!("the {family} ladder named one by one");
        let (status, body) = call(
            &router,
            REGION,
            "DescribeInstanceTypes",
            &indexed("InstanceType", sizes),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        let mut named = reported_types(&body);
        named.sort();
        let mut expected: Vec<String> = sizes.iter().map(|size| (*size).to_string()).collect();
        expected.sort();
        assert_eq!(
            named, expected,
            "case {case:?}: expected the whole ladder {expected:?}, got {named:?}"
        );

        let case = format!("the {family} ladder selected by wildcard");
        let pattern = format!("{family}.*");
        let (status, body) = call(
            &router,
            REGION,
            "DescribeInstanceTypes",
            &filters(&[("instance-type", &[&pattern])]),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        let mut selected = reported_types(&body);
        selected.sort();
        assert_eq!(
            selected, expected,
            "case {case:?}: a wildcard over the family must report the same ladder as naming \
             it, expected {expected:?}, got {selected:?}"
        );
    }
}
