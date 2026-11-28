// SPDX-License-Identifier: CC0-1.0

//! The WORDS a security-group refusal of this API carries, driven end to end over the wire
//! with Amazon EC2 — which owns the groups — mounted beside this service, so every refusal
//! below is the answer to a read EC2 itself made or refused.
//!
//! The rule asserted here: **a security group a load balancer cannot be GIVEN — an id of the
//! form EC2 mints that the account holds no group under, or a group governing traffic in
//! another VPC — is refused as a configuration that cannot hold together, and that refusal is
//! this API's own: `InvalidConfigurationRequest` (HTTP 400) with Elastic Load Balancing's
//! fixed sentence for the class, which names none of the ids the request sent, carries no
//! part of the refusal the lookup ran into (the service it was made to, the status, that
//! service's error code, the id of that call, a proxy clause) and nothing minted per call —
//! so two identical requests are answered the same text, with the request id carried in
//! `ErrorResponse/RequestId` where a caller reads it. A value that is no security group id at
//! all is a different class and stays the modelled `InvalidSecurityGroup`, which states the
//! fault the read ran into and so names the value at fault. Either way nothing is created and
//! nothing a load balancer carries is replaced.**
//!
//! `InvalidConfigurationRequestException` is modelled as "The requested configuration is not
//! valid." and `InvalidSecurityGroupException` as "The specified security group does not
//! exist."
//! (`smithy_query(query="shape",shape="InvalidConfigurationRequestException,InvalidSecurityGroupException")`),
//! neither of which fixes the text of a refusal; the sentence this API states for the
//! configuration class lives once in this crate's state module, which is where the rows read
//! it from rather than restating it.
//!
//! The rows differ along what the rule generalises over: the FORM of the value the request
//! carries (both id lengths EC2 mints, a prefix with nothing after it, a suffix between the
//! two lengths, a suffix that is not hexadecimal, a value with no prefix at all), what the
//! lookup answers about it (a group it reports, an id it refuses as unresolvable, an id it
//! refuses as unreadable), the VPC the group governs traffic in, how many groups one request
//! names, and which operation reads them — `CreateLoadBalancer` for the groups a new load
//! balancer starts with and `SetSecurityGroups` for the groups that replace them.
//!
//! WHICH class each input earns is `all/create_load_balancer_security_group_refusals.rs`;
//! this suite is about the text, and shares that suite's seeded network so the two cannot
//! drift on what EC2 holds. The wire harness (`action`, `call`, `members`, `p`) comes from
//! `provisioning_loop` and the EC2 front door (`emulator_with_ec2`) from
//! `target_security_group_health_check`.

use smithy_server_runtime::MultiServiceRouter;

use crate::create_load_balancer_security_group_refusals::{
    seed_network, Network, NOT_AN_ID, UNHELD_LONG_ID, UNHELD_SHORT_ID,
};
use crate::provisioning_loop::{action, call, members, p, Reply};
use crate::target_security_group_health_check::emulator_with_ec2;
use service_com_amazonaws_elasticloadbalancingv2_elasticloadbalancing_v10::state::INVALID_SECURITY_GROUPS;

/// The awsQuery codes the two classes answer under, from the model's `awsQueryError` traits.
const INVALID_CONFIGURATION_REQUEST: &str = "InvalidConfigurationRequest";
const INVALID_SECURITY_GROUP: &str = "InvalidSecurityGroup";

/// Fragments of the refusal a read of the account's groups can come back with, and of the
/// clause an SDK renders around such a failed call: the configuration refusal this API words
/// itself carries none of them. The service name and the inner error-code tokens are the ones
/// that read is refused under — one for an id no group carries, one for a value that is not a
/// group id at all.
const FOREIGN_FAULT_FRAGMENTS: &[&str] = &[
    "Service:",
    "Status Code:",
    "Error Code:",
    "Request ID:",
    "Proxy:",
    "AmazonEC2",
    "InvalidGroup",
    "Invalid id",
];

/// A value whose suffix is the id prefix and nothing else.
const PREFIX_ONLY: &str = "sg-";

/// A value whose suffix is one digit short of the long form EC2 mints.
const BETWEEN_LENGTHS: &str = "sg-0123456789abcdef";

/// A value of the long form's length whose suffix is not hexadecimal.
const NOT_HEXADECIMAL: &str = "sg-0123456789abcdefz";

/// Which groups a row names, resolved against the seeded network.
enum Groups {
    /// Literal values the request carries as given.
    Literal(&'static [&'static str]),
    /// The group of the load balancer's own VPC, followed by literal values.
    OwnPlus(&'static [&'static str]),
    /// The group of ANOTHER VPC.
    OtherVpc,
}

