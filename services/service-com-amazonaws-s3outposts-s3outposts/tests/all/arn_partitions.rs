// SPDX-License-Identifier: CC0-1.0

//! The partition field of every ARN this service reports, end to end over HTTP.
//!
//! The rule under test has two halves. An ARN names its resource in FULL, so the partition
//! field and the Region field of one ARN describe the same place — the partition is a
//! function of the Region the request was signed for, never a constant. And an ARN member
//! can only carry a value its own pattern admits: `EndpointArn`, `OutpostArn` and
//! `S3OutpostArn` each spell their partition field out as a CLOSED alternation
//! (`^arn:(aws|aws-cn|aws-us-gov|aws-iso|aws-iso-b):…`) beside a Region field their pattern
//! leaves open, so a request signed for a GovCloud Region owes `aws-us-gov` ARNs, one
//! signed for a China Region owes `aws-cn` ARNs — and one signed for a Region whose
//! partition that alternation does not list owes NO value for the member at all, which,
//! every ARN member here being optional, means the member is absent while the resource
//! itself is reported as usual.
//!
//! Every case drives the four operations that report an ARN — `CreateEndpoint`,
//! `ListEndpoints`, `ListSharedEndpoints` and `ListOutpostsWithS3` — through the router in
//! the row's Region, so each answer is the one a client signing for that Region receives.

use serde_json::{json, Value};

use crate::arn_patterns::optional_member;
use crate::endpoint_lifecycle::{
    create_in, emulator_holding, emulator_in_region, endpoint_id_of, list_endpoints_in,
    list_outposts_in, list_shared_in, minimal_create, owned_outposts_in, shared_outpost_in,
    shared_query, string_at, Answer, WHOLE_PAGE,
};

/// A Region and the partition its ARNs belong to.
///
/// The Region codes are taken from the emulator's own Region catalog
/// (`emulator_telemetry::policy::KNOWN_REGIONS`, "every AWS region the wire may carry",
/// plus `eusc-de-east-1`, which the models' own Region value sets publish) and
/// the partition names from this service's ARN patterns
/// (smithy_query(query="shape", shape="EndpointArn,OutpostArn,S3OutpostArn")), whose
/// alternation lists `aws`, `aws-cn`, `aws-us-gov`, `aws-iso` and `aws-iso-b` and nothing
/// else.
struct Row {
    case: &'static str,
    region: &'static str,
    /// The partition this Region's ARNs name, when the members' own alternation lists it —
    /// and `None` for a Region in a partition that alternation does NOT list, where no ARN
    /// member of this service has a legal value at all and so carries none.
    partition: Option<&'static str>,
}

/// One row per partition the catalog reaches, plus a second Region in the partitions that
/// have one, so a case cannot pass by matching a single Region code.
const ROWS: &[Row] = &[
    Row {
        case: "the commercial partition",
        region: "us-east-1",
        partition: Some("aws"),
    },
    Row {
        case: "another commercial Region",
        region: "ap-southeast-2",
        partition: Some("aws"),
    },
    Row {
        case: "a commercial Region whose code is longest",
        region: "ap-northeast-3",
        partition: Some("aws"),
    },
    Row {
        case: "the China partition",
        region: "cn-north-1",
        partition: Some("aws-cn"),
    },
    Row {
        case: "the other China Region",
        region: "cn-northwest-1",
        partition: Some("aws-cn"),
    },
    Row {
        case: "GovCloud (US-West)",
        region: "us-gov-west-1",
        partition: Some("aws-us-gov"),
    },
    Row {
        case: "GovCloud (US-East)",
        region: "us-gov-east-1",
        partition: Some("aws-us-gov"),
    },
    Row {
        case: "the isolated partition",
        region: "us-iso-east-1",
        partition: Some("aws-iso"),
    },
    Row {
        case: "the other isolated Region",
        region: "us-iso-west-1",
        partition: Some("aws-iso"),
    },
    Row {
        case: "the second isolated partition, whose prefix extends the first",
        region: "us-isob-east-1",
        partition: Some("aws-iso-b"),
    },
    Row {
        case: "the European Sovereign Cloud, outside the alternation",
        region: "eusc-de-east-1",
        partition: None,
    },
    Row {
        case: "an isolated partition the ARN patterns' alternation does not list",
        region: "us-isof-east-1",
        partition: None,
    },
    Row {
        case: "the European isolated partition, also outside the alternation",
        region: "eu-isoe-west-1",
        partition: None,
    },
];

