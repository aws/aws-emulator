// SPDX-License-Identifier: CC0-1.0

//! The host a load balancer publishes as its `DNSName`, over the wire.
//!
//! One rule: the value is the address a client that honours it reaches the load balancer
//! AT, and the FAMILY decides what that is:
//!
//! * an Application or Network Load Balancer — whose listeners carry a client's own traffic
//!   (an Application Load Balancer's answer its request, a Network Load Balancer's hand the
//!   connection to what its target group selects) — publishes the `host:port` of the socket
//!   the emulator opened for it, an address a client can dial. BOTH schemes publish it:
//!   `Scheme` decides which clients may reach the load balancer in a real network (an
//!   internet-facing one's "nodes … have public IP addresses", while an internal one's "have
//!   only private IP addresses" and "can route requests only from clients with access to the
//!   VPC for the load balancer"), and in this emulator every such client — a function, an
//!   instance, a CloudFront VPC origin, a client on this machine — reaches the load balancer
//!   at the one address this emulator serves it on;
//! * a Gateway Load Balancer gets no socket: its `GENEVE` listeners carry no
//!   request of that kind, so it keeps the host Elastic Load Balancing's own zone gives it,
//!   `<name>-<hexadecimal id>.elb.<region>.amazonaws.com`
//!   (`my-network-load-balancer-1234567890abcdef.elb.us-west-2.amazonaws.com`), whose id
//!   is the very one its ARN carries, and whose host states no scheme — it carries none at
//!   all ("you can't specify a scheme for a Gateway Load Balancer").
//!
//! The rows span what the rule generalises over: every family the model's
//! `LoadBalancerTypeEnum` names, both schemes its `LoadBalancerSchemeEnum` names for the
//! families that carry one, names of different shapes, and two Regions — the Region and the
//! family are exactly what the published value is keyed on. Every assertion is made on what a
//! later `DescribeLoadBalancers` reports as well as on what the create answered, and no row
//! asserts a literal id or port: both are assigned, so what is under test is the FORM and
//! that the two reads agree on it.
//!
//! The wire harness (`emulator`, `call_in_region`, `action`, `p`) comes from
//! `provisioning_loop`; nothing here restates it. That a published address really ANSWERS —
//! a client dialling it, completing the handshake and being served — is
//! `alb_https_listener_tls.rs`; what this suite holds is the FORM each family publishes.

use crate::provisioning_loop::{action, call_in_region, emulator, p, Reply};
use smithy_server_runtime::MultiServiceRouter;

/// The two Regions the rows run in: the one every other suite signs for, and a second one,
/// so a host that hard-coded a Region would fail a row.
const REGIONS: [&str; 2] = ["us-west-2", "eu-west-1"];

/// How many hexadecimal digits the id a Gateway Load Balancer publishes has: the width of the
/// id its own ARN carries.
const HEX_ID_DIGITS: usize = 16;

/// One row: the case name, the load balancer's `Name`, the `Type` it is created with, the
/// `Scheme` it names (`None` for a family that carries none), and how many subnets it is
/// placed in.
struct Row {
    case: &'static str,
    name: &'static str,
    lb_type: &'static str,
    scheme: Option<&'static str>,
    subnets: usize,
}

/// Every family against both schemes it can carry, with names of different shapes and
/// lengths — the name is carried into the host verbatim, so a row with hyphens and digits in
/// it is a different input from a plain one.
const ROWS: &[Row] = &[
    Row {
        case: "an internet-facing application load balancer",
        name: "alb-public",
        lb_type: "application",
        scheme: Some("internet-facing"),
        subnets: 2,
    },
    Row {
        case: "an internal application load balancer",
        name: "alb-private",
        lb_type: "application",
        scheme: Some("internal"),
        subnets: 2,
    },
    Row {
        case: "an application load balancer with digits and hyphens in its name",
        name: "alb-7-edge-01",
        lb_type: "application",
        scheme: Some("internet-facing"),
        subnets: 3,
    },
    Row {
        case: "an application load balancer with a one-character name",
        name: "a",
        lb_type: "application",
        scheme: Some("internal"),
        subnets: 2,
    },
    Row {
        case: "an internet-facing network load balancer",
        name: "nlb-public",
        lb_type: "network",
        scheme: Some("internet-facing"),
        subnets: 1,
    },
    Row {
        case: "an internal network load balancer",
        name: "nlb-private",
        lb_type: "network",
        scheme: Some("internal"),
        subnets: 3,
    },
    Row {
        // A Gateway Load Balancer carries no scheme at all, so neither gateway row names
        // one; what they vary is the name and the placement.
        case: "a gateway load balancer in one subnet",
        name: "gwlb-public",
        lb_type: "gateway",
        scheme: None,
        subnets: 1,
    },
    Row {
        case: "a gateway load balancer in two subnets",
        name: "gwlb-private",
        lb_type: "gateway",
        scheme: None,
        subnets: 2,
    },
];

/// What the `DNSName` of a load balancer of `row` in `region` must be.
enum Published {
    /// An address a client on this machine can DIAL: the `host:port` of the socket this
    /// emulator opened for the load balancer.
    Dialable,
    /// A host under Elastic Load Balancing's own zone, split into the text before the id,
    /// the id's own rule, and the text after it.
    Zone {
        /// Everything the host carries before the id, including the trailing hyphen.
        head: String,
        /// Everything it carries after the id.
        tail: String,
        /// How many characters the id has.
        digits: usize,
        /// What the id itself is.
        id: ZoneId,
    },
}