impl Groups {
    fn values(&self, network: &Network) -> Vec<String> {
        match self {
            Self::Literal(values) => values.iter().map(|value| (*value).to_string()).collect(),
            Self::OwnPlus(values) => std::iter::once(network.own_group.clone())
                .chain(values.iter().map(|value| (*value).to_string()))
                .collect(),
            Self::OtherVpc => vec![network.other_vpc_group.clone()],
        }
    }
}

/// What a row expects of the message, beside the error code.
enum Stated {
    /// The groups cannot be given to the load balancer: this API's own fixed sentence for the
    /// class, naming nothing the request sent and nothing of the lookup's own refusal.
    OwnSentence,
    /// The value is no security group id at all: the refusal states the fault the read ran
    /// into, which names the value at fault — held here as the value it must name, so the
    /// two classes are not asserted to word one message.
    FaultNaming(&'static str),
}

/// One row: the case, the groups the request names, and the code and the kind of message this
/// API answers for the class they fall in.
struct Row {
    case: &'static str,
    groups: Groups,
    code: &'static str,
    stated: Stated,
}

/// Every row of the rule, read by both operations below.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a long-form id this account holds no group under",
            groups: Groups::Literal(&[UNHELD_LONG_ID]),
            code: INVALID_CONFIGURATION_REQUEST,
            stated: Stated::OwnSentence,
        },
        Row {
            case: "a short-form id this account holds no group under",
            groups: Groups::Literal(&[UNHELD_SHORT_ID]),
            code: INVALID_CONFIGURATION_REQUEST,
            stated: Stated::OwnSentence,
        },
        Row {
            case: "a group governing traffic in another VPC",
            groups: Groups::OtherVpc,
            code: INVALID_CONFIGURATION_REQUEST,
            stated: Stated::OwnSentence,
        },
        Row {
            case: "a held group beside an id this account holds no group under",
            groups: Groups::OwnPlus(&[UNHELD_LONG_ID]),
            code: INVALID_CONFIGURATION_REQUEST,
            stated: Stated::OwnSentence,
        },
        Row {
            case: "two ids this account holds no group under",
            groups: Groups::Literal(&[UNHELD_LONG_ID, UNHELD_SHORT_ID]),
            code: INVALID_CONFIGURATION_REQUEST,
            stated: Stated::OwnSentence,
        },
        // The other class, which the rule does not collapse into the first: a value that is no
        // group id at all.
        Row {
            case: "a value with no group prefix at all",
            groups: Groups::Literal(&[NOT_AN_ID]),
            code: INVALID_SECURITY_GROUP,
            stated: Stated::FaultNaming(NOT_AN_ID),
        },
        Row {
            case: "the id prefix with nothing after it",
            groups: Groups::Literal(&[PREFIX_ONLY]),
            code: INVALID_SECURITY_GROUP,
            stated: Stated::FaultNaming(PREFIX_ONLY),
        },
        Row {
            case: "a suffix between the two lengths EC2 mints",
            groups: Groups::Literal(&[BETWEEN_LENGTHS]),
            code: INVALID_SECURITY_GROUP,
            stated: Stated::FaultNaming(BETWEEN_LENGTHS),
        },
        Row {
            case: "a suffix of the long form's length that is not hexadecimal",
            groups: Groups::Literal(&[NOT_HEXADECIMAL]),
            code: INVALID_SECURITY_GROUP,
            stated: Stated::FaultNaming(NOT_HEXADECIMAL),
        },
        Row {
            case: "a held group beside a value that is no group id at all",
            groups: Groups::OwnPlus(&[NOT_AN_ID]),
            code: INVALID_SECURITY_GROUP,
            stated: Stated::FaultNaming(NOT_AN_ID),
        },
    ]
}