/// The partition field of an ARN: its second colon-separated field.
fn partition_of(arn: &str) -> &str {
    let mut fields = arn.split(':');
    assert_eq!(
        fields.next(),
        Some("arn"),
        "{arn:?} must begin with the literal arn"
    );
    fields
        .next()
        .unwrap_or_else(|| panic!("{arn:?} carries no partition field"))
}

/// The one endpoint a listing reports, and the `EndpointArn` member it carries for it —
/// `None` where the answer carries no such member.
fn the_only_endpoint(answer: &Answer, case: &str, listing: &str) -> (Value, Option<String>) {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: {listing} must answer 200, got {} with {:?}",
        answer.status, answer.body
    );
    let endpoints = answer.endpoints();
    assert_eq!(
        endpoints.len(),
        1,
        "case {case:?}: {listing} must report exactly the endpoint created in this Region, \
         got {:?}",
        answer.body
    );
    let endpoint = endpoints[0].clone();
    let arn = optional_member(&endpoint, "EndpointArn");
    (endpoint, arn)
}

/// The Outpost `outpost_id` in `answer`.
fn outpost_with_id(answer: &Answer, outpost_id: &str) -> Value {
    answer
        .outposts()
        .iter()
        .find(|outpost| outpost["OutpostId"].as_str() == Some(outpost_id))
        .unwrap_or_else(|| panic!("no Outpost {outpost_id} in {:?}", answer.body))
        .clone()
}

/// The `EndpointArn` a create in `region` owes for `outpost_id`, read off the member's own
/// pattern: the whole ARN in the Region's partition where the alternation lists it, and
/// nothing where it does not.
fn expected_endpoint_arn(
    partition: Option<&str>,
    region: &str,
    account: &str,
    outpost_id: &str,
    reported: Option<&str>,
) -> Option<String> {
    let partition = partition?;
    let endpoint_id = endpoint_id_of(reported?);
    Some(format!(
        "arn:{partition}:s3-outposts:{region}:{account}:outpost/{outpost_id}/endpoint/{endpoint_id}"
    ))
}

