// SPDX-License-Identifier: CC0-1.0

//! The VPC a function is ATTACHED to, as the run inside it addresses that VPC: the private
//! address its traffic leaves from, and the VPC's own DNS resolver.
//!
//! "To connect a function to a VPC, Lambda creates an elastic network interface for each
//! subnet in your function's VPC configuration" ([`crate::state::vpc_interfaces`] stands those
//! interfaces up in the account's own Amazon EC2 inventory, each holding a private address out
//! of its subnet's range). That attachment is what a run addresses the VPC through, and this
//! module is what the run reads off it:
//!
//! * the SOURCE address a datagram or a connection toward an address of the VPC leaves from —
//!   the private address of the interface the route resolves through, which is the interface in
//!   the destination's own subnet when there is one and otherwise the first the function holds.
//!   An address OUTSIDE the VPC's ranges has no route through the attachment, so it is answered
//!   the way a host with no route to an address answers one;
//! * the VPC's own DNS resolver, which answers at the second address of the VPC's IPv4 range —
//!   "the Amazon DNS server … is located at the base of your VPC network range plus two"
//!   (Amazon VPC User Guide, "Amazon DNS server") — and at the LINK-LOCAL address every VPC's
//!   resolver also answers at ([`LINK_LOCAL_RESOLVER`]), reachable from any execution
//!   environment the VPC holds an interface for. It answers the queries the function sends it
//!   while the VPC's `enableDnsSupport` attribute is on: "If … enableDnsSupport is false, the
//!   Amazon Route 53 Resolver server cannot resolve … DNS hostnames"
//!   (`ModifyVpcAttributeRequest$EnableDnsSupport`). It answers at that port over EITHER
//!   transport — a query carried in a datagram ([`VpcAttachment::dns_reply`]) and a query
//!   carried on a connection the client opened to it ([`VpcAttachment::carry_dns_connection`]),
//!   which is the transport a client falls back to for an answer that will not fit in a
//!   datagram — out of the same zones, because the transport decides the framing and nothing
//!   else.
//!
//! What that resolver answers is the VPC's OWN internal zone — the Region's private DNS domain
//! (`ec2.internal` in `us-east-1`, `<region>.compute.internal` elsewhere), whose
//! Amazon-provided hostnames name an address in the VPC. So a query for such a hostname of an
//! address in this VPC is answered with that address, a reverse query for such an address with
//! that hostname — while the VPC's Resolver configuration still has the AUTODEFINED rules for
//! reverse lookups, which is that configuration's own fact, read out of the register the
//! service owning it records ([`::service_common::vpc_private_dns`]) — a name of the internal
//! zone that names no address in the VPC with `NXDOMAIN`, and a name of another zone — one the
//! resolver recurses for — with what that recursion can establish from this emulator: a name a
//! DNS STANDARD reserves for documentation is answered out of the one reading of those names
//! this tree holds (`service_common::public_dns_standard`), a name the public namespace fixes as
//! absent (`::service_common::vpc_private_dns::established_not_to_exist`) does not exist, so it
//! is `NXDOMAIN`, and every other such name is one nothing here can answer, so it is `SERVFAIL`.
//! The DNS messages themselves are read and written by the `simple-dns` crate, so what travels
//! is a real DNS exchange rather than a shape of this module's own.
//!
//! Before the resolver reads anything for a query, the DNS FIREWALL filtering that VPC decides
//! it: the rule groups that reach the VPC — its own associations, and the Route 53 Profiles
//! associated with it — are the records of the service that OWNS DNS Firewall, so the decision
//! is read out of the register that service records itself in
//! ([`::service_common::vpc_dns_firewall`]) rather than out of a copy of the rules held here. A
//! name a matched `BLOCK` rule covers is answered that rule's `BlockResponse` way instead of
//! out of the zone; an engine no such service is mounted on filters nothing.
//!
//! Past the firewall, the RESOLVER RULES in force in the VPC decide whether this resolver answers
//! the query at all: a query whose name the most specific rule reaching the VPC sends away is
//! forwarded to the resolvers that rule names, so what the client reads is what answered there
//! and not what any zone holds — nothing at all, where nothing answers at the addresses the rule
//! names. Those rules and their targets are the records of the service that owns Resolver rules,
//! so the outcome is read out of the same register ([`::service_common::vpc_private_dns`]).
//!
//! A query the resolver answers is also RECORDED where the account asked for it: the Resolver
//! query logging configurations associated with the VPC are the records of the service that owns
//! them, so the record of the query is handed to that service through the same register
//! ([`::service_common::vpc_private_dns`]) rather than rendered or delivered here.
//!
//! The attachment is read out of the EC2 inventory ONCE, when the invocation's seam is built
//! ([`attachment_of`]), which is the point the execution environment is given its place in the
//! VPC; nothing here writes to that inventory. A function attached to no VPC has no attachment
//! at all, and its datagrams reach only the in-process X-Ray daemon
//! ([`super::xray_daemon`]) — the position every function was in before this module.

use ::std::net::{IpAddr, Ipv4Addr};

use ::ipnet::Ipv4Net;
use ::service_com_amazonaws_ec2_amazonec2::state as ec2;
use ::service_common::vpc_dns_firewall::{
    FirewallDecision, VpcQueryFilters, OVERRIDE_DNS_TYPE_CNAME,
};
use ::service_common::vpc_private_dns::{
    established_not_to_exist, ForwardedQuery, InVpcQuerySource, LoggedAnswer, LoggedQuery,
    PrivateZoneAnswer, PrivateZoneCode, PrivateZoneRecord, QueryAdmission, VpcPrivateDnsPlane,
    CLASS_INTERNET, PROTOCOL_UDP, SOURCE_ID_INSTANCE,
};
use ::simple_dns::rdata::{RData, A, AAAA, CNAME, MX, NS, PTR, TXT};
use ::simple_dns::{
    Name, Packet, PacketFlag, QCLASS, Question, ResourceRecord, CLASS, QTYPE, RCODE, TYPE,
};

/// How far into the VPC's own IPv4 range the Amazon-provided DNS server sits: "the Amazon DNS
/// server … is located at the base of your VPC network range plus two" (Amazon VPC User Guide,
/// "Amazon DNS server").
const RESOLVER_OFFSET: u32 = 2;

/// The port that server answers on, which is the port DNS is served at (`domain`, 53).
pub const RESOLVER_PORT: u16 = 53;

/// The LINK-LOCAL address the Amazon-provided DNS server of every VPC also answers at: the
/// Amazon-provided DNS server is reached either at "the 169.254.169.253 IP address, or the
/// reserved IP address at the base of the VPC network range plus two"
/// (`ModifyVpcAttributeRequest$EnableDnsSupport`'s own account of the two addresses).
///
/// A link-local address is not part of the VPC's own ranges, so the route to it is the
/// attachment's rather than one of a range it holds: an execution environment with an
/// interface in the VPC reaches it, one attached to no VPC does not.
pub const LINK_LOCAL_RESOLVER: Ipv4Addr = Ipv4Addr::new(169, 254, 169, 253);

