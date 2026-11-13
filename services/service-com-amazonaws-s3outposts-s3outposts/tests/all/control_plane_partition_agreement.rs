// SPDX-License-Identifier: CC0-1.0

//! The partition every ARN this service reports names, against the partition the AWS
//! Outposts control plane that OWNS the Outpost names for the very same resource — driven
//! end to end over HTTP across BOTH emulated services.
//!
//! The rule under test, stated once and then exercised over a SET of Regions: an ARN names
//! its resource in full, so its partition field is a function of the resource's Region
//! alone, read off AWS's published region-to-partition prefixes; and a member whose pattern
//! closes that field to a fixed alternation
//! (`^arn:(aws|aws-cn|aws-us-gov|aws-iso|aws-iso-b):…`, identical in `Outpost$OutpostArn`,
//! `Outpost$S3OutpostArn`, `Endpoint$EndpointArn` and `CreateEndpointResult$EndpointArn`)
//! has NO legal value for a Region in a partition that alternation does not list — the
//! members being optional, it then carries nothing while the resource itself is reported as
//! usual.
//!
//! One Outpost therefore never has two ARNs: for each Region, either this service reports
//! exactly the ARN its owning control plane reports for that Outpost, or it reports no ARN
//! at all. Which of the two is decided by the Region's partition and nothing else.

use serde_json::Value;

use crate::control_plane_inventory::{deliver_orders, string_at, Catalog, Emulator, Item};

/// One Region and the partition AWS places it in.
///
/// SOURCE of the partitions: AWS's published partitions and their Region prefixes — `aws`
/// for the commercial Regions, `aws-cn` for `cn-*`, `aws-us-gov` for `us-gov-*`, `aws-eusc`
/// for the European Sovereign Cloud (`eusc-de-*`), and the ISO partitions `aws-iso`
/// (`us-iso-*`), `aws-iso-b` (`us-isob-*`), `aws-iso-e` (`eu-isoe-*`) and `aws-iso-f`
/// (`us-isof-*`).
///
/// SOURCE of the Region codes: the emulator's own Region catalog
/// (`emulator_telemetry::policy::KNOWN_REGIONS`, "every AWS region the wire may carry"),
/// except `eusc-de-east-1`, which the models' own Region value sets publish (the
/// `VPCRegion` / `CloudWatchRegion` value sets of `route53`'s model list it).
struct Row {
    case: &'static str,
    region: &'static str,
    partition: &'static str,
}

/// Rows spanning every partition AWS publishes a Region prefix for, with a second Region in
/// the partitions that have one and the prefixes that share a leading substring
/// (`us-iso-east-1` beside `us-isob-east-1`), so no case can pass by matching one Region
/// code — and so the table covers partitions on both sides of the members' alternation.
const ROWS: &[Row] = &[
    Row {
        case: "the commercial partition",
        region: "us-east-1",
        partition: "aws",
    },
    Row {
        case: "another commercial Region",
        region: "ap-southeast-2",
        partition: "aws",
    },
    Row {
        case: "the China partition",
        region: "cn-north-1",
        partition: "aws-cn",
    },
    Row {
        case: "GovCloud",
        region: "us-gov-east-1",
        partition: "aws-us-gov",
    },
    Row {
        case: "the isolated partition",
        region: "us-iso-east-1",
        partition: "aws-iso",
    },
    Row {
        case: "the second isolated partition, whose prefix extends the first",
        region: "us-isob-east-1",
        partition: "aws-iso-b",
    },
    Row {
        case: "the European Sovereign Cloud",
        region: "eusc-de-east-1",
        partition: "aws-eusc",
    },
    Row {
        case: "the European isolated partition",
        region: "eu-isoe-west-1",
        partition: "aws-iso-e",
    },
    Row {
        case: "the newest isolated partition",
        region: "us-isof-east-1",
        partition: "aws-iso-f",
    },
];

