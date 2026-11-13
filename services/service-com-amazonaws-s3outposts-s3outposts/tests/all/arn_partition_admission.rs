// SPDX-License-Identifier: CC0-1.0

//! An ARN member carries a value its own pattern admits in EVERY segment that pattern
//! closes, or carries nothing at all.
//!
//! The rule under test: `Outpost$OutpostArn`, `Outpost$S3OutpostArn`,
//! `Endpoint$EndpointArn` and `CreateEndpointResult$EndpointArn` close TWO of their
//! segments as fixed alternations — the partition
//! (`^arn:(aws|aws-cn|aws-us-gov|aws-iso|aws-iso-b):…`, identical in all four) and the
//! Outpost segment (`(op-[a-f0-9]{17}|ec2)`, `(op-[a-f0-9]{17}|\d{12})`) — so a member has
//! a legal value only when the resource's Region lies in a partition its alternation lists
//! AND the Outpost's id lies in a branch it lists. Each member is optional, so where either
//! half says no the member is absent and everything else about the resource is reported as
//! usual.
//!
//! The cases are the CROSS PRODUCT of those two halves, so neither gate can be satisfied by
//! the other: a Region outside the alternation hides the member for an Outpost whose id
//! every member admits, and an Outpost id outside a member's branch list hides it in a
//! Region the alternation does list.
//!
//! Every case goes through the router, so each exercises routing, deserialization, boundary
//! validation, the handler, the persisted state and serialization.

use crate::arn_patterns::{optional_member, outpost_arn_members, the_outpost};
use crate::endpoint_lifecycle::{
    create_in, emulator_holding, list_endpoints_in, minimal_create, string_at, WHOLE_PAGE,
};

/// A Region, and the partition its ARNs name when the members' own alternation lists it.
///
/// The partition names are the five the alternation publishes
/// (`smithy_query(query="shape", shape="OutpostArn,S3OutpostArn,EndpointArn")`); `None`
/// marks a Region whose partition that alternation does not list, for which no member of
/// this service has a legal value. The Region codes come from the emulator's own catalog
/// (`emulator_telemetry::policy::KNOWN_REGIONS`), plus `eusc-de-east-1`, which the models'
/// own Region value sets publish.
struct RegionRow {
    case: &'static str,
    region: &'static str,
    partition: Option<&'static str>,
}

/// Regions spanning every published partition, and the ones outside the published list.
const REGIONS: &[RegionRow] = &[
    RegionRow {
        case: "the commercial partition",
        region: "us-east-1",
        partition: Some("aws"),
    },
    RegionRow {
        case: "the China partition",
        region: "cn-northwest-1",
        partition: Some("aws-cn"),
    },
    RegionRow {
        case: "GovCloud",
        region: "us-gov-east-1",
        partition: Some("aws-us-gov"),
    },
    RegionRow {
        case: "the isolated partition",
        region: "us-iso-west-1",
        partition: Some("aws-iso"),
    },
    RegionRow {
        case: "the second isolated partition, whose prefix extends the first",
        region: "us-isob-east-1",
        partition: Some("aws-iso-b"),
    },
    RegionRow {
        case: "a partition outside the published alternation",
        region: "us-isof-east-1",
        partition: None,
    },
    RegionRow {
        case: "the European Sovereign Cloud, also outside it",
        region: "eusc-de-east-1",
        partition: None,
    },
    RegionRow {
        case: "the European isolated partition, also outside it",
        region: "eu-isoe-west-1",
        partition: None,
    },
];

/// An Outpost id per branch of `OutpostId`'s `^(op-[a-f0-9]{17}|\d{12}|ec2)$`
/// (`smithy_query(query="shape", shape="OutpostId")`), so the partition half of the rule is
/// exercised beside each branch half.
const OUTPOST_IDS: &[(&str, &str)] = &[
    ("the op- branch", "op-1b2c3d4e5f6071829"),
    ("the 12-digit branch", "123456789012"),
    ("the ec2 branch", "ec2"),
];

/// Whether `Endpoint$EndpointArn`'s own `…:outpost/(op-[a-f0-9]{17}|ec2)/endpoint/…` admits
/// an Outpost identified by `outpost_id`: the 12-digit form is not one of its branches.
/// Read off the member's pattern here rather than from the service, so a wrong constant in
/// the service cannot make a case pass.
fn endpoint_arn_admits(outpost_id: &str) -> bool {
    outpost_id == "ec2" || outpost_id.starts_with("op-")
}