/// How long an answer of that resolver may be held: the TTL every record below carries.
///
/// CHOSEN rather than taken from a published figure: AWS documents no TTL for the
/// Amazon-provided hostnames of a VPC. One minute is short enough that a client holds no
/// answer of this emulator past the resource it names.
const ANSWER_TTL_SECONDS: u32 = 60;

/// The prefix an Amazon-provided private hostname of an IPv4 address carries: "ip-" followed
/// by the address with its dots replaced by dashes (`ip-10-0-1-5.<region>.compute.internal`),
/// which is the `ip-name` hostname EC2 gives a private address
/// (`ec2::private_dns_name`).
const IP_NAME_PREFIX: &str = "ip-";

/// The id a lookup made THROUGH this module carries ([`VpcAttachment::addresses_of`]): one
/// query is built, handed to the resolver and read back before the next is built, so the id
/// matches no other exchange and is the same for every one of them.
const LOOKUP_QUERY_ID: u16 = 1;

/// The zone a reverse lookup of an IPv4 address is asked in (RFC 1035's `IN-ADDR.ARPA`
/// domain).
const REVERSE_ZONE: &str = "in-addr.arpa";

/// How many questions of ONE message this resolver answers.
///
/// A client asks one question per message — "the question section … usually contains exactly
/// one entry" (RFC 1035 §4.1.2) — and a datagram is bounded only by how much it can carry, so
/// this is the bound on how much work one datagram can ask for: the number of questions it may
/// multiply the firewall's decisions and the zone's lookups by. A message past it is dropped,
/// as one that carries no readable DNS message is. Well above the one question a client asks.
const MAX_QUESTIONS_PER_MESSAGE: usize = 16;

/// The response codes this resolver answers with, as RFC 1035 §4.1.1 numbers them: no error,
/// a server failure (what a resolver answers for a question it cannot have answered), and a
/// name that does not exist.
const RESPONSE_NO_ERROR: u8 = 0;
/// See [`RESPONSE_NO_ERROR`].
const RESPONSE_SERVER_FAILURE: u8 = 2;
/// See [`RESPONSE_NO_ERROR`].
const RESPONSE_NAME_ERROR: u8 = 3;

/// What the DNS of a VPC answered one name with ([`VpcAttachment::answered_name`]): the names
/// the reply chained the question on to, and the addresses it carried.
#[derive(Debug, Clone, Default)]
pub struct AnsweredName {
    /// The names the reply chained the question on to, in the order it chained them — empty
    /// for a name it answered without an alias.
    pub chain: Vec<String>,
    /// The addresses the reply carried, IPv4 first, as a resolver that prefers them answers.
    pub addresses: Vec<IpAddr>,
}

/// The reply to ONE question, read apart ([`VpcAttachment::answered_of_type`]).
#[derive(Debug, Clone, Default)]
struct AnsweredRecords {
    /// The addresses the answer section carried.
    addresses: Vec<IpAddr>,
    /// Its alias records: the name each was held under, and the name it stands for.
    aliases: Vec<(String, String)>,
}

/// The names a reply chains a question about `name` on to, walked off its alias records: each
/// alias stands for a name, and the name it stands for may itself stand at one.
///
/// Bounded by the records the reply carried, so a chain that comes back to a name it already
/// passed through — which is what a zone holding a loop of aliases answers — ends there instead
/// of being walked for ever.
fn chained_names(name: &str, aliases: &[(String, String)]) -> Vec<String> {
    let mut walked: Vec<String> = Vec::new();
    let mut current = normalized_name(name);
    while let Some((_, stands_for)) = aliases
        .iter()
        .find(|(held, _)| normalized_name(held) == current)
    {
        let next = normalized_name(stands_for);
        if next == current || walked.contains(&next) {
            break;
        }
        walked.push(next.clone());
        current = next;
        if walked.len() >= aliases.len() {
            break;
        }
    }
    walked
}

/// `name` as names are compared: without the trailing dot that spells the root explicitly, and
/// case-folded — a name is compared without regard to case (RFC 4343).
///
/// The one folding this module reads a name through, which the name service above it reads a
/// host through as well ([`super::guest_resolver`], after taking off the brackets an address
/// literal may be written in) — so what counts as the same name cannot drift between the two.
pub(super) fn normalized_name(name: &str) -> String {
    let named = name.trim();
    named
        .strip_suffix('.')
        .unwrap_or(named)
        .to_ascii_lowercase()
}

/// ONE interface of the attachment: the private address EC2 handed it, and the range of the
/// subnet it sits in.
#[derive(Debug, Clone, Copy)]
struct AttachedInterface {
    /// `NetworkInterface$PrivateIpAddress` of the interface.
    address: Ipv4Addr,
    /// `Subnet$CidrBlock` of the subnet it was placed in, which is what decides whether a
    /// destination is reached through this interface rather than another.
    subnet: Ipv4Net,
}

/// The VPC attachment one invocation runs with: where its traffic leaves from, and which
/// addresses it reaches.
#[derive(Debug, Clone)]
pub struct VpcAttachment {
    /// The interfaces the function holds, in the order of the subnets they sit in.
    interfaces: Vec<AttachedInterface>,
    /// Every IPv4 range of the VPC those interfaces sit in — the address space a route
    /// through them reaches.
    ranges: Vec<Ipv4Net>,
    /// Whether the VPC's `enableDnsSupport` attribute is on, which decides whether its DNS
    /// server answers at all.
    dns_support: bool,
    /// The Region the function runs in, whose private DNS domain is the suffix of every
    /// Amazon-provided hostname the resolver answers for.
    region: String,
    /// The VPC those interfaces sit in, which is what the DNS Firewall filtering the run's
    /// queries is keyed by.
    vpc_id: String,
    /// Where a query's firewall decision is read from, and `None` for an engine that vends no
    /// such register — then nothing filters the run's queries.
    filters: Option<VpcQueryFilters>,
    /// The security groups the interfaces sit behind, by id — the member list a security-group
    /// rule on the interface a query is ADDRESSED to is matched against, which is what decides
    /// whether a query to a Resolver endpoint of the VPC is admitted.
    groups: Vec<String>,
    /// Where the Resolver data plane of this VPC is read from — which addresses of it answer
    /// DNS besides the VPC's own resolver, and what the private hosted zones associated with it
    /// hold — and `None` for an engine that vends no such register, which then has no Resolver
    /// endpoint and no private zone anywhere.
    plane: Option<VpcPrivateDnsPlane>,
    /// The id of the elastic network interface the run's traffic leaves the VPC through, which
    /// is the resource inside the VPC a query of this run came FROM — the one a record of that
    /// query attributes it to ([`VpcAttachment::recorded`]).
    source_interface_id: Option<String>,
}