/// Every ARN an answer reports names the partition of the Region the request was signed
/// for — the endpoint ARN `CreateEndpoint` mints and the listings report, and the Outpost's
/// own two ARNs — and where that partition is one the member's own alternation does not
/// list, the member is absent while the endpoint and the Outpost are still reported.
#[tokio::test]
async fn every_arn_names_the_partition_of_the_region_the_request_was_signed_for() {
    let account = engine::emulator_account_id();
    let mut absent_rows = 0;

    for Row {
        case,
        region,
        partition,
    } in ROWS
    {
        // Its own emulator per row, whose Outposts are installed in the row's Region, so
        // the endpoint the row creates is the only one of this account in the answer.
        let router = emulator_in_region(region);
        let outpost_id = owned_outposts_in(&router, region)
            .await
            .first()
            .expect("the emulator presents an owned Outpost")
            .clone();

        let created = create_in(&router, region, minimal_create(&outpost_id)).await;
        assert_eq!(
            created.status, 200,
            "case {case:?} ({region}): CreateEndpoint must answer 200, got {} with {:?}",
            created.status, created.body
        );
        let reported = optional_member(&created.body, "EndpointArn");
        assert_eq!(
            reported.is_some(),
            partition.is_some(),
            "case {case:?} ({region}): the EndpointArn member must be carried exactly where \
             the member's own alternation lists this Region's partition, got {:?}",
            created.body
        );
        let expected = expected_endpoint_arn(
            *partition,
            region,
            account,
            &outpost_id,
            reported.as_deref(),
        );
        assert_eq!(
            reported, expected,
            "case {case:?} ({region}): expected EndpointArn {expected:?} — the ARN in this \
             Region's own partition where the member admits it, nothing where it does not — \
             got {reported:?}"
        );
        if reported.is_none() {
            absent_rows += 1;
        }

        // Stored either way: the listing of this account's own endpoints reports the
        // endpoint, carrying the very same EndpointArn member the create answered with.
        let (endpoint, listed_arn) = the_only_endpoint(
            &list_endpoints_in(&router, region, WHOLE_PAGE).await,
            case,
            "ListEndpoints",
        );
        assert_eq!(
            string_at(&endpoint, "OutpostsId"),
            outpost_id,
            "case {case:?} ({region}): ListEndpoints must report the endpoint on its own \
             Outpost, got {endpoint:?}"
        );
        assert_eq!(
            listed_arn, reported,
            "case {case:?} ({region}): ListEndpoints must report the same EndpointArn member \
             the create answered with ({reported:?}), got {listed_arn:?}"
        );

        // The Outpost-scoped listing serves an Outpost another account shared with this
        // caller (`shared_endpoint_listing.rs`), so an endpoint created there is read back
        // through it — under the very same member rule.
        let shared_outpost = shared_outpost_in(&router, region).await;
        const SHARED_SUBNET: &str = "subnet-0000000000000f001";
        let mut request = minimal_create(&shared_outpost);
        request["SubnetId"] = json!(SHARED_SUBNET);
        request["SecurityGroupId"] = json!("sg-0000000000000f001");
        let on_shared = create_in(&router, region, request).await;
        assert_eq!(
            on_shared.status, 200,
            "case {case:?} ({region}): CreateEndpoint on the shared Outpost must answer 200, \
             got {} with {:?}",
            on_shared.status, on_shared.body
        );
        let shared_arn = optional_member(&on_shared.body, "EndpointArn");
        assert_eq!(
            shared_arn.is_some(),
            partition.is_some(),
            "case {case:?} ({region}): the EndpointArn member must be carried exactly where \
             the member's own alternation lists this Region's partition, got {:?}",
            on_shared.body
        );
        let listing = list_shared_in(&router, region, &shared_query(&shared_outpost)).await;
        assert_eq!(
            listing.status, 200,
            "case {case:?} ({region}): ListSharedEndpoints must answer 200, got {} with {:?}",
            listing.status, listing.body
        );
        let listed = listing
            .endpoints()
            .iter()
            .find(|endpoint| endpoint["SubnetId"] == json!(SHARED_SUBNET))
            .cloned()
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?} ({region}): ListSharedEndpoints must report the endpoint \
                     created on {shared_outpost}, got {:?}",
                    listing.body
                )
            });
        assert_eq!(
            string_at(&listed, "OutpostsId"),
            shared_outpost,
            "case {case:?} ({region}): ListSharedEndpoints must report the endpoint on its own \
             Outpost, got {listed:?}"
        );
        assert_eq!(
            optional_member(&listed, "EndpointArn"),
            shared_arn,
            "case {case:?} ({region}): ListSharedEndpoints must report the same EndpointArn \
             member the create answered with ({shared_arn:?}), got {listed:?}"
        );
        if let Some(arn) = &shared_arn {
            assert_eq!(
                Some(partition_of(arn)),
                *partition,
                "case {case:?} ({region}): the EndpointArn must name this Region's partition, \
                 got {arn:?}"
            );
        }

        let outposts = list_outposts_in(&router, region, WHOLE_PAGE).await;
        assert_eq!(
            outposts.status, 200,
            "case {case:?} ({region}): ListOutpostsWithS3 must answer 200, got {} with {:?}",
            outposts.status, outposts.body
        );
        let outpost = outpost_with_id(&outposts, &outpost_id);
        let expected_outpost_arn = partition
            .map(|p| format!("arn:{p}:outposts:{region}:{account}:outpost/{outpost_id}"));
        assert_eq!(
            optional_member(&outpost, "OutpostArn"),
            expected_outpost_arn,
            "case {case:?} ({region}): expected OutpostArn {expected_outpost_arn:?}, got \
             {outpost:?}"
        );
        let expected_s3_arn = partition
            .map(|p| format!("arn:{p}:s3-outposts:{region}:{account}:outpost/{outpost_id}/s3"));
        assert_eq!(
            optional_member(&outpost, "S3OutpostArn"),
            expected_s3_arn,
            "case {case:?} ({region}): expected S3OutpostArn {expected_s3_arn:?}, got \
             {outpost:?}"
        );
    }

    // The table covers both halves of the rule, so a service that always filled the member
    // in — with a partition its own pattern rejects — would fail above.
    assert!(
        absent_rows > 0,
        "the table must cover Regions in a partition the ARN members' alternation does not \
         list, got {absent_rows} such rows"
    );
}

