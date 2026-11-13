// SPDX-License-Identifier: CC0-1.0

//! The Region a Multi-Region Access Point reports for each MEMBER bucket,
//! driven end to end over HTTP against Amazon S3 and S3 Control on one shared
//! engine.
//!
//! The rule under test: `RegionReport` is "a combination of a bucket and Region
//! that's part of a Multi-Region Access Point", while
//! `CreateMultiRegionAccessPoint`'s `Details.Regions` names each member only by
//! bucket (`Region$Bucket`, `Region$BucketAccountId`) — so the Region a report
//! carries for a member is resolved from THAT BUCKET, not from the Region the
//! create was signed for. A Multi-Region Access Point over buckets in two
//! Regions therefore reports two, and the same resolution is what
//! `ListMultiRegionAccessPoints` and the access point's routing configuration
//! report.
//!
//! The rows span what the rule generalises over: how many member buckets there
//! are, how many Regions they are spread over, and whether the create was signed
//! for one of those Regions or for none of them. Every member names a bucket the
//! object store HOLDS, because a create naming one it does not hold is not
//! carried out at all — see
//! `crate::multi_region_access_point_create_outcomes`, which drives that rule.

use std::time::Duration;

use service_com_amazonaws_s3control_awss3controlservicev20180820::state::{
    MULTI_REGION_ACCESS_POINT_SETTLE_AFTER, STATUS_READY,
};

use crate::access_point_lifecycle::{Reply, XMLNS};
use crate::object_store::{elements, ObjectStore, DEFAULT_BUCKET_REGION};

/// One row: the member buckets to create and where, the Region the create is
/// signed for, and the (bucket, Region) pairs the report must then carry.
struct Row {
    case: &'static str,
    name: &'static str,
    /// Each member bucket and the Region to create it in, or `None` for a
    /// member the object store is to hold no bucket for.
    members: &'static [(&'static str, Option<&'static str>)],
    /// The Region the `CreateMultiRegionAccessPoint` call is signed for.
    signed_for: &'static str,
}

impl Row {
    /// The (bucket, Region) pairs a report of this access point must carry: each
    /// member bucket's own Region, and the Region the create was signed for for
    /// a member no bucket answers for.
    fn expected(&self) -> Vec<(String, String)> {
        self.members
            .iter()
            .map(|(bucket, region)| {
                (
                    (*bucket).to_string(),
                    region.unwrap_or(self.signed_for).to_string(),
                )
            })
            .collect()
    }
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "two buckets in two Regions, signed for one of them",
            name: "mrap-two-regions",
            members: &[
                ("mrap-west-bucket", Some("us-west-2")),
                ("mrap-east-bucket", Some(DEFAULT_BUCKET_REGION)),
            ],
            signed_for: "us-west-2",
        },
        Row {
            case: "three buckets in three Regions, signed for none of them",
            name: "mrap-three-regions",
            members: &[
                ("mrap-eu-bucket", Some("eu-central-1")),
                ("mrap-ap-bucket", Some("ap-southeast-2")),
                ("mrap-sa-bucket", Some("sa-east-1")),
            ],
            signed_for: "us-west-2",
        },
        Row {
            case: "one bucket in the Region the create was signed for",
            name: "mrap-one-region",
            members: &[("mrap-local-bucket", Some("eu-west-1"))],
            signed_for: "eu-west-1",
        },
        Row {
            case: "two buckets in two Regions, neither the Region signed for",
            name: "mrap-remote-regions",
            members: &[
                ("mrap-tokyo-bucket", Some("ap-northeast-1")),
                ("mrap-london-bucket", Some("eu-west-2")),
            ],
            signed_for: "us-west-2",
        },
    ]
}

/// The leaf paths a `GetMultiRegionAccessPoint` report carries its regional
/// entries at, and the ones a listing carries them at.
const REPORT_BUCKETS: &str = "Bucket";
const REPORT_REGIONS: &str = "Region";