/// The attachment of the function whose interfaces are `interface_ids`, read out of the
/// account's own EC2 inventory.
///
/// `None` for a function attached to no VPC — one whose configuration named no subnet, so
/// Lambda stood no interface up for it — and for one whose interfaces that inventory no longer
/// holds, which is a function with nothing to reach a VPC through either way. Read-only: the
/// inventory is the EC2 service's, and this only reads the records that service wrote.
#[must_use]
pub fn attachment_of<Eng: Send + Sync + 'static>(
    engine: &Eng,
    region: &str,
    interface_ids: &[String],
) -> Option<VpcAttachment> {
    if interface_ids.is_empty() {
        return None;
    }
    let state = ec2::service_state(engine)?;
    let store = state.lock();
    let mut interfaces: Vec<AttachedInterface> = Vec::with_capacity(interface_ids.len());
    let mut held_vpc: Option<String> = None;
    let mut groups: Vec<String> = Vec::new();
    // The interface the function's traffic LEAVES from, which is the first one it holds
    // ([`VpcAttachment::private_address`]): the resource inside the VPC a query of this run came
    // from.
    let mut source_interface_id: Option<String> = None;
    for interface_id in interface_ids {
        let Some(interface) = store.network_interfaces.get(interface_id.as_str()) else {
            continue;
        };
        let address = interface
            .private_ip_address
            .as_deref()
            .and_then(|address| address.parse::<Ipv4Addr>().ok());
        let subnet = interface
            .subnet_id
            .as_deref()
            .and_then(|subnet_id| store.subnets.get(subnet_id))
            .and_then(|subnet| subnet.cidr_block.as_deref())
            .and_then(|cidr| cidr.parse::<Ipv4Net>().ok());
        if let (Some(address), Some(subnet)) = (address, subnet) {
            interfaces.push(AttachedInterface { address, subnet });
            if source_interface_id.is_none() {
                source_interface_id = Some(interface_id.clone());
            }
            if held_vpc.is_none() {
                held_vpc = interface.vpc_id.clone();
            }
            // The groups on the interface, which is what the function is a MEMBER of inside the
            // VPC: a rule on another interface whose source is a security group admits this
            // traffic only through one of them.
            for group in interface.groups.iter().flatten() {
                if let Some(group_id) = group.group_id.clone() {
                    if !groups.contains(&group_id) {
                        groups.push(group_id);
                    }
                }
            }
        }
    }
    let vpc_id = held_vpc?;
    if interfaces.is_empty() {
        return None;
    }
    // The VPC's own ranges: the block it was created with and every block later associated
    // with it, as EC2's record of the VPC reports them.
    let vpc = store.vpcs.get(vpc_id.as_str())?;
    let mut ranges: Vec<Ipv4Net> = vpc
        .cidr_block
        .as_deref()
        .into_iter()
        .chain(
            vpc.cidr_block_association_set
                .iter()
                .flatten()
                .filter_map(|association| association.cidr_block.as_deref()),
        )
        .filter_map(|cidr| cidr.parse::<Ipv4Net>().ok())
        .collect();
    ranges.sort_by_key(|range| (u32::from(range.network()), range.prefix_len()));
    ranges.dedup();
    if ranges.is_empty() {
        return None;
    }
    let dns_support = store
        .vpc_attributes
        .get(vpc_id.as_str())
        .is_some_and(|attributes| attributes.enable_dns_support);
    // The EC2 inventory is done with: nothing below reads it.
    drop(store);
    Some(VpcAttachment {
        interfaces,
        ranges,
        dns_support,
        region: region.to_string(),
        filters: VpcQueryFilters::of(engine),
        groups,
        plane: VpcPrivateDnsPlane::of(engine),
        vpc_id,
        source_interface_id,
    })
}

impl VpcAttachment {
    /// The private address a datagram or a connection toward `destination` leaves from, and
    /// `None` for a destination the attachment has no route to.
    ///
    /// The route resolves through the interface in the destination's OWN subnet when the
    /// function holds one there, and otherwise through the first interface it holds — which is
    /// how an address elsewhere in the VPC is reached, through the VPC's own router, and how
    /// the link-local address of the VPC's resolver is reached, which belongs to no subnet.
    #[must_use]
    pub fn source_toward(&self, destination: Ipv4Addr) -> Option<Ipv4Addr> {
        if !self.holds(destination) && destination != LINK_LOCAL_RESOLVER {
            return None;
        }
        self.interfaces
            .iter()
            .find(|interface| interface.subnet.contains(&destination))
            .or_else(|| self.interfaces.first())
            .map(|interface| interface.address)
    }

    /// The private address the function's traffic carries INSIDE the VPC it is attached to:
    /// the address of the first interface Lambda stood up for it, which is the one a
    /// destination elsewhere in the VPC is reached through.
    ///
    /// What a plane of that VPC states its client as ([`super::guest_socket`]), where the
    /// transport that carried the connection has an address of its own.
    #[must_use]
    pub fn private_address(&self) -> Option<Ipv4Addr> {
        self.interfaces.first().map(|interface| interface.address)
    }

    /// Whether `destination` is an address of the VPC this function is attached to.
    #[must_use]
    pub fn holds(&self, destination: Ipv4Addr) -> bool {
        self.ranges.iter().any(|range| range.contains(&destination))
    }

    /// The address of the VPC's own DNS server: the second address of its first IPv4 range.
    #[must_use]
    pub fn resolver(&self) -> Option<Ipv4Addr> {
        let range = self.ranges.first()?;
        let address = Ipv4Addr::from(u32::from(range.network()).checked_add(RESOLVER_OFFSET)?);
        range.contains(&address).then_some(address)
    }

    /// Whether `destination`:`port` is where that server answers: either address it is reached
    /// at, on the DNS port.
    #[must_use]
    pub fn is_resolver(&self, destination: Ipv4Addr, port: u16) -> bool {
        port == RESOLVER_PORT
            && (destination == LINK_LOCAL_RESOLVER || self.resolver() == Some(destination))
    }