/// The partition is a function of the Region ALONE: one store holding an Outpost in every
/// row's Region reports each one in that Region's partition — no ARN of an answer names a
/// partition another ARN of the same answer contradicts, no two Regions in different
/// partitions report the same partition, and each Region's answer carries only what is
/// installed in it.
#[tokio::test]
async fn one_store_reports_each_region_s_own_partition_consistently() {
    let caller = engine::emulator_account_id().to_string();
    // One Outpost per row, in that row's Region, all in ONE store: the records differ only
    // in the Region they are installed in, so any difference in the answers is the
    // projection under test.
    let declared: Vec<(String, &Row)> = ROWS
        .iter()
        .enumerate()
        .map(|(index, row)| (format!("op-{index:017x}"), row))
        .collect();
    let router = emulator_holding(
        &declared
            .iter()
            .map(|(outpost_id, row)| (outpost_id.clone(), row.region))
            .collect::<Vec<_>>(),
    );

    // One endpoint of this account on each of them, created through the Region the Outpost
    // is installed in.
    for (outpost_id, row) in &declared {
        let created = create_in(&router, row.region, minimal_create(outpost_id)).await;
        assert_eq!(
            created.status, 200,
            "case {:?} ({}): CreateEndpoint must answer 200, got {} with {:?}",
            row.case, row.region, created.status, created.body
        );
    }

    for (outpost_id, row) in &declared {
        let Row {
            case,
            region,
            partition,
        } = row;

        let listed = list_endpoints_in(&router, region, WHOLE_PAGE).await;
        let (_, endpoint_arn) = the_only_endpoint(&listed, case, "ListEndpoints");
        let expected = expected_endpoint_arn(
            *partition,
            region,
            &caller,
            outpost_id,
            endpoint_arn.as_deref(),
        );
        assert_eq!(
            endpoint_arn, expected,
            "case {case:?} ({region}): expected EndpointArn {expected:?} — this Region and \
             its own partition, or nothing where the member's alternation does not list that \
             partition — got {endpoint_arn:?}"
        );

        let outposts = list_outposts_in(&router, region, WHOLE_PAGE).await;
        let reported: Vec<String> = outposts
            .outposts()
            .iter()
            .map(|outpost| string_at(outpost, "OutpostId"))
            .collect();
        assert_eq!(
            reported,
            vec![outpost_id.clone()],
            "case {case:?} ({region}): the inventory must report exactly the Outpost installed \
             in this Region, got {reported:?}"
        );
        let outpost = outpost_with_id(&outposts, outpost_id);
        let outpost_arn = optional_member(&outpost, "OutpostArn");
        let s3_outpost_arn = optional_member(&outpost, "S3OutpostArn");
        for (member, reported) in [
            ("OutpostArn", &outpost_arn),
            ("S3OutpostArn", &s3_outpost_arn),
        ] {
            assert_eq!(
                reported.is_some(),
                partition.is_some(),
                "case {case:?} ({region}): {member} must be carried exactly where its own \
                 alternation lists this Region's partition, got {reported:?}"
            );
        }

        // Where the members do carry an ARN, every one of them names this Region and the
        // one partition it belongs to: no answer contradicts itself.
        for reported in [&endpoint_arn, &outpost_arn, &s3_outpost_arn]
            .into_iter()
            .flatten()
        {
            assert_eq!(
                Some(partition_of(reported)),
                *partition,
                "case {case:?} ({region}): every ARN of the answer must name that Region's \
                 partition, got {reported:?}"
            );
            assert!(
                reported.contains(&format!(":{region}:")),
                "case {case:?}: an ARN naming partition {partition:?} must name the Region it \
                 belongs to, got {reported:?}"
            );
        }
    }
}