/// The partitions an ARN member of THIS service can name a resource in, spelled out by the
/// alternation every one of their patterns opens with (`smithy_query(query="shape",
/// shape="OutpostArn,S3OutpostArn,EndpointArn")`).
///
/// Written down here, off the members' own patterns, rather than read from the service, so a
/// wrong constant in the service cannot make a case pass.
const MEMBER_PARTITIONS: &[&str] = &["aws", "aws-cn", "aws-us-gov", "aws-iso", "aws-iso-b"];

/// The string member `name` of `value`, or `None` where the answer carries no such member.
fn optional_member(value: &Value, name: &str) -> Option<String> {
    value[name].as_str().map(str::to_string)
}

/// The partition field of an ARN: its second colon-separated field.
fn partition_of(case: &str, arn: &str) -> String {
    let mut fields = arn.split(':');
    assert_eq!(
        fields.next(),
        Some("arn"),
        "case {case:?}: {arn:?} must begin with the literal arn"
    );
    fields
        .next()
        .unwrap_or_else(|| panic!("case {case:?}: {arn:?} carries no partition field"))
        .to_string()
}

/// The one Outpost `ListOutpostsWithS3` reports in `region`.
async fn the_outpost_in(emulator: &Emulator, case: &str, region: &str) -> Value {
    let reported = emulator.inventory(region).await;
    assert_eq!(
        reported.len(),
        1,
        "case {case:?}: the inventory must report exactly the Outpost provisioned in this \
         Region, got {reported:?}"
    );
    reported[0].clone()
}

/// The endpoints `ListEndpoints` reports in `region`.
async fn endpoints_in(emulator: &Emulator, case: &str, region: &str) -> Vec<Value> {
    let listed = emulator.endpoints_in(region).await;
    assert_eq!(
        listed.status, 200,
        "case {case:?}: ListEndpoints must answer 200, got {} with {}",
        listed.status, listed.body
    );
    listed.body["Endpoints"]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: ListEndpoints must carry Endpoints, got {}",
                listed.body
            )
        })
        .clone()
}

/// The id of the one endpoint the store holds on `outpost_id`.
///
/// `CreateEndpointResult` publishes the endpoint's ARN alone and `Endpoint` carries no id
/// member of its own (`smithy_query(query="shape", shape="Endpoint")`), so in a Region
/// whose partition no ARN member admits there is no response to read the id off at all —
/// which is why this case reads the store for it, exactly as `outpost_departure.rs` reads
/// what no response reports. Everything asserted about behaviour still goes over the wire.
fn stored_endpoint_id(emulator: &Emulator, case: &str, outpost_id: &str) -> String {
    let store = emulator.s3_store();
    let store = store.lock();
    let mut ids = store
        .endpoints
        .keys()
        .filter(|(on_outpost, _)| on_outpost == outpost_id)
        .map(|(_, endpoint_id)| endpoint_id.clone());
    let endpoint_id = ids
        .next()
        .unwrap_or_else(|| panic!("case {case:?}: the store must hold the created endpoint"));
    assert_eq!(
        ids.next(),
        None,
        "case {case:?}: the store must hold exactly one endpoint on {outpost_id}"
    );
    endpoint_id
}

/// An Outpost of every row's Region, each provisioned purely through `outposts` API calls
/// in ONE emulator, as the `(row, OutpostId)` pairs placed.
async fn an_outpost_per_region(
    emulator: &Emulator,
    catalog: &Catalog,
) -> Vec<(&'static Row, String)> {
    let mut placed = Vec::new();
    for row in ROWS {
        let outpost_id = emulator.an_outpost_in(row.region, "partition").await;
        emulator
            .an_order_in(
                row.region,
                &outpost_id,
                &catalog.line_items(&[(Item::S3(0), 1)]),
            )
            .await;
        placed.push((row, outpost_id));
    }
    deliver_orders(emulator);
    placed
}