/// The (bucket, Region) pairs an answer carries, read from the `<Bucket>` and
/// `<Region>` elements of its regional entries in document order.
///
/// A `RegionReport` is written as `<Region><Bucket/><Region/><BucketAccountId/>`,
/// so the entry's own Region is the `<Region>` element that FOLLOWS each
/// `<Bucket>`; both lists are therefore read in document order and zipped.
fn reported_regions(answer: &Reply) -> Vec<(String, String)> {
    let buckets = elements(&answer.body, REPORT_BUCKETS);
    // The outer `<Region>` wrapper of each entry has no text of its own, so the
    // text-carrying `<Region>` elements are exactly the entries' Regions.
    let regions: Vec<String> = elements(&answer.body, REPORT_REGIONS)
        .into_iter()
        .filter(|value| !value.contains('<') && !value.is_empty())
        .collect();
    buckets.into_iter().zip(regions).collect()
}

/// How long to wait before reading the create the clock owes.
fn settled_wait() -> Duration {
    MULTI_REGION_ACCESS_POINT_SETTLE_AFTER + Duration::from_millis(250)
}

#[tokio::test]
async fn a_multi_region_access_point_reports_each_member_buckets_own_region() {
    for row in rows() {
        let store = ObjectStore::start();
        for (bucket, region) in row.members {
            if let Some(region) = region {
                store.create_bucket(row.case, region, bucket).await;
            }
        }

        let members: String = row
            .members
            .iter()
            .map(|(bucket, _)| format!("<Region><Bucket>{bucket}</Bucket></Region>"))
            .collect();
        let body = format!(
            "<CreateMultiRegionAccessPointRequest xmlns=\"{XMLNS}\">\
             <ClientToken>{}</ClientToken><Details><Name>{}</Name>\
             <Regions>{members}</Regions></Details>\
             </CreateMultiRegionAccessPointRequest>",
            row.name, row.name
        );
        let created = store
            .s3control(
                row.signed_for,
                "POST",
                "/v20180820/async-requests/mrap/create",
                &body,
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case {:?}: expected the create to answer 200, got {} with body {}",
            row.case, created.status, created.body
        );

        tokio::time::sleep(settled_wait()).await;

        let read = store
            .s3control(
                row.signed_for,
                "GET",
                &format!("/v20180820/mrap/instances/{}", row.name),
                "",
            )
            .await;
        assert_eq!(
            read.status, 200,
            "case {:?}: expected GetMultiRegionAccessPoint to answer 200, got {} with body {}",
            row.case, read.status, read.body
        );
        assert_eq!(
            read.text("Status").as_deref(),
            Some(STATUS_READY),
            "case {:?}: expected the create to have settled, got body {}",
            row.case,
            read.body
        );
        assert_eq!(
            reported_regions(&read),
            row.expected(),
            "case {:?}: expected each member bucket reported in its own Region, got body {}",
            row.case,
            read.body
        );

        // The LISTING reports the same resolution: one access point cannot be in
        // two sets of Regions depending on which read is asked.
        let listed = store
            .s3control(row.signed_for, "GET", "/v20180820/mrap/instances", "")
            .await;
        assert_eq!(
            listed.status, 200,
            "case {:?}: expected ListMultiRegionAccessPoints to answer 200, got {} with body {}",
            row.case, listed.status, listed.body
        );
        assert_eq!(
            reported_regions(&listed),
            row.expected(),
            "case {:?}: expected the listing to report the same member Regions, got body {}",
            row.case,
            listed.body
        );

        // So does the routing configuration — one route per member bucket. A
        // routing read addresses the access point by the ALIAS the service
        // assigned it, which is what the report above carries.
        let alias = read.text("Alias").unwrap_or_else(|| {
            panic!(
                "case {:?}: the report names the access point's alias, got body {}",
                row.case, read.body
            )
        });
        let routes = store
            .s3control(
                row.signed_for,
                "GET",
                &format!("/v20180820/mrap/instances/{alias}/routes"),
                "",
            )
            .await;
        assert_eq!(
            reported_regions(&routes),
            row.expected(),
            "case {:?}: expected the routes to name each member bucket's own Region, got body {}",
            row.case,
            routes.body
        );
    }
}
