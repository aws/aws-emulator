// SPDX-License-Identifier: CC0-1.0

//! WHEN the directory an `AddRegion` names is resolved, relative to the placement its
//! `VPCSettings` describe.
//!
//! RULE: an operation whose subject is a STORED directory resolves that directory against
//! this service's state — answering an id it cannot resolve with the modelled
//! `DirectoryDoesNotExistException` and this service's one sentence for that failure
//! (`Directory: '<id>' does not exist.`) — BEFORE the VPC and subnets the request asks for
//! are resolved against the service that OWNS them. Nothing a placement says can make a
//! call against a directory that does not exist answerable, so the answer reports the
//! missing subject and points the caller at its only recovery instead of at a network the
//! call would never have used.
//!
//! Both services are mounted on ONE engine here
//! (`crate::directory_dns_addresses::network`), and the network every row names is built
//! through EC2's own API, because a placement refusal is grounded in what EC2 answered: an
//! emulator with no EC2 beside this service refuses no placement at all, so the ordering
//! would be unobservable.
//!
//! The tables span what the rule generalises over:
//!
//! * the DIRECTORY ID form — four well-formed ids (`DirectoryId` is
//!   `pattern ^d-[0-9a-f]{10}$`, `smithy_query query="operation" operation="AddRegion"`)
//!   this account holds no directory under;
//! * the PLACEMENT, over the two things EC2 grounds a refusal in: a VPC the account does
//!   not have, and two subnets of the account that sit in ONE Availability Zone.
//!
//! The control is what shows the placement resolution was not simply dropped: the SAME
//! placements against a directory this account DOES hold are still refused, in the
//! resource-semantics class. The served case — a replication into a Region whose placement
//! resolves — is `crate::region_replication`'s.

use serde_json::{json, Value};

use crate::directory_dns_addresses::{network, AccountSubnet};
use crate::directory_lifecycle::{call, ok, string, PASSWORD, REGION};

/// Well-formed `DirectoryId`s this account holds no directory under: the `d-` prefix and
/// ten hexadecimal digits, with different digits and widths of value, so no row can be
/// refused for its form instead of for naming nothing.
const ABSENT_DIRECTORIES: [(&str, &str); 4] = [
    ("d-1234567890", "ten decimal digits"),
    ("d-0000000000", "all zeros"),
    ("d-ffffffffff", "all f"),
    ("d-0a1b2c3d4e", "every digit class"),
];

/// Well-formed `VpcId`s the account has no VPC for — the form EC2 mints one in, at both
/// published digit widths.
const ABSENT_VPCS: [&str; 2] = ["vpc-0a1b2c3d", "vpc-00000000000000000"];

/// The identifiers of `subnets`, as the owner of subnets minted them.
fn subnet_ids(subnets: &[AccountSubnet]) -> Vec<String> {
    subnets.iter().map(|subnet| subnet.id.clone()).collect()
}

/// An `AddRegion` body for `directory_id`, placing the added Region's servers in `vpc` and
/// `subnets`.
fn add_region(directory_id: &str, vpc: &str, subnets: &[String]) -> Value {
    json!({
        "DirectoryId": directory_id,
        "RegionName": REGION,
        "VPCSettings": { "VpcId": vpc, "SubnetIds": subnets },
    })
}

/// The placements no row can have: each one is something only the owner of VPCs and
/// subnets can refuse, so a call that reached that resolution answers from it.
///
/// `(case, vpc, subnet indices into the account's own subnets)` — the account's subnets sit
/// in the zones `crate::directory_dns_addresses` names, where the first and the fourth
/// share one zone.
fn unresolvable_placements() -> Vec<(String, String, [usize; 2])> {
    let mut rows = Vec::new();
    for absent in ABSENT_VPCS {
        rows.push((
            format!("a VPC the account does not have ({absent})"),
            absent.to_string(),
            [0, 1],
        ));
    }
    rows.push((
        "two subnets of the account in one Availability Zone".to_string(),
        String::new(),
        [0, 3],
    ));
    rows
}