/// Assert `reply` is the refusal `row` states: its code, and the message its class words.
fn assert_stated(case: &str, row: &Row, named: &[String], reply: &Reply) {
    assert_eq!(
        (reply.status, reply.error_code().as_str()),
        (400, row.code),
        "case {case:?}: expected HTTP 400 {}, got {} with {}",
        row.code,
        reply.status,
        reply.body
    );
    let message = reply.text("Message").unwrap_or_default();
    match row.stated {
        Stated::OwnSentence => {
            assert_eq!(
                message, INVALID_SECURITY_GROUPS,
                "case {case:?}: expected the sentence {INVALID_SECURITY_GROUPS:?}, got \
                 {message:?}"
            );
            for fragment in FOREIGN_FAULT_FRAGMENTS {
                assert!(
                    !message.contains(fragment),
                    "case {case:?}: expected no {fragment:?} in the message, got {message:?}"
                );
            }
            for value in named {
                assert!(
                    !message.contains(value),
                    "case {case:?}: expected the message to name no value the request sent, \
                     got {message:?}"
                );
            }
        }
        Stated::FaultNaming(value) => {
            assert!(
                message.contains(value),
                "case {case:?}: expected the refusal to name {value:?}, got {message:?}"
            );
            assert_ne!(
                message, INVALID_SECURITY_GROUPS,
                "case {case:?}: expected a refusal of its own class, got {message:?}"
            );
        }
    }
    // Whatever the class, the id of this call is reported where a caller reads it.
    assert!(
        reply
            .text("RequestId")
            .is_some_and(|id| !id.trim().is_empty()),
        "case {case:?}: expected a RequestId in the envelope, got {}",
        reply.body
    );
}

/// Ask for an internal Application Load Balancer across the seeded subnets, carrying
/// `groups`. Internal, so the placement needs no internet gateway on the seeded VPC: what is
/// under test is the security groups.
async fn create(
    router: &MultiServiceRouter,
    name: &str,
    network: &Network,
    groups: &[String],
) -> Reply {
    let mut form = action("CreateLoadBalancer");
    form.push(p("Name", name));
    form.push(p("Type", "application"));
    form.push(p("Scheme", "internal"));
    for (index, subnet) in network.subnets.iter().enumerate() {
        form.push(p(&format!("Subnets.member.{}", index + 1), subnet));
    }
    for (index, group) in groups.iter().enumerate() {
        form.push(p(&format!("SecurityGroups.member.{}", index + 1), group));
    }
    call(router, &form).await
}

/// Replace the groups of the load balancer `arn` names with `groups`.
async fn set_groups(router: &MultiServiceRouter, arn: &str, groups: &[String]) -> Reply {
    let mut form = action("SetSecurityGroups");
    form.push(p("LoadBalancerArn", arn));
    for (index, group) in groups.iter().enumerate() {
        form.push(p(&format!("SecurityGroups.member.{}", index + 1), group));
    }
    call(router, &form).await
}

#[tokio::test]
async fn create_load_balancer_states_this_api_s_own_security_group_refusal() {
    let router = emulator_with_ec2();
    let network = seed_network(&router).await;

    for (index, row) in rows().iter().enumerate() {
        let case = row.case;
        let named = row.groups.values(&network);
        let name = format!("sgwords-create-{index}");
        let reply = create(&router, &name, &network, &named).await;
        assert_stated(case, row, &named, &reply);

        // The configuration refusal this API words itself is the same text on every call:
        // nothing minted per call is interpolated into it, so a caller comparing two failures
        // of one request sees one message.
        if matches!(row.stated, Stated::OwnSentence) {
            let repeat = create(&router, &name, &network, &named).await;
            assert_eq!(
                repeat.text("Message"),
                reply.text("Message"),
                "case {case:?}: expected the repeat to state the same message, got {} then {}",
                reply.body,
                repeat.body
            );
        }

        // And a refused create stored nothing under the name it asked for.
        let mut read = action("DescribeLoadBalancers");
        read.push(p("Names.member.1", &name));
        let described = call(&router, &read).await;
        assert_eq!(
            (described.status, described.error_code().as_str()),
            (400, "LoadBalancerNotFound"),
            "case {case:?}: a refused create must store nothing, got {} with {}",
            described.status,
            described.body
        );
    }
}

#[tokio::test]
async fn set_security_groups_states_the_same_words_as_the_create() {
    let router = emulator_with_ec2();
    let network = seed_network(&router).await;

    let created = create(
        &router,
        "sgwords-replace",
        &network,
        std::slice::from_ref(&network.own_group),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case the load balancer whose groups are replaced: expected the create to be served, \
         got {} with {}",
        created.status, created.body
    );
    let arn = created
        .text("LoadBalancerArn")
        .expect("the created load balancer reports its ARN");

    for row in rows() {
        let case = row.case;
        let named = row.groups.values(&network);
        let reply = set_groups(&router, &arn, &named).await;
        assert_stated(case, &row, &named, &reply);

        // The refused replacement left the groups the load balancer carries alone.
        let mut read = action("DescribeLoadBalancers");
        read.push(p("LoadBalancerArns.member.1", &arn));
        let described = call(&router, &read).await;
        assert_eq!(
            (described.status, members(&described.body, "SecurityGroups")),
            (200, vec![network.own_group.clone()]),
            "case {case:?}: a refused replacement must change nothing, got {}",
            described.body
        );
    }
}