    /// The reply the VPC's DNS server answers `query` with, or `None` when it answers nothing
    /// at all: a VPC whose `enableDnsSupport` is off has no resolver to reply with, a datagram
    /// that carries no readable DNS message is one the server drops, and so is one asking more
    /// questions than [`MAX_QUESTIONS_PER_MESSAGE`].
    #[must_use]
    pub fn resolved(&self, query: &[u8]) -> Option<Vec<u8>> {
        if !self.dns_support {
            return None;
        }
        let asked = Packet::parse(query).ok()?;
        if asked.questions.len() > MAX_QUESTIONS_PER_MESSAGE {
            return None;
        }
        let mut reply = Packet::new_reply(asked.id());
        if asked.has_flags(PacketFlag::RECURSION_DESIRED) {
            reply.set_flags(PacketFlag::RECURSION_DESIRED);
        }
        // The server is a resolver: it answers for the zone it holds and recurses for the
        // rest.
        reply.set_flags(PacketFlag::RECURSION_AVAILABLE);
        // What the firewall decides about this message's questions, read ONCE for the whole
        // message however many questions it carries.
        let mut filtered = self.filtered(&asked.questions);
        let mut code = RESPONSE_NO_ERROR;
        // What each question came to, in the order asked, which is what the query logging
        // configurations of this VPC record for it below.
        let mut answered: Vec<Answered> = Vec::with_capacity(asked.questions.len());
        for question in &asked.questions {
            reply.questions.push(question.clone());
            let came_to = self.answered(question, filtered.next().flatten());
            if came_to.authoritative {
                reply.set_flags(PacketFlag::AUTHORITATIVE_ANSWER);
            }
            if came_to.code != RESPONSE_NO_ERROR {
                code = came_to.code;
            }
            for record in &came_to.records {
                reply.answers.push(record.clone());
            }
            answered.push(came_to);
        }
        let mut bytes = reply.build_bytes_vec().ok()?;
        // The RESPONSE CODE is the low four bits of the second flag byte of the message
        // header (RFC 1035 §4.1.1). The crate writes every other field of the reply and reads
        // this one back (`Packet::rcode`), but exposes no setter for it, so it is written here
        // over the header the crate itself just built.
        let flags = bytes.get_mut(3)?;
        *flags = (*flags & 0xF0) | (code & 0x0F);
        // The queries this VPC's Resolver answered are recorded where the account asked for
        // them, which is the owning service's own write ([`Self::recorded`]).
        self.recorded(&asked.questions, &answered, code);
        Some(bytes)
    }

    /// What ONE question of a message came to, once the firewall has had its say about it.
    ///
    /// `filtered` is that say: `None` for a query it lets through — one no rule matches, and one
    /// a matched `ALLOW` or `ALERT` rule permits — and otherwise the answer the matched `BLOCK`
    /// rule's `BlockResponse` states, which is what the query comes to whatever any zone holds
    /// for the name.
    fn answered(&self, question: &Question<'_>, filtered: Option<Filtered>) -> Answered {
        // The firewall decides a query before the resolver reads anything for it, so a blocked
        // name is answered the rule's way whatever the zone holds. A block is the resolver's own
        // answer rather than a zone's, so it is not authoritative.
        match filtered {
            Some(Filtered::NxDomain) => {
                return Answered::of(Vec::new(), RESPONSE_NAME_ERROR, false)
            }
            Some(Filtered::NoData) => return Answered::of(Vec::new(), RESPONSE_NO_ERROR, false),
            Some(Filtered::Record(record)) => {
                return Answered::of(vec![*record], RESPONSE_NO_ERROR, false)
            }
            None => {}
        }
        // The RESOLVER RULES in force in this VPC decide the query before any zone is read for
        // it: a query whose name the most specific rule reaching the VPC sends away goes to the
        // resolvers that rule names, so nothing here answers it out of a zone. Which rules those
        // are, which one decides the query and what answered at its targets are Route 53
        // Resolver's own records, read out of the register that owns them
        // ([`::service_common::vpc_private_dns`]).
        match self.forwarded(question) {
            // The query left the VPC and nothing answered: a resolver whose forwarders leave a
            // query unanswered has no answer of its own for it, and the answer is not this
            // resolver's authority's.
            Some(ForwardedQuery::Unanswered) => {
                return Answered::of(Vec::new(), RESPONSE_SERVER_FAILURE, false)
            }
            Some(ForwardedQuery::Answered(answered)) => {
                let code = if answered.code == PrivateZoneCode::NxDomain {
                    RESPONSE_NAME_ERROR
                } else {
                    RESPONSE_NO_ERROR
                };
                // Another resolver's answer, relayed: this one holds no authority for it.
                return Answered::of(private_zone_records(&answered), code, false);
            }
            None => {}
        }
        // The PRIVATE HOSTED ZONES associated with this VPC answer ahead of the VPC's own
        // internal zone: a query asked inside a VPC is answered out of the private zones
        // associated with it, authoritatively, and only a name NO such zone holds is left to
        // the zones below. Which zones those are, and what they hold, are Route 53's and Route 53
        // Resolver's records, read out of the register that owns them
        // ([`::service_common::vpc_private_dns`]).
        if let Some(answered) = self.private_zone_answer(question) {
            let code = if answered.code == PrivateZoneCode::NxDomain {
                RESPONSE_NAME_ERROR
            } else {
                RESPONSE_NO_ERROR
            };
            // A zone of the VPC holds the name, so its answer is authoritative.
            return Answered::of(private_zone_records(&answered), code, true);
        }
        // A name no zone of the account holds is one the resolver recurses for, and the only
        // names it can be answered out here are those a DNS standard fixes the records of
        // (`service_common::public_dns_standard`): a recursed answer is not this resolver's own,
        // so it is not authoritative.
        if let Some(records) = standard_public_records(question) {
            return Answered::of(records, RESPONSE_NO_ERROR, false);
        }
        match self.answer_for(&question.qname, question.qtype) {
            // The zone is the VPC's own, so the answer is authoritative.
            Answer::Found(record) => Answered::of(vec![*record], RESPONSE_NO_ERROR, true),
            Answer::Empty => Answered::of(Vec::new(), RESPONSE_NO_ERROR, true),
            Answer::Absent => Answered::of(Vec::new(), RESPONSE_NAME_ERROR, true),
            // The name is under no zone of this resolver, so the answer is not its own
            // authority's: what establishes the name does not exist is the recursion.
            Answer::Nonexistent => Answered::of(Vec::new(), RESPONSE_NAME_ERROR, false),
            Answer::Elsewhere => Answered::of(Vec::new(), RESPONSE_SERVER_FAILURE, false),
        }
    }