/// The id a host under Elastic Load Balancing's own zone carries.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ZoneId {
    /// The very hexadecimal id the load balancer's own ARN carries.
    ArnHex,
}

impl Published {
    /// The published value for `row` in `region`.
    fn of(row: &Row, region: &str) -> Self {
        match row.lb_type {
            // A load balancer whose listeners carry a client's own traffic answers on a socket
            // of its own, and publishes that socket's address — whatever its scheme.
            "application" | "network" => Self::Dialable,
            // A Gateway Load Balancer's host states the scheme nowhere, and the id beside its
            // name is the very one its ARN carries.
            _ => Self::Zone {
                head: format!("{}-", row.name),
                tail: format!(".elb.{region}.amazonaws.com"),
                digits: HEX_ID_DIGITS,
                id: ZoneId::ArnHex,
            },
        }
    }

    /// The id `host` carries AND the ARN must carry too, or an explanation of how it breaks
    /// this form. `None` for a published address, which carries no id at all — it carries a
    /// port.
    fn id_of(&self, host: &str) -> Result<Option<String>, String> {
        let (head, tail, digits, kind) = match self {
            Self::Dialable => {
                return match host.parse::<std::net::SocketAddr>() {
                    Ok(_) => Ok(None),
                    Err(error) => Err(format!(
                        "expected an address a client can dial, got {host:?} ({error})"
                    )),
                };
            }
            Self::Zone {
                head,
                tail,
                digits,
                id,
            } => (head, tail, *digits, *id),
        };
        let id = host
            .strip_prefix(head)
            .ok_or_else(|| format!("expected the host to start with {head:?}"))?
            .strip_suffix(tail)
            .ok_or_else(|| format!("expected the host to end with {tail:?}"))?;
        if id.len() != digits {
            return Err(format!("expected an id of {digits} characters, got {id:?}"));
        }
        match kind {
            ZoneId::ArnHex => {
                if !id.chars().all(|c| c.is_ascii_hexdigit()) {
                    return Err(format!("expected a hexadecimal id, got {id:?}"));
                }
                Ok(Some(id.to_string()))
            }
        }
    }
}

/// Create the load balancer `row` describes in `region`, and answer what the create replied.
async fn create(router: &MultiServiceRouter, region: &str, row: &Row) -> Reply {
    let mut form = action("CreateLoadBalancer");
    form.push(p("Name", row.name));
    form.push(p("Type", row.lb_type));
    if let Some(scheme) = row.scheme {
        form.push(p("Scheme", scheme));
    }
    if row.lb_type == "gateway" {
        // A Gateway Load Balancer carries IPv4 addresses alone.
        form.push(p("IpAddressType", "ipv4"));
    }
    for index in 1..=row.subnets {
        form.push(p(
            &format!("Subnets.member.{index}"),
            &format!("subnet-{index}{index}{index}{index}{index}{index}{index}{index}"),
        ));
    }
    let reply = call_in_region(router, region, &form).await;
    assert_eq!(
        reply.status, 200,
        "case {:?} in {region}: expected the create to be served, got {} body {}",
        row.case, reply.status, reply.body
    );
    reply
}

/// The value a load balancer publishes as its `DNSName` is the address a client reaches it
/// at: a dialable address for the load balancer whose traffic a client on this machine dials
/// directly, and the host its own zone gives it for the ones whose client sits inside this
/// emulator — in whichever Region it was created, with a later read reporting the same value.
#[tokio::test]
async fn the_published_host_follows_the_form_of_its_family() {
    // Every host seen, so no two load balancers publish the same one.
    let mut seen: Vec<String> = Vec::new();

    for region in REGIONS {
        let (router, _engine) = emulator();
        for row in ROWS {
            let created = create(&router, region, row).await;
            let host = created
                .text("DNSName")
                .unwrap_or_else(|| panic!("case {:?}: no DNSName in {}", row.case, created.body));
            let arn = created.text("LoadBalancerArn").unwrap_or_else(|| {
                panic!("case {:?}: no LoadBalancerArn in {}", row.case, created.body)
            });

            let published = Published::of(row, region);
            let id = published
                .id_of(&host)
                .unwrap_or_else(|why| panic!("case {:?} in {region}: {why}, host {host:?}", row.case));

            // A Gateway Load Balancer publishes the very id its ARN carries; a load balancer
            // that publishes an address of its own carries no id at all — it carries a port.
            let arn_id = arn.rsplit('/').next().unwrap_or_default().to_string();
            if let Some(id) = id {
                assert_eq!(
                    id, arn_id,
                    "case {:?} in {region}: expected the host to carry the ARN's own id, \
                     host {host:?} arn {arn}",
                    row.case
                );
            }

            // The read reports the host the create answered: it is stored, not recomposed.
            let mut form = action("DescribeLoadBalancers");
            form.push(p("LoadBalancerArns.member.1", &arn));
            let read = call_in_region(&router, region, &form).await;
            assert_eq!(
                read.status, 200,
                "case {:?} in {region}: expected 200 from the read, got {} body {}",
                row.case, read.status, read.body
            );
            assert_eq!(
                read.text("DNSName").as_deref(),
                Some(host.as_str()),
                "case {:?} in {region}: a read must report the host the create published, \
                 got body {}",
                row.case,
                read.body
            );

            assert!(
                !seen.contains(&host),
                "case {:?} in {region}: every load balancer publishes its own host, \
                 {host:?} repeated",
                row.case
            );
            seen.push(host);
        }
    }
}
