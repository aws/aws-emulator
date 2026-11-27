// SPDX-License-Identifier: CC0-1.0

//! What a self-deployed `CreateEnvironment` whose `vpcId` names no VPC of the account SAYS,
//! over awsJson1_0.
//!
//! THE RULE under test: the refusal reports one failed precondition per check that failed,
//! and the `hosts` a `vcfVersion` of `SELF_DEPLOYED` does not support fail none — "Amazon EVS
//! provisions only the VLAN subnets; no hosts are added and no VCF installation is performed"
//! (`CreateEnvironment` doc), so nothing the service derives reads a fact out of an entry. A
//! request whose only failed precondition is the VPC therefore draws THAT clause and no other:
//! the whole aggregated `message` is the prefix plus the VPC clause, with no join in it,
//! however many Amazon EC2 resources the inert entries name. The service access subnet is
//! judged INSIDE the VPC the request named, so an unknown VPC leaves it nothing to resolve
//! against either.
//!
//! This is the consequence a client reads, and the reason the rule is worth asserting on the
//! message rather than only on the `fieldList`: the refusal has to send the caller to the one
//! member that is wrong. Which members each mode draws a clause FOR is tabled by the sibling
//! suite `tests/all/self_deployed_host_placement_inertness.rs`; what this one adds is the
//! text the aggregation produces when the inert ones draw none.
//!
//! The rows differ along the `vpcId` form the member's own pattern publishes and along which
//! references the inert entries carry. Amazon EVS and Amazon EC2 are mounted on ONE shared
//! engine (the sibling suites' own harness), so the VPC really is looked for in Amazon EC2's
//! own store, and every call goes over the wire a client makes it on.

use serde_json::{json, Value};

use crate::create_environment_validation::{assert_aggregated_refusal, environment_count};
use crate::environment_lifecycle::{call, four_hosts, vlans_with_prefix};
use crate::vlan_subnet_propagation::emulator as evs_with_ec2;

/// The mode that adds no host (`smithy_query(query="shape", shape="VcfVersion")`:
/// `SELF_DEPLOYED | VCF-5.2.1 | VCF-5.2.2`).
const SELF_DEPLOYED: &str = "SELF_DEPLOYED";

/// A /24 VLAN subnet, well inside the published /24../28 range, so no VLAN block of its own
/// fails a check.
const VLAN_PREFIX_LEN: u8 = 24;

/// The clause Amazon EVS states for a `vpcId` the account holds no VPC for, and the text the
/// aggregated `message` opens with — both written out here rather than read from the crate, so
/// a change of wording fails this suite. `<vpcId>` stands for the id taken from the request;
/// the rest is fixed text for the class.
const INVALID_VPC_CLAUSE: &str = "The VPC id <vpcId> is not valid or does not exist. Please \
                                  verify the VPC id exists and is valid.";
/// See [`INVALID_VPC_CLAUSE`].
const VALIDATION_FAILURE_PREFIX: &str = "Request validation failure: ";
/// What the aggregated `message` joins two clauses with, so a message carrying exactly ONE
/// clause can be shown to carry no join at all.
const VALIDATION_FAILURE_SEPARATOR: &str = " | ";

/// Well-formed `vpcId` values of the two forms the member's own pattern publishes —
/// `^vpc-[a-f0-9]{8}([a-f0-9]{9})?$` — that the account holds no VPC for.
const UNHELD_VPC_SHORT: &str = "vpc-00000000";
/// See [`UNHELD_VPC_SHORT`].
const UNHELD_VPC_LONG: &str = "vpc-0123456789abcdef0";

/// A well-formed `serviceAccessSubnetId` the account holds no subnet for.
const UNHELD_SUBNET: &str = "subnet-00000000";

/// Amazon EC2 ids of the forms the `hosts` members' own patterns publish that nothing holds.
const UNHELD_GROUP: &str = "pg-00000000";
/// See [`UNHELD_GROUP`].
const UNHELD_DEDICATED_HOST: &str = "h-0123456789abcdef0";

/// RULE: a self-deployed create whose `vpcId` names no VPC is refused for the VPC alone, and
/// the whole message is the prefix plus that one clause.
#[tokio::test]
async fn a_self_deployed_create_naming_no_vpc_is_refused_for_the_vpc_alone() {
    // (case, the vpcId nothing holds, what the first inert entry names beyond a host). Every
    // entry of `four_hosts` names the key pair the crate's host fixtures use, which this
    // emulator's Amazon EC2 store holds none of — so each row's entries carry an unresolvable
    // `keyName` as well.
    let rows: Vec<(&str, &str, Value)> = vec![
        (
            "a short-form vpcId, with entries naming an unheld key pair",
            UNHELD_VPC_SHORT,
            json!({}),
        ),
        (
            "a long-form vpcId, with an entry also naming an unheld placement group",
            UNHELD_VPC_LONG,
            json!({ "placementGroupId": UNHELD_GROUP }),
        ),
        (
            "a short-form vpcId, with an entry also naming an unheld Dedicated Host",
            UNHELD_VPC_SHORT,
            json!({ "dedicatedHostId": UNHELD_DEDICATED_HOST }),
        ),
    ];

    for (case, vpc_id, extra) in rows {
        let router = evs_with_ec2();
        let request = json!({
            "vpcId": vpc_id,
            "serviceAccessSubnetId": UNHELD_SUBNET,
            "vcfVersion": SELF_DEPLOYED,
            "termsAccepted": true,
            "initialVlans": vlans_with_prefix(VLAN_PREFIX_LEN),
            "hosts": four_hosts(extra),
        });
        let (status, body) = call(&router, "CreateEnvironment", &request).await;
        // ONE failed precondition, reported once in `fieldList` and once in the joined
        // `message` — the aggregation rule `tests/all/create_environment_validation.rs` owns.
        assert_aggregated_refusal(case, status, &body, &["vpcId"]);
        let said = body["message"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no message in {body}"));
        let expected = format!(
            "{VALIDATION_FAILURE_PREFIX}{}",
            INVALID_VPC_CLAUSE.replace("<vpcId>", vpc_id)
        );
        assert_eq!(
            said, expected,
            "case {case:?}: expected {expected:?}, got {said:?}"
        );
        assert!(
            !said.contains(VALIDATION_FAILURE_SEPARATOR),
            "case {case:?}: one failed check is joined to nothing, got {said:?}"
        );
        assert_eq!(
            environment_count(&router).await,
            0,
            "case {case:?}: a refused create must store no environment"
        );
    }
}