    /// Record every question of one message in the Resolver query logging configurations
    /// associated with this VPC: the question, the response code the client was answered with,
    /// and the records of the answer section.
    ///
    /// WHICH configurations those are, what they deliver into and in which form is the owning
    /// service's own ([`::service_common::vpc_private_dns`]), so nothing of a query log is
    /// rendered here: what this states is the QUERY, which is what only the component carrying
    /// it knows — the name asked for, the class and type, what came back, and the address inside
    /// the VPC it came from.
    fn recorded(&self, questions: &[Question<'_>], answered: &[Answered], code: u8) {
        let Some(plane) = self.plane.as_ref() else {
            return;
        };
        let Some(source_ip) = self.private_address().map(|address| address.to_string()) else {
            return;
        };
        // WHICH resource inside the VPC the query came from, as a record of it attributes one: a
        // record states the source under its `instance` member, and the resource this run's
        // queries leave the VPC through is the elastic network interface Lambda stood up for the
        // function — the one the account's EC2 inventory holds the source address on, which is
        // what a record of a query from a client that is no instance names there.
        let source_ids: Vec<(&str, &str)> = self
            .source_interface_id
            .as_deref()
            .map(|interface_id| (SOURCE_ID_INSTANCE, interface_id))
            .into_iter()
            .collect();
        for (question, came_to) in questions.iter().zip(answered) {
            let answers: Vec<LoggedAnswer> =
                came_to.records.iter().filter_map(logged_answer).collect();
            plane.log_query(
                &InVpcQuerySource {
                    vpc_id: &self.vpc_id,
                    source_ip: &source_ip,
                    security_group_ids: &self.groups,
                },
                &LoggedQuery {
                    name: &question.qname.to_string(),
                    record_type: type_keyword(question.qtype).unwrap_or_default(),
                    class: CLASS_INTERNET,
                    response_code: response_code_keyword(code),
                    answers: &answers,
                    // A datagram is a query carried over UDP, which is the transport a record of
                    // it states.
                    transport: PROTOCOL_UDP,
                    source_ids: &source_ids,
                },
            );
        }
    }

    /// What the DNS Firewall of this VPC answers each of `questions` with, in the order they
    /// were asked: `None` for a question it lets through — one no rule matches, and one a
    /// matched `ALLOW` or `ALERT` rule permits.
    ///
    /// The rules are read ONCE for the whole message ([`VpcQueryFilters::decisions`]), so a
    /// message of several questions costs one reading of them and not one per question.
    fn filtered(&self, questions: &[Question<'_>]) -> ::std::vec::IntoIter<Option<Filtered>> {
        let Some(filters) = self.filters.as_ref() else {
            return Vec::new().into_iter();
        };
        let asked: Vec<(String, Option<u16>)> = questions
            .iter()
            .map(|question| {
                (
                    question.qname.to_string(),
                    Some(u16::from(question.qtype)),
                )
            })
            .collect();
        filters
            .decisions(&self.vpc_id, &asked)
            .into_iter()
            .zip(questions)
            .map(|(decided, question)| {
                decided.and_then(|decision| filtered_answer(&question.qname, decision))
            })
            .collect::<Vec<Option<Filtered>>>()
            .into_iter()
    }

    /// The addresses the VPC's own DNS server answers for `name`, ASKED OF IT the way a client
    /// asks: one question per address family, each carried to [`Self::resolved`] as a real DNS
    /// message and read back off the reply that server answered with.
    ///
    /// The IPv4 addresses come first, as a resolver that prefers them answers.
    ///
    /// Empty for a name it holds no address under — a name of its own zone that names no
    /// address in the VPC, a name of another zone it would have to recurse for, a name the
    /// VPC's DNS Firewall blocks, and every name at all while the VPC's `enableDnsSupport` is
    /// off, which leaves it with no server to answer. So a name service that asks this
    /// ([`super::guest_resolver`]) is answered by the same server, and with the same records, as
    /// a handler that sends the query itself.
    #[must_use]
    pub fn addresses_of(&self, name: &str) -> Vec<IpAddr> {
        let mut resolved: Vec<IpAddr> = Vec::new();
        for kind in [TYPE::A, TYPE::AAAA] {
            resolved.extend(self.answered_addresses(name, kind));
        }
        resolved
    }

    /// What the VPC's own DNS server answers for `name`, ASKED OF IT the way a client asks:
    /// the names the reply CHAINS the question on to, in the order it chained them, and the
    /// addresses it carried.
    ///
    /// A resolver answers a question about a name an alias stands at with the alias record and
    /// the records of the name it stands for, so what a client learns is the name the chain ENDS
    /// at beside the addresses — the canonical name a lookup asking for the whole answer reports
    /// ([`super::guest_resolver`]). A chain whose last name the reply carried no address for is
    /// left for the caller to go on asking about, which is what a stub resolver does with one.
    ///
    /// Empty on both counts for a name the server holds nothing under; see [`Self::addresses_of`],
    /// which is this answer's addresses alone.
    #[must_use]
    pub fn answered_name(&self, name: &str) -> AnsweredName {
        let mut addresses: Vec<IpAddr> = Vec::new();
        let mut aliases: Vec<(String, String)> = Vec::new();
        for kind in [TYPE::A, TYPE::AAAA] {
            let answered = self.answered_of_type(name, kind);
            addresses.extend(answered.addresses);
            for alias in answered.aliases {
                if !aliases.contains(&alias) {
                    aliases.push(alias);
                }
            }
        }
        AnsweredName {
            chain: chained_names(name, &aliases),
            addresses,
        }
    }

    /// The addresses of ONE family the server answers for `name`: the records of the reply to a
    /// question of that type.
    fn answered_addresses(&self, name: &str, kind: TYPE) -> Vec<IpAddr> {
        self.answered_of_type(name, kind).addresses
    }

    /// The reply to a question of ONE type, read apart: the addresses it carried and the alias
    /// records it answered with, each as the name it was held under and the name it stands for.
    fn answered_of_type(&self, name: &str, kind: TYPE) -> AnsweredRecords {
        let Ok(asked) = Name::new(name) else {
            return AnsweredRecords::default();
        };
        let mut query = Packet::new_query(LOOKUP_QUERY_ID);
        query.set_flags(PacketFlag::RECURSION_DESIRED);
        query.questions.push(Question::new(
            asked.into_owned(),
            QTYPE::TYPE(kind),
            QCLASS::CLASS(CLASS::IN),
            false,
        ));
        let Ok(bytes) = query.build_bytes_vec() else {
            return AnsweredRecords::default();
        };
        let Some(reply) = self.resolved(&bytes) else {
            return AnsweredRecords::default();
        };
        let Ok(answered) = Packet::parse(&reply) else {
            return AnsweredRecords::default();
        };
        if answered.rcode() != RCODE::NoError {
            return AnsweredRecords::default();
        }
        let mut read = AnsweredRecords::default();
        for record in &answered.answers {
            match &record.rdata {
                RData::A(A { address }) => {
                    read.addresses.push(IpAddr::V4(Ipv4Addr::from(*address)));
                }
                RData::AAAA(AAAA { address }) => read
                    .addresses
                    .push(IpAddr::V6(::std::net::Ipv6Addr::from(*address))),
                RData::CNAME(CNAME(stands_for)) => read
                    .aliases
                    .push((record.name.to_string(), stands_for.to_string())),
                _other => {}
            }
        }
        read
    }

    /// What the Resolver rules in force in this VPC do with one question, and `None` for a
    /// question no rule of them sends away — as well as for a question whose TYPE this resolver
    /// cannot name, which is one it answers nothing for either way.
    fn forwarded(&self, question: &Question<'_>) -> Option<ForwardedQuery> {
        let plane = self.plane.as_ref()?;
        let record_type = type_keyword(question.qtype)?;
        plane.forwarded(&self.vpc_id, &question.qname.to_string(), record_type)
    }

    /// What the PRIVATE HOSTED ZONES associated with this VPC hold for one question, and `None`
    /// when no zone of it holds the name — a question those zones do not answer at all — as well
    /// as for a question whose TYPE this resolver cannot name.
    fn private_zone_answer(&self, question: &Question<'_>) -> Option<PrivateZoneAnswer> {
        let plane = self.plane.as_ref()?;
        let record_type = type_keyword(question.qtype)?;
        plane.zone_answer(&self.vpc_id, &question.qname.to_string(), record_type)
    }

    /// Whether DNS is answered at `destination`:`port` for a query this attachment carries over
    /// `protocol`, and whether such a query is admitted to the server that answers there.
    ///
    /// Two kinds of address answer DNS in a VPC and they are gated differently. The VPC's own
    /// resolver ([`Self::is_resolver`]) is behind no interface of the account and answers every
    /// client of the VPC. An INBOUND Resolver endpoint adds addresses of its own, on elastic
    /// network interfaces behind the endpoint's security groups: a query reaching one is
    /// delivered only where a rule of those groups admits its protocol and port from this
    /// attachment's own address, and one no rule admits is DROPPED rather than refused — which
    /// is what revoking such a rule looks like to the client. Which addresses those are and what
    /// they admit is the Resolver service's own decision, read out of the register that owns it
    /// ([`::service_common::vpc_private_dns`]).
    #[must_use]
    pub fn dns_admission(
        &self,
        destination: Ipv4Addr,
        port: u16,
        protocol: &str,
    ) -> QueryAdmission {
        let source_ip = self.private_address().map(|address| address.to_string());
        let admission = match (self.plane.as_ref(), source_ip.as_deref()) {
            (Some(plane), Some(source_ip)) => plane.admission(
                &InVpcQuerySource {
                    vpc_id: &self.vpc_id,
                    source_ip,
                    security_group_ids: &self.groups,
                },
                destination,
                port,
                protocol,
            ),
            _unregistered => QueryAdmission::NoServer,
        };
        match admission {
            // No Resolver endpoint answers there — which leaves the VPC's own resolver, at the
            // two addresses it is reached at, which stands behind no interface and so admits
            // every client of the VPC whichever protocol carries the query.
            QueryAdmission::NoServer if self.is_resolver(destination, port) => {
                QueryAdmission::Admitted
            }
            decided => decided,
        }
    }

    /// The reply a DATAGRAM addressed to `destination`:`port` from inside this VPC is answered
    /// with, and `None` when nothing comes back at all — an address nothing of the VPC answers
    /// DNS at, and one a rule of the addressed interface does not admit this query to, whose
    /// datagram is dropped.
    #[must_use]
    pub fn dns_reply(&self, destination: Ipv4Addr, port: u16, query: &[u8]) -> Option<Vec<u8>> {
        // A datagram is a query carried over UDP, which is the protocol a rule on the addressed
        // interface has to admit for it to be delivered.
        match self.dns_admission(destination, port, PROTOCOL_UDP) {
            QueryAdmission::Admitted => self.resolved(query),
            QueryAdmission::Dropped | QueryAdmission::NoServer => None,
        }
    }

    /// Answer DNS on `wire` for as long as the client holds it open: the queries a client of
    /// this VPC carries on a CONNECTION it opened to an address the VPC's DNS is served at.
    ///
    /// A resolver answers at the same port over either transport, and a client uses the
    /// connection when the answer it wants will not fit in a datagram, when it is asking for a
    /// zone transfer, or simply because its own stub resolver was told to: "Messages sent over
    /// TCP connections use server port 53 (decimal). The message is prefixed with a two byte
    /// length field which gives the message length" (RFC 1035 §4.2.2), one message after
    /// another on the one connection. What each message is answered with is the same reading of
    /// the same zones a datagram's is ([`Self::resolved`]) — the transport decides the framing
    /// and nothing else.
    ///
    /// The connection is carried until the client closes it, which is the end of file a read
    /// here reports; a message the server cannot answer at all ends it the same way, because a
    /// connection carries no way to say nothing. Every bound on it belongs to the caller: how
    /// many bytes the run may carry and how long any one of its calls may wait
    /// ([`super::guest_socket`]), plus the two-byte prefix, which cannot frame a message longer
    /// than it can count.
    pub async fn carry_dns_connection<S>(&self, wire: &mut S)
    where
        S: ::tokio::io::AsyncRead + ::tokio::io::AsyncWrite + Unpin + ?Sized,
    {
        use ::tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
        loop {
            let mut prefix = [0_u8; 2];
            if wire.read_exact(&mut prefix).await.is_err() {
                return;
            }
            let mut query = vec![0_u8; usize::from(u16::from_be_bytes(prefix))];
            if !query.is_empty() && wire.read_exact(&mut query).await.is_err() {
                return;
            }
            let Some(reply) = self.resolved(&query) else {
                return;
            };
            let Ok(length) = u16::try_from(reply.len()) else {
                return;
            };
            if wire.write_all(&length.to_be_bytes()).await.is_err()
                || wire.write_all(&reply).await.is_err()
                || wire.flush().await.is_err()
            {
                return;
            }
        }
    }

    /// What the resolver holds for ONE question.
    fn answer_for(&self, asked: &Name<'_>, kind: QTYPE) -> Answer {
        let name = normalized_name(&asked.to_string());
        let internal = ec2::private_dns_domain(&self.region).to_ascii_lowercase();
        if let Some(labels) = name.strip_suffix(&format!(".{REVERSE_ZONE}")) {
            return self.reverse_answer(asked, labels, kind, &internal);
        }
        let Some(host) = name.strip_suffix(&format!(".{internal}")) else {
            // A name of another zone, which the resolver recurses for. What a recursion comes
            // to is the public namespace, and the only part of it this emulator can read is
            // the part a STANDARD fixes: a name the public namespace establishes does not
            // exist is answered as one that does not
            // ([`::service_common::vpc_private_dns::established_not_to_exist`], the one reading
            // of that question every in-VPC resolver here takes), and every other name is one
            // this emulator cannot establish anything about.
            return if established_not_to_exist(&name) {
                Answer::Nonexistent
            } else {
                Answer::Elsewhere
            };
        };
        let Some(address) = address_in_name(host).filter(|address| self.holds(*address)) else {
            return Answer::Absent;
        };
        if kind != QTYPE::TYPE(TYPE::A) {
            // The name exists, so a question about a record type the internal zone holds none
            // of is answered empty rather than as a name that is not there.
            return Answer::Empty;
        }
        Answer::Found(Box::new(ResourceRecord::new(
            asked.clone().into_owned(),
            CLASS::IN,
            ANSWER_TTL_SECONDS,
            RData::A(A {
                address: u32::from(address),
            }),
        )))
    }

    /// What the resolver holds for a REVERSE question: `labels` is what was asked under the
    /// `in-addr.arpa` zone, in that zone's reversed order.
    ///
    /// The Amazon-provided hostname of an address of the VPC is answered by the AUTODEFINED rules
    /// Resolver creates for reverse DNS lookups — "Resolver … create autodefined rules for
    /// reverse DNS lookups" (`UpdateResolverConfigRequest$AutodefinedReverseFlag`) — so a VPC
    /// whose Resolver configuration has them DISABLED has nothing to answer such a question
    /// from, and the reverse name of a private address is answered as the name that does not
    /// exist it is in the locally served reverse zones of private address space (RFC 6303 §4).
    /// Whether they are enabled is that configuration's own fact, read out of the register the
    /// service owning it records ([`::service_common::vpc_private_dns`]).
    fn reverse_answer(
        &self,
        asked: &Name<'_>,
        labels: &str,
        kind: QTYPE,
        internal: &str,
    ) -> Answer {
        let Some(address) = reversed_address(labels).filter(|address| self.holds(*address)) else {
            return Answer::Absent;
        };
        if !self.answers_autodefined_reverse() {
            return Answer::Absent;
        }
        if kind != QTYPE::TYPE(TYPE::PTR) {
            return Answer::Empty;
        }
        let hostname = ec2::private_dns_name(&address.to_string(), &self.region);
        // Built from the Region's own private domain, so a hostname that does not end in it is
        // not one this zone answers.
        if !hostname.to_ascii_lowercase().ends_with(internal) {
            return Answer::Absent;
        }
        match Name::new(&hostname) {
            Ok(answered) => Answer::Found(Box::new(ResourceRecord::new(
                asked.clone().into_owned(),
                CLASS::IN,
                ANSWER_TTL_SECONDS,
                RData::PTR(PTR(answered.into_owned())),
            ))),
            Err(_unnamed) => Answer::Absent,
        }
    }

    /// Whether the autodefined rules for reverse DNS lookups are in force in this VPC, read off
    /// the register at the instant the query is answered — so a configuration the account has
    /// just changed is observed by the next query.
    ///
    /// `true` where no such register is on this engine: nothing then says the rules a VPC has by
    /// default ("This is enabled by default", `ResolverConfig$AutodefinedReverse`) are off.
    fn answers_autodefined_reverse(&self) -> bool {
        self.plane
            .as_ref()
            .is_none_or(|plane| plane.answers_autodefined_reverse(&self.vpc_id))
    }
}

/// What ONE question of a message came to: the records it is answered with, the response code
/// the client reads for it, and whether that answer is an authority's own.
struct Answered {
    /// The answer section for this question, in the order a resolver puts the records.
    records: Vec<ResourceRecord<'static>>,
    /// The response code, as RFC 1035 §4.1.1 numbers one.
    code: u8,
    /// Whether the answer came out of a zone this resolver is the authority for — a private
    /// hosted zone of the VPC, or the VPC's own internal zone — rather than from a firewall rule
    /// or from recursion.
    authoritative: bool,
}

impl Answered {
    /// One answer, stated in the order [`Answered`]'s own fields are.
    fn of(records: Vec<ResourceRecord<'static>>, code: u8, authoritative: bool) -> Self {
        Self {
            records,
            code,
            authoritative,
        }
    }
}

/// The response code `code` as a query log names one, by the mnemonics RFC 1035 §4.1.1 gives the
/// numbers this resolver answers with.
fn response_code_keyword(code: u8) -> &'static str {
    match code {
        RESPONSE_NAME_ERROR => "NXDOMAIN",
        RESPONSE_SERVER_FAILURE => "SERVFAIL",
        _no_error => "NOERROR",
    }
}

/// The records a question for a name whose addresses a DNS STANDARD fixes is answered with, and
/// `None` for a name no standard fixes — which is a name this resolver cannot have resolved.
///
/// Which names those are, and which addresses they stand for, is the one reading of them this
/// tree holds (`service_common::public_dns_standard`); a type such a name carries no record of
/// is an answer with no record, because the name itself exists.
fn standard_public_records(question: &Question<'_>) -> Option<Vec<ResourceRecord<'static>>> {
    let asked = question.qname.to_string();
    if !::service_common::public_dns_standard::names_a_fixed_address(&asked) {
        return None;
    }
    let owner = question.qname.clone().into_owned();
    let record_type = type_keyword(question.qtype).unwrap_or_default();
    Some(
        ::service_common::public_dns_standard::fixed_addresses_of_type(&asked, record_type)
            .into_iter()
            .map(|address| {
                let rdata = match address {
                    IpAddr::V4(address) => RData::A(A {
                        address: u32::from(address),
                    }),
                    IpAddr::V6(address) => RData::AAAA(AAAA {
                        address: u128::from(address),
                    }),
                };
                ResourceRecord::new(owner.clone(), CLASS::IN, ANSWER_TTL_SECONDS, rdata)
            })
            .collect(),
    )
}

/// One record of an answer section as a query log states it, and `None` for a record type this
/// resolver does not name — the inverse of [`private_zone_record`], over the same set of types.
fn logged_answer(record: &ResourceRecord<'_>) -> Option<LoggedAnswer> {
    let (record_type, rdata) = match &record.rdata {
        RData::A(A { address }) => ("A", Ipv4Addr::from(*address).to_string()),
        RData::AAAA(AAAA { address }) => ("AAAA", ::std::net::Ipv6Addr::from(*address).to_string()),
        RData::CNAME(CNAME(name)) => ("CNAME", name.to_string()),
        RData::NS(NS(name)) => ("NS", name.to_string()),
        RData::PTR(PTR(name)) => ("PTR", name.to_string()),
        RData::TXT(text) => (
            "TXT",
            // The character strings of the record, as the wire carried them: the crate reads a
            // `TXT` record into the `name=value` attributes a text record states, so the value
            // is rendered back from them rather than from the record set the answer came out of.
            text.attributes()
                .into_iter()
                .map(|(name, value)| match value {
                    Some(value) => format!("{name}={value}"),
                    None => name,
                })
                .collect::<Vec<String>>()
                .join(" "),
        ),
        RData::MX(MX {
            preference,
            exchange,
        }) => ("MX", format!("{preference} {exchange}")),
        _other => return None,
    };
    Some(LoggedAnswer {
        rdata,
        record_type: record_type.to_string(),
        class: CLASS_INTERNET.to_string(),
    })
}

/// What one question came to in the DNS Firewall of the VPC.
enum Filtered {
    /// `BLOCK` with `BlockResponse` `NXDOMAIN`: the name is answered as one that does not
    /// exist, with no record.
    NxDomain,
    /// `BLOCK` with `BlockResponse` `NODATA`: a successful answer carrying no record.
    NoData,
    /// `BLOCK` with `BlockResponse` `OVERRIDE`: the rule's own record answers the question.
    Record(Box<ResourceRecord<'static>>),
}

/// How one question is answered once the firewall has decided it, or `None` for a decision
/// that lets the query through to the zone.
fn filtered_answer(asked: &Name<'_>, decision: FirewallDecision) -> Option<Filtered> {
    match decision {
        FirewallDecision::Allow => None,
        FirewallDecision::BlockNxDomain => Some(Filtered::NxDomain),
        FirewallDecision::BlockNoData => Some(Filtered::NoData),
        FirewallDecision::BlockOverride {
            domain,
            dns_type,
            ttl,
        } => {
            // The override answers with the rule's OWN record: the one type such a rule may
            // name is a `CNAME`, so a rule naming another carries no record this resolver can
            // write, and the query is answered as the block with no record it is.
            if !dns_type.eq_ignore_ascii_case(OVERRIDE_DNS_TYPE_CNAME) {
                return Some(Filtered::NoData);
            }
            let Ok(target) = Name::new(&domain) else {
                return Some(Filtered::NoData);
            };
            Some(Filtered::Record(Box::new(ResourceRecord::new(
                asked.clone().into_owned(),
                CLASS::IN,
                ttl,
                RData::CNAME(CNAME(target.into_owned())),
            ))))
        }
    }
}

/// The TYPE keyword a question asks about, as a hosted zone's record sets are held under
/// (`ResourceRecordSet$Type`), and `None` for a question type no record set carries — a request
/// for every type at once, and one this emulator cannot name.
///
/// This is the whole set of types a private hosted zone's answer is carried in below
/// ([`private_zone_record`]): a record set of another type is one no question here asks for.
fn type_keyword(kind: QTYPE) -> Option<&'static str> {
    match kind {
        QTYPE::TYPE(TYPE::A) => Some("A"),
        QTYPE::TYPE(TYPE::AAAA) => Some("AAAA"),
        QTYPE::TYPE(TYPE::CNAME) => Some("CNAME"),
        QTYPE::TYPE(TYPE::NS) => Some("NS"),
        QTYPE::TYPE(TYPE::PTR) => Some("PTR"),
        QTYPE::TYPE(TYPE::TXT) => Some("TXT"),
        QTYPE::TYPE(TYPE::MX) => Some("MX"),
        _other => None,
    }
}

/// The answer section a private hosted zone's answer comes to, in the order the zone answered:
/// one record per value it holds, and nothing at all for a value that is not one of its type —
/// which is a record set the zone's own operations could not have written.
fn private_zone_records(answered: &PrivateZoneAnswer) -> Vec<ResourceRecord<'static>> {
    answered
        .records
        .iter()
        .filter_map(private_zone_record)
        .collect()
}

/// One record of such an answer, and `None` for a value that is not one of its own type.
fn private_zone_record(record: &PrivateZoneRecord) -> Option<ResourceRecord<'static>> {
    let owner = Name::new(&record.name).ok()?.into_owned();
    // A record set's TTL is held as the model declares it (`ResourceRecordSet$TTL`, 0..172800),
    // and the header field it is written into is an unsigned 32-bit count of seconds.
    let ttl = u32::try_from(record.ttl).unwrap_or(0);
    let value = record.value.trim();
    let rdata = match record.record_type.to_ascii_uppercase().as_str() {
        "A" => RData::A(A {
            address: u32::from(value.parse::<Ipv4Addr>().ok()?),
        }),
        "AAAA" => RData::AAAA(AAAA {
            address: u128::from(value.parse::<::std::net::Ipv6Addr>().ok()?),
        }),
        "CNAME" => RData::CNAME(CNAME(Name::new(value).ok()?.into_owned())),
        "NS" => RData::NS(NS(Name::new(value).ok()?.into_owned())),
        "PTR" => RData::PTR(PTR(Name::new(value).ok()?.into_owned())),
        // A `TXT` record set holds its character strings quoted, which is how Route 53 takes
        // and reports them; what travels on the wire is the string itself.
        "TXT" => RData::TXT(
            TXT::new()
                .with_string(value.trim_matches('"'))
                .ok()?
                .into_owned(),
        ),
        // "<preference> <exchange>" is the one form an `MX` value takes.
        "MX" => {
            let (preference, exchange) = value.split_once(char::is_whitespace)?;
            RData::MX(MX {
                preference: preference.trim().parse::<u16>().ok()?,
                exchange: Name::new(exchange.trim()).ok()?.into_owned(),
            })
        }
        _other => return None,
    };
    Some(ResourceRecord::new(owner, CLASS::IN, ttl, rdata))
}

