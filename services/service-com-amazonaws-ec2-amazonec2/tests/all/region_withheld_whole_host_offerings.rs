// SPDX-License-Identifier: CC0-1.0

//! Which WHOLE-HOST instance types a Region is offered, driven over the wire this service
//! speaks (`ec2Query`, an `application/x-www-form-urlencoded` POST answered with XML;
//! README.md / OPERATIONS.md "Wire protocol").
//!
//! The rule under test: `DescribeInstanceTypeOfferings` reports an offering only for a type the
//! requested location actually has, so a type no Region's published offering set names is
//! absent from the set — in every kind of location — while the types the published set does
//! name are reported wherever their locations are. A client calls this operation to decide
//! whether a type is launchable before it launches it, so a type reported as offered that the
//! Region does not have is a launch that fails after the choice was made.
//!
//! The rows span what the rule generalises over: the whole-host sizes of the newest network
//! optimized, bandwidth optimized and block storage optimized families, in all three forms the
//! name takes (`metal-24xl`, `metal-48xl` and `metal-96xl`), the bare metal size of a previous
//! generation storage optimized family — a withheld type whose family is not new at all — and,
//! against each, a type of the same family that IS offered: a VIRTUALIZED size, and the wider
//! whole-host size of a family whose narrower one is withheld, so the withholding follows the
//! type rather than the family or the form of the name. Each row is asked of two Regions and of
//! two kinds of location, the restriction being a fact of the Region rather than of how a
//! request names its locations.

use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, filters};
use super::wire;

/// One form field.
fn param(name: &str, value: &str) -> (String, String) {
    (name.to_string(), value.to_string())
}

/// The instance type of every offering a describe filtered to `instance_type` reports.
async fn offered_types(
    case: &str,
    router: &MultiServiceRouter,
    region: &str,
    location_type: &str,
    instance_type: &str,
) -> Vec<String> {
    let mut params = filters(&[("instance-type", &[instance_type])]);
    params.push(param("LocationType", location_type));
    let (status, body) = call(router, region, "DescribeInstanceTypeOfferings", &params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}"
    );
    let document = wire::document(case, &body);
    wire::set_items(document.root_element(), "instanceTypeOfferingSet")
        .into_iter()
        .map(|item| wire::child_text(item, "instanceType").unwrap_or_default())
        .collect()
}

/// The rule, over the withheld whole-host sizes and the virtualized siblings beside them.
#[tokio::test]
async fn a_whole_host_size_no_region_is_published_with_is_offered_nowhere() {
    let (router, _engine) = super::ec2_query::emulator();

    // (the type, whether any location of a Region offers it)
    let rows: [(&str, bool); 15] = [
        // The newest network optimized Graviton compute family: its whole-host sizes are
        // named by no Region, its virtualized ones by every Region.
        ("c8gn.metal-24xl", false),
        ("c8gn.metal-48xl", false),
        ("c8gn.24xlarge", true),
        // The bandwidth optimized Intel families, whose whole-host sizes are the `metal-48xl`
        // and the wider `metal-96xl`.
        ("c8ib.metal-48xl", false),
        ("c8ib.metal-96xl", false),
        ("c8ib.96xlarge", true),
        ("r8in.metal-96xl", false),
        ("r8in.48xlarge", true),
        // The newest general purpose Graviton families: the narrower of the two whole-host
        // sizes is named by no Region while the WIDER one is published, so the withholding
        // follows the type and not the `metal-NNxl` form of its name.
        ("m9g.metal-24xl", false),
        ("m9gd.metal-24xl", false),
        ("m9g.metal-48xl", true),
        ("m9g.48xlarge", true),
        // A previous generation storage optimized family: its bare metal size is named by no
        // Region either, while the virtualized sizes beside it are offered.
        ("i3.metal", false),
        ("i3.16xlarge", true),
        ("i3.large", true),
    ];

    for region in ["us-west-2", "eu-central-1"] {
        for location_type in ["region", "availability-zone"] {
            for (instance_type, offered) in rows {
                let case = format!("{region}/{location_type}/{instance_type}");
                let reported =
                    offered_types(&case, &router, region, location_type, instance_type).await;
                assert_eq!(
                    reported.is_empty(),
                    !offered,
                    "case {case:?}: expected the type to be offered: {offered}, got \
                     {reported:?}"
                );
                for reported_type in &reported {
                    assert_eq!(
                        reported_type, instance_type,
                        "case {case:?}: the filtered report carries only the type asked for, \
                         got {reported_type:?}"
                    );
                }
            }
        }
    }
}