/// The rule over the whole table: for one Outpost, this service reports either exactly the
/// ARN its owning control plane reports — same partition, same Region — or no ARN at all,
/// and which of the two is decided by whether the member's own alternation lists that
/// Region's partition.
#[tokio::test]
async fn every_reported_arn_agrees_with_the_control_plane_that_owns_the_outpost() {
    let catalog = Catalog::discover(&Emulator::new()).await;
    let emulator = Emulator::new();
    let account = engine::emulator_account_id().to_string();
    let placed = an_outpost_per_region(&emulator, &catalog).await;
    let (mut carried, mut absent) = (0, 0);

    for (row, outpost_id) in &placed {
        let Row {
            case,
            region,
            partition,
        } = row;
        let case = format!("{case} ({region})");

        // What the control plane that OWNS this Outpost reports for it.
        let owned = emulator
            .outposts_ok(region, "GET", &format!("/outposts/{outpost_id}"), None)
            .await;
        let control_plane_arn = string_at(&owned["Outpost"], "OutpostArn");
        assert_eq!(
            control_plane_arn,
            format!("arn:{partition}:outposts:{region}:{account}:outpost/{outpost_id}"),
            "case {case:?}: the owning control plane names this Region's own partition, got \
             {control_plane_arn:?}"
        );

        // The same Outpost through this service. It is reported whatever its ARNs: a member
        // whose pattern cannot express the resource is left out, it does not hide it.
        let admitted = MEMBER_PARTITIONS.contains(partition);
        let reported = the_outpost_in(&emulator, &case, region).await;
        assert_eq!(
            string_at(&reported, "OutpostId"),
            *outpost_id,
            "case {case:?}: the inventory must report the provisioned Outpost, got {reported}"
        );
        assert_eq!(
            string_at(&reported, "OwnerId"),
            account,
            "case {case:?}: the Outpost must report its owning account, got {reported}"
        );
        assert!(
            reported["CapacityInBytes"].as_i64().unwrap_or_default() > 0,
            "case {case:?}: the Outpost must report the S3 capacity its order bought, got \
             {reported}"
        );

        let expected_outpost_arn = admitted.then(|| control_plane_arn.clone());
        let reported_outpost_arn = optional_member(&reported, "OutpostArn");
        assert_eq!(
            reported_outpost_arn, expected_outpost_arn,
            "case {case:?}: expected OutpostArn {expected_outpost_arn:?} — the control \
             plane's own ARN where the member's alternation lists this partition, nothing \
             where it does not — got {reported_outpost_arn:?}"
        );
        let expected_s3_arn = admitted.then(|| {
            format!("arn:{partition}:s3-outposts:{region}:{account}:outpost/{outpost_id}/s3")
        });
        let reported_s3_arn = optional_member(&reported, "S3OutpostArn");
        assert_eq!(
            reported_s3_arn, expected_s3_arn,
            "case {case:?}: expected S3OutpostArn {expected_s3_arn:?}, got {reported_s3_arn:?}"
        );

        // The endpoint side of the same rule, through the mint and the read back.
        let created = emulator.create_endpoint_in(region, outpost_id, 1).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: CreateEndpoint must answer 200, got {} with {}",
            created.status, created.body
        );
        let minted = optional_member(&created.body, "EndpointArn");
        assert_eq!(
            minted.is_some(),
            admitted,
            "case {case:?}: the EndpointArn member must be carried exactly where the \
             member's own alternation lists this Region's partition (admitted: {admitted}), \
             got {}",
            created.body
        );
        if let Some(arn) = &minted {
            let expected_prefix = format!(
                "arn:{partition}:s3-outposts:{region}:{account}:outpost/{outpost_id}/endpoint/"
            );
            assert!(
                arn.starts_with(&expected_prefix),
                "case {case:?}: the EndpointArn must name this partition, Region, caller and \
                 Outpost ({expected_prefix}…), got {arn:?}"
            );
        }
        let listed = endpoints_in(&emulator, &case, region).await;
        assert_eq!(
            listed.len(),
            1,
            "case {case:?}: ListEndpoints must report exactly the endpoint created in this \
             Region, got {listed:?}"
        );
        let listed_arn = optional_member(&listed[0], "EndpointArn");
        assert_eq!(
            listed_arn, minted,
            "case {case:?}: ListEndpoints must report the same EndpointArn member the create \
             answered with ({minted:?}), got {listed_arn:?}"
        );

        // No answer of either service contradicts the other: every ARN reported for this
        // resource names ONE partition — the Region's own — and that Region.
        let control_plane_partition = partition_of(&case, &control_plane_arn);
        for arn in [reported_outpost_arn, reported_s3_arn, minted]
            .into_iter()
            .flatten()
        {
            assert_eq!(
                partition_of(&case, &arn),
                control_plane_partition,
                "case {case:?}: an ARN of this service must name the partition the control \
                 plane names for the same Outpost ({control_plane_partition}), got {arn:?}"
            );
            assert!(
                arn.contains(&format!(":{region}:")),
                "case {case:?}: an ARN naming partition {partition} must name the Region it \
                 belongs to, got {arn:?}"
            );
            carried += 1;
        }
        if !admitted {
            absent += 1;
        }
    }

    // Both halves of the rule are genuinely exercised: Regions whose partition the members
    // admit, and Regions whose partition they do not.
    assert!(
        carried > 0 && absent > 0,
        "the table must cover Regions on both sides of the members' alternation, got \
         {carried} carried ARNs and {absent} Regions with none"
    );
}