/// What one question came to in the resolver's own zone: a record, a name of its own zone with
/// no record of the type asked about, a name of that zone it holds nothing under, a name the
/// public namespace establishes does not exist, or a name of another zone.
enum Answer {
    /// The record itself.
    Found(Box<ResourceRecord<'static>>),
    /// The name exists in the zone, but carries no record of the type asked about.
    Empty,
    /// The zone is the resolver's and holds no such name.
    Absent,
    /// The name belongs to NO zone, and the public namespace establishes as much
    /// ([`::service_common::vpc_private_dns::established_not_to_exist`]): the name does not
    /// exist, and this resolver is not the authority that says so — the recursion is.
    Nonexistent,
    /// The name belongs to another zone, which this resolver would have to recurse for.
    Elsewhere,
}

/// The address an Amazon-provided `ip-name` hostname names: `ip-10-0-1-5` is `10.0.1.5`.
///
/// `None` for a label of any other form, which names no address at all.
fn address_in_name(host: &str) -> Option<Ipv4Addr> {
    if host.contains('.') {
        return None;
    }
    host.strip_prefix(IP_NAME_PREFIX)?
        .replace('-', ".")
        .parse::<Ipv4Addr>()
        .ok()
}

/// The address a reverse question names: the four labels of `in-addr.arpa`, in the reverse of
/// the address's own order (`5.1.0.10` is `10.0.1.5`).
fn reversed_address(labels: &str) -> Option<Ipv4Addr> {
    let mut octets: Vec<&str> = labels.split('.').collect();
    if octets.len() != 4 {
        return None;
    }
    octets.reverse();
    octets.join(".").parse::<Ipv4Addr>().ok()
}

#[cfg(test)]
pub(crate) mod tests;