// ---------------------------------------------------------------------------
// RULE: the directory is resolved first, so an `AddRegion` naming one that does not exist
// is the modelled not-found — whatever the placement it asks for.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn add_region_on_an_absent_directory_reports_the_directory_not_the_placement() {
    let network = network().await;
    let router = &network.router;

    for (why_placement, vpc, picked) in unresolvable_placements() {
        // An empty VPC in the row means "the VPC the account holds", which is what the
        // one-zone row needs so that only its subnets are at fault.
        let vpc = if vpc.is_empty() {
            network.vpc.clone()
        } else {
            vpc
        };
        let subnets = subnet_ids(&[
            network.client[picked[0]].clone(),
            network.client[picked[1]].clone(),
        ]);
        for (absent, form) in ABSENT_DIRECTORIES {
            let case = format!("{why_placement} / a directory id of {form}");
            let (status, parsed) = call(router, "AddRegion", add_region(absent, &vpc, &subnets)).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected the absent directory to be refused, got {status} \
                 with {parsed}"
            );
            assert_eq!(
                parsed["__type"],
                json!("DirectoryDoesNotExistException"),
                "case {case:?}: the missing directory must be reported before the \
                 placement, got {parsed}"
            );
            let request_id = string(&parsed, "RequestId", &case);
            assert_eq!(
                parsed["Message"].as_str().unwrap_or_default(),
                format!("Directory: '{absent}' does not exist. : RequestId: {request_id}"),
                "case {case:?}: expected this service's not-found sentence, got {parsed}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// CONTROL: the rule is about ORDER, not about dropping the placement resolution — against
// a directory this account holds, every placement above is still refused, and in the
// resource-semantics class.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn once_the_directory_resolves_the_same_placement_is_still_refused() {
    let network = network().await;
    let router = &network.router;
    let held = subnet_ids(&network.client[..2]);

    let created = ok(
        router,
        "CreateMicrosoftAD",
        json!({
            "Name": "add-region-order.example.com",
            "Password": PASSWORD,
            "Edition": "Enterprise",
            "VpcSettings": { "VpcId": &network.vpc, "SubnetIds": &held },
        }),
        "the directory the control replicates",
    )
    .await;
    let directory_id = string(&created, "DirectoryId", "the directory the control replicates");

    for (why_placement, vpc, picked) in unresolvable_placements() {
        let vpc = if vpc.is_empty() {
            network.vpc.clone()
        } else {
            vpc
        };
        let subnets = subnet_ids(&[
            network.client[picked[0]].clone(),
            network.client[picked[1]].clone(),
        ]);
        let case = format!("a stored directory / {why_placement}");
        let (status, parsed) = call(
            router,
            "AddRegion",
            add_region(&directory_id, &vpc, &subnets),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: on a directory that resolves, the placement is still resolved, \
             got {status} with {parsed}"
        );
        assert_eq!(
            parsed["__type"],
            json!("ClientException"),
            "case {case:?}: expected the resource-semantics refusal class, got {parsed}"
        );
        assert!(
            parsed["Message"]
                .as_str()
                .is_some_and(|message| !message.is_empty()),
            "case {case:?}: the refusal must say what the placement broke, got {parsed}"
        );
    }
}

/// RULE, the other half: a MALFORMED directory id names nothing in any account, so it is
/// refused before either resolution — the validation class, not the not-found.
#[tokio::test]
async fn an_add_region_naming_no_directory_id_at_all_is_refused_as_a_parameter() {
    let network = network().await;
    let router = &network.router;
    let held = subnet_ids(&network.client[..2]);

    // The rows span the ways a value is outside `^d-[0-9a-f]{10}$`: no prefix, too few
    // digits, too many, and a digit outside hexadecimal.
    let rows: [(&str, &str); 4] = [
        ("no prefix at all", "1234567890"),
        ("too few digits", "d-123456789"),
        ("too many digits", "d-12345678901"),
        ("a digit outside hexadecimal", "d-123456789z"),
    ];
    for (case, malformed) in rows {
        let (status, parsed) = call(
            router,
            "AddRegion",
            add_region(malformed, &network.vpc, &held),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a malformed directory id must be refused, got {status} with \
             {parsed}"
        );
        assert_ne!(
            parsed["__type"],
            json!("DirectoryDoesNotExistException"),
            "case {case:?}: a value outside the member's form names nothing, so it is not \
             reported as a missing directory, got {parsed}"
        );
    }
}