/// A Region whose partition no ARN member of this service can name is still served in
/// full: the Outpost resolves, the endpoint lifecycle runs on it and the resource is
/// reported throughout — only the ARN members, which have no legal value there, are absent.
#[tokio::test]
async fn a_region_outside_the_members_alternation_still_serves_the_whole_lifecycle() {
    let catalog = Catalog::discover(&Emulator::new()).await;
    let emulator = Emulator::new();
    let placed = an_outpost_per_region(&emulator, &catalog).await;

    for (row, outpost_id) in placed
        .iter()
        .filter(|(row, _)| !MEMBER_PARTITIONS.contains(&row.partition))
    {
        let case = format!("{} ({})", row.case, row.region);
        let region = row.region;

        let created = emulator.create_endpoint_in(region, outpost_id, 2).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: CreateEndpoint must answer 200, got {} with {}",
            created.status, created.body
        );
        assert_eq!(
            optional_member(&created.body, "EndpointArn"),
            None,
            "case {case:?}: no ARN member of this service can name a resource in \
             {partition}, so the mint carries none, got {}",
            created.body,
            partition = row.partition,
        );

        // `CreateEndpointResult` publishes the ARN alone and `Endpoint` carries no id
        // member of its own, so the id of an endpoint in a Region no ARN can name is read
        // through the store seam, exactly as `outpost_departure.rs` reads what no response
        // reports. Everything asserted about behaviour still goes over the wire.
        let listed = endpoints_in(&emulator, &case, region).await;
        assert_eq!(
            listed.len(),
            1,
            "case {case:?}: the created endpoint must read back through ListEndpoints, got \
             {listed:?}"
        );
        assert_eq!(
            string_at(&listed[0], "OutpostsId"),
            *outpost_id,
            "case {case:?}: the endpoint must be reported on its own Outpost, got {}",
            listed[0]
        );
        assert_eq!(
            optional_member(&listed[0], "EndpointArn"),
            None,
            "case {case:?}: the listing must carry no EndpointArn either, got {}",
            listed[0]
        );
        let endpoint_id = stored_endpoint_id(&emulator, &case, outpost_id);

        let deleted = emulator
            .delete_endpoint_in(region, outpost_id, &endpoint_id)
            .await;
        assert_eq!(
            deleted.status, 200,
            "case {case:?}: DeleteEndpoint must answer 200, got {} with {}",
            deleted.status, deleted.body
        );
    }
}
