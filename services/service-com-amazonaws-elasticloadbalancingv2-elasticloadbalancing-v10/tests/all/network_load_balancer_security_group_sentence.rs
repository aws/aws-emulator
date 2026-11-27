// SPDX-License-Identifier: CC0-1.0

//! The SENTENCE `SetSecurityGroups` refuses a Network Load Balancer created without any
//! security group with — over the wire, through the router.
//!
//! RULE under test: **the boundary this operation's own documentation states — "you can't perform
//! this operation on a Network Load Balancer unless you specified a security group for the load
//! balancer when you created it" — is answered with `InvalidConfigurationRequest` carrying the
//! fixed sentence Elastic Load Balancing words that boundary with, whatever the request asked
//! for: the message states the rule, so no part of it is read off the request.** A load balancer
//! the boundary is not about is served, and the OTHER boundary a security-group request can cross
//! carries its own sentence — so one refusal is never worded as the other.
//!
//! The rows differ along what the rule generalises over: how many groups the request lists and
//! which ones, whether the load balancer was created with a group at all, which family it is, and
//! which of this operation's two documented boundaries the request crosses.
//!
//! The wire harness (`emulator`, `call_on`, `assert_refused`) is the crate's own, as is the
//! wording of the operation's OTHER boundary (`PRIVATE_LINK_SETTING_UNAVAILABLE`, which the
//! suite that pins that boundary already states); the sentence under test here is written out
//! below, because its wording is what these rows pin.

use crate::load_balancer_attributes::{create_load_balancer, INVALID_CONFIGURATION};
use crate::load_balancer_network_capacity_and_policy::{
    assert_refused, call_on, PRIVATE_LINK_SETTING_UNAVAILABLE,
};
use crate::provisioning_loop::{emulator, members};

/// The sentence this operation refuses a Network Load Balancer created with no security group
/// with. Entirely fixed text: it names neither the load balancer nor any value the request
/// carried.
const NO_SECURITY_GROUPS_AT_CREATION: &str =
    "You cannot set security groups on a Network Load Balancer which was created without any \
     security groups.";

/// What one row expects of the call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expected {
    /// The groups the request listed are applied, and the answer reports them.
    Served,
    /// Refused with the sentence for a Network Load Balancer created without a group.
    NoSecurityGroupsAtCreation,
    /// Refused with the sentence for the PrivateLink setting being unavailable.
    PrivateLinkUnavailable,
}

/// One row: the fields the load balancer is created with, the groups the request lists, whether
/// it carries the PrivateLink member, and what the call must answer.
struct Row {
    case: &'static str,
    created_with: &'static [(&'static str, &'static str)],
    requested: &'static [&'static str],
    private_link: bool,
    expected: Expected,
}

const ROWS: &[Row] = &[
    Row {
        case: "a network load balancer created without a group, asked for one",
        created_with: &[("Type", "network")],
        requested: &["sg-00000001"],
        private_link: false,
        expected: Expected::NoSecurityGroupsAtCreation,
    },
    // The same boundary, a request of a different shape: the sentence is fixed text, so three
    // groups earn exactly the one group's.
    Row {
        case: "the same load balancer, asked for three groups",
        created_with: &[("Type", "network")],
        requested: &["sg-00000002", "sg-00000003", "sg-00000004"],
        private_link: false,
        expected: Expected::NoSecurityGroupsAtCreation,
    },
    // The other boundary of the same operation, on the same kind of load balancer: a request
    // carrying the PrivateLink member is refused for THAT member instead.
    Row {
        case: "the same load balancer, asked for one group and the PrivateLink setting",
        created_with: &[("Type", "network")],
        requested: &["sg-00000005"],
        private_link: true,
        expected: Expected::PrivateLinkUnavailable,
    },
    // The complement: a network load balancer that WAS created with a security group is not
    // what the boundary is about.
    Row {
        case: "a network load balancer created with a group",
        created_with: &[
            ("Type", "network"),
            ("SecurityGroups.member.1", "sg-00000006"),
        ],
        requested: &["sg-00000007"],
        private_link: false,
        expected: Expected::Served,
    },
    // Neither is an application load balancer, which carries security groups whether the create
    // listed any or not.
    Row {
        case: "an application load balancer created with no group at all",
        created_with: &[],
        requested: &["sg-00000008", "sg-00000009"],
        private_link: false,
        expected: Expected::Served,
    },
];

/// RULE: the refusal of a security-group change on a Network Load Balancer created without one
/// carries Elastic Load Balancing's own fixed sentence for that boundary, and the operation's
/// other boundary carries its own.
#[tokio::test]
async fn the_refusal_on_a_network_load_balancer_created_without_one_states_the_rule() {
    let (router, _engine) = emulator();
    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let name = format!("lb-sg-sentence-{index}");
        let arn = create_load_balancer(&router, &name, row.created_with).await;

        let mut fields: Vec<(String, String)> = row
            .requested
            .iter()
            .enumerate()
            .map(|(position, group)| {
                (
                    format!("SecurityGroups.member.{}", position + 1),
                    (*group).to_string(),
                )
            })
            .collect();
        if row.private_link {
            fields.push((
                "EnforceSecurityGroupInboundRulesOnPrivateLinkTraffic".to_string(),
                "on".to_string(),
            ));
        }
        let borrowed: Vec<(&str, &str)> = fields
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str()))
            .collect();
        let reply = call_on(&router, "SetSecurityGroups", &arn, &borrowed).await;

        match row.expected {
            Expected::Served => {
                assert_eq!(
                    reply.status, 200,
                    "case {case:?}: expected the change to be served, got {} body {}",
                    reply.status, reply.body
                );
                let answered = members(&reply.body, "SecurityGroupIds");
                let wanted: Vec<String> =
                    row.requested.iter().map(|group| (*group).to_string()).collect();
                assert_eq!(
                    answered, wanted,
                    "case {case:?}: expected the groups the request listed, got {answered:?}"
                );
            }
            Expected::NoSecurityGroupsAtCreation => assert_refused(
                case,
                &reply,
                INVALID_CONFIGURATION,
                NO_SECURITY_GROUPS_AT_CREATION,
            ),
            Expected::PrivateLinkUnavailable => assert_refused(
                case,
                &reply,
                INVALID_CONFIGURATION,
                PRIVATE_LINK_SETTING_UNAVAILABLE,
            ),
        }
    }
}