/// The partition an ARN member can name a resource in, with BOTH halves of the rule
/// applied: the Region's partition when the member's alternation lists it and its Outpost
/// segment lists this id's branch, and `None` — the member carries nothing — when either
/// half rejects it. No candidate string is built for a case with no legal value.
fn admitted_in(partition: Option<&'static str>, branch_admitted: bool) -> Option<&'static str> {
    partition.filter(|_| branch_admitted)
}

/// For every (partition, Outpost id branch) pair, each ARN member carries exactly the ARN
/// its own pattern admits — and nothing when either the partition or the id branch is one
/// the pattern does not list, while the Outpost and the endpoint are still reported in full.
#[tokio::test]
async fn an_arn_member_is_carried_only_where_both_its_closed_segments_admit_the_resource() {
    let account = engine::emulator_account_id().to_string();
    let mut absent_for_partition = 0;
    let mut absent_for_branch = 0;
    let mut carried = 0;

    for region_row in REGIONS {
        let RegionRow {
            case: region_case,
            region,
            partition,
        } = region_row;

        for (id_case, outpost_id) in OUTPOST_IDS {
            let case = format!("{id_case} in {region_case} ({region})");
            // One emulator per case, holding exactly this Outpost in this Region: an
            // Outpost has a home Region, so each case needs its own fixture.
            let router = emulator_holding(&[((*outpost_id).to_string(), region)]);

            // Which members the two halves of the rule admit, composed: the partition half
            // applies to every member alike, the branch half differs per member.
            let (branch_outpost_arn, branch_s3_outpost_arn) = outpost_arn_members(outpost_id);
            let outpost_arn_in = admitted_in(*partition, branch_outpost_arn);
            let s3_outpost_arn_in = admitted_in(*partition, branch_s3_outpost_arn);
            let endpoint_arn_in = admitted_in(*partition, endpoint_arn_admits(outpost_id));

            // The Outpost itself is reported whatever its ARNs: a member a pattern cannot
            // express is left out, it does not hide the resource.
            let outpost = the_outpost(&router, region).await;
            assert_eq!(
                string_at(&outpost, "OutpostId"),
                *outpost_id,
                "case {case:?}: the inventory must report the provisioned Outpost, got \
                 {outpost:?}"
            );
            assert_eq!(
                string_at(&outpost, "OwnerId"),
                account,
                "case {case:?}: the Outpost must report its owning account, got {outpost:?}"
            );

            let expected_outpost_arn = outpost_arn_in
                .map(|p| format!("arn:{p}:outposts:{region}:{account}:outpost/{outpost_id}"));
            assert_eq!(
                optional_member(&outpost, "OutpostArn"),
                expected_outpost_arn,
                "case {case:?}: expected OutpostArn {expected_outpost_arn:?} — the ARN where \
                 both the partition and the id branch are ones the member's pattern lists, \
                 nothing otherwise — got {outpost:?}"
            );

            let expected_s3_arn = s3_outpost_arn_in.map(|p| {
                format!("arn:{p}:s3-outposts:{region}:{account}:outpost/{outpost_id}/s3")
            });
            assert_eq!(
                optional_member(&outpost, "S3OutpostArn"),
                expected_s3_arn,
                "case {case:?}: expected S3OutpostArn {expected_s3_arn:?}, got {outpost:?}"
            );

            // The endpoint is created, and so stored, whatever its ARN member can express.
            let created = create_in(&router, region, minimal_create(outpost_id)).await;
            assert_eq!(
                created.status, 200,
                "case {case:?}: CreateEndpoint must answer 200, got {} with {:?}",
                created.status, created.body
            );
            let reported = optional_member(&created.body, "EndpointArn");
            assert_eq!(
                reported.is_some(),
                endpoint_arn_in.is_some(),
                "case {case:?}: the EndpointArn member must be carried exactly where its \
                 pattern lists both this partition and this id branch (admitted: {}), got {:?}",
                endpoint_arn_in.is_some(),
                created.body
            );
            match (endpoint_arn_in, reported.as_deref()) {
                (Some(partition), Some(arn)) => {
                    let (prefix, endpoint_id) = arn
                        .rsplit_once("/endpoint/")
                        .unwrap_or_else(|| panic!("case {case:?}: {arn:?} is no EndpointArn"));
                    let expected_prefix = format!(
                        "arn:{partition}:s3-outposts:{region}:{account}:outpost/{outpost_id}"
                    );
                    assert_eq!(
                        prefix, expected_prefix,
                        "case {case:?}: the EndpointArn must name this partition, Region, \
                         caller and Outpost ({expected_prefix}), got {arn:?}"
                    );
                    // `EndpointId`'s own `^[a-zA-Z0-9]{19}$`, so the whole ARN is one the
                    // member's pattern admits.
                    assert!(
                        endpoint_id.len() == 19
                            && endpoint_id.bytes().all(|b| b.is_ascii_alphanumeric()),
                        "case {case:?}: the EndpointArn must end in a 19-character \
                         alphanumeric endpoint id, got {arn:?}"
                    );
                    carried += 1;
                }
                _ if partition.is_some() => absent_for_branch += 1,
                _ => absent_for_partition += 1,
            }

            // Read back through the listing: the stored endpoint is reported with the very
            // same member the create answered with, and its other members regardless.
            let listed = list_endpoints_in(&router, region, WHOLE_PAGE).await;
            assert_eq!(
                listed.status, 200,
                "case {case:?}: ListEndpoints must answer 200, got {} with {:?}",
                listed.status, listed.body
            );
            let endpoints = listed.endpoints();
            assert_eq!(
                endpoints.len(),
                1,
                "case {case:?}: ListEndpoints must report the created endpoint, got {:?}",
                listed.body
            );
            assert_eq!(
                string_at(&endpoints[0], "OutpostsId"),
                *outpost_id,
                "case {case:?}: the listing must report the endpoint on its own Outpost, got \
                 {:?}",
                listed.body
            );
            assert_eq!(
                optional_member(&endpoints[0], "EndpointArn"),
                reported,
                "case {case:?}: the listing must report the same EndpointArn member the create \
                 answered with ({reported:?}), got {:?}",
                listed.body
            );
        }
    }

    // All three outcomes of the composed rule are genuinely exercised: a member carried, a
    // member hidden by the id branch alone, and a member hidden by the partition alone.
    assert!(
        carried > 0 && absent_for_branch > 0 && absent_for_partition > 0,
        "the cross product must cover a carried EndpointArn and one hidden by each half of \
         the rule, got {carried} carried, {absent_for_branch} hidden by the id branch and \
         {absent_for_partition} hidden by the partition"
    );
}
