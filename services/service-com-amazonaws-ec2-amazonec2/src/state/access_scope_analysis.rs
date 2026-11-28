// SPDX-License-Identifier: CC0-1.0

//! Running a Network Access Scope ANALYSIS: the findings a scope has, read off the network
//! interfaces, security groups, network ACLs and routes the account actually holds.
//!
//! "Network Access Analyzer … identifies unintended network access to your resources on
//! Amazon Web Services" (Amazon VPC Network Access Analyzer guide): a scope describes the
//! access a caller asks about, and a FINDING is one path through the stored network that
//! matches it. So `NetworkInsightsAccessScopeAnalysis$FindingsFound` and the findings
//! `GetNetworkInsightsAccessScopeAnalysisFindings` reports are COMPUTED here from that state,
//! the way a Reachability Analyzer verdict is ([`super::reachability_trace`]), rather than
//! reported as a result nothing produced:
//!
//! 1. every network interface of the account is one end a path can have — that is what makes
//!    it one of the "network interfaces analyzed" (`$AnalyzedEniCount`);
//! 2. each ordered pair of interfaces in one VPC is a candidate path, selected by the scope's
//!    `MatchPaths` (a scope that names none matches every pair) and dropped by its
//!    `ExcludePaths`;
//! 3. a selected pair is a finding when the stored network CARRIES a packet from the one to
//!    the other, which is the trace [`ComAmazonawsEc2Amazonec2State::trace_network_insights_path`]
//!    walks, and the finding's components are the hops that trace walked.
//!
//! What the trace does not walk is reported rather than guessed: a match path that names
//! `ThroughResources`, one whose packet header constrains the packet by something this walk
//! does not read (an address, a source port, a prefix list, a protocol other than TCP or
//! UDP), an exclude path whose packet header this walk cannot apply, and a scope whose
//! candidate pairs or findings run past the bounds below, all leave the analysis carrying a
//! `WarningMessage` saying the report is partial.
//!
//! Split out of the [`crate::state`] module root so each file holds one concern, and
//! re-exported from it, so a handler reaches these as `crate::state::<item>` like every other
//! primitive there.

use super::*;
use crate::ops::types::{
    AccessScopeAnalysisFinding, AccessScopePath, NetworkInsightsPath, PathStatement,
    ResourceStatement,
};

/// The most findings one analysis reports.
///
/// A capacity bound on this process's memory in the same style as
/// [`MAX_NETWORK_INSIGHTS_ACCESS_SCOPE_ANALYSES`], not an AWS quota: the findings of every
/// analysis the account has run are held for as long as the analysis is, so the number one
/// analysis can contribute has to be finite. An analysis that reaches it says so in its
/// `WarningMessage` rather than reporting a complete report it did not produce.
pub const MAX_ACCESS_SCOPE_FINDINGS: usize = 100;

/// The most Network Insights analyses ONE clock pass runs.
///
/// Every handler of this service materialises the moves the clock owes while holding the
/// store, so the work one pass does has to be bounded by something other than how many
/// analyses the account has started: past this many, the rest stay DUE and are run by the
/// next pass, which reports exactly what a caller that had called then would have seen.
///
/// The one bound for BOTH kinds of analysis this service runs on that clock — a Network
/// Access Scope analysis and a Reachability Analyzer path analysis — since each traces the
/// stored network while the store is borrowed, so neither can be the unbounded one.
pub const MAX_SCOPE_ANALYSES_PER_CLOCK_PASS: usize = 8;

/// The digits a finding's position in its analysis's report is written to.
///
/// Wide enough for every position [`MAX_ACCESS_SCOPE_FINDINGS`] admits, so the ids of one
/// report sort in the order the analysis found them: a page cursor over those ids names a
/// position in that order, which it could not if `…/2` sorted after `…/10`.
const FINDING_POSITION_DIGITS: usize = 3;

/// The most candidate paths one analysis walks: the pairs of network interfaces it INSPECTS.
///
/// The candidates are the ORDERED PAIRS of the account's interfaces, walked once per match
/// path of the scope, so the work one analysis does grows with the square of the interface
/// count and with the number of paths. Every pair the walk inspects is charged to this one
/// budget — before the pair is looked at, so a pair a filter then skips is charged too —
/// which is what bounds that product however many interfaces the account holds and however
/// many paths the scope names. The same contract the crate's filtered describes have
/// ([`MAX_FILTER_MATCH_STEPS`]); an analysis that reaches it says so in its `WarningMessage`.
pub const MAX_ACCESS_SCOPE_CANDIDATE_PATHS: usize = 2_000;

/// "The warning message" (`NetworkInsightsAccessScopeAnalysis$WarningMessage`) of an analysis
/// whose report is PARTIAL: the scope asked about more than one analysis of this emulator
/// walks, so what it reports is some of the findings rather than all of them.
pub const ACCESS_SCOPE_ANALYSIS_PARTIAL_WARNING: &str =
    "This analysis reports the matching paths it evaluated: the scope asked about more \
     paths, more packets or more resources to pass through than one analysis evaluates.";

/// The `FindingsFound` enum's `true` member: the analysis found at least one path matching
/// the scope.
pub const FINDINGS_FOUND_TRUE: &str = "true";

/// The `FindingsFound` enum's `false` member: the analysis ran and no path matched the scope.
pub const FINDINGS_FOUND_FALSE: &str = "false";

/// The resource type word a scope's `ResourceStatement` names a network interface by.
///
/// "The resource types" (`ResourceStatement$ResourceTypes`) are named the way the Network
/// Access Analyzer guide writes them — the CloudFormation type of the resource
/// (`AWS::EC2::NetworkInterface`) — so the four kinds an interface of this store belongs to
/// are spelled here once and nowhere else.
pub const SCOPE_RESOURCE_TYPE_NETWORK_INTERFACE: &str = "AWS::EC2::NetworkInterface";

/// The resource type word for the instance an interface is attached to (see
/// [`SCOPE_RESOURCE_TYPE_NETWORK_INTERFACE`]).
pub const SCOPE_RESOURCE_TYPE_INSTANCE: &str = "AWS::EC2::Instance";

/// The resource type word for the subnet an interface sits in.
pub const SCOPE_RESOURCE_TYPE_SUBNET: &str = "AWS::EC2::Subnet";

/// The resource type word for the VPC that subnet belongs to.
pub const SCOPE_RESOURCE_TYPE_VPC: &str = "AWS::EC2::VPC";

/// One end a candidate path may have: a network interface of the account, with the resources
/// it belongs to, which is what a scope's resource statement names it by.
#[derive(Debug, Clone)]
struct ScopeEndpoint {
    /// The interface itself.
    network_interface_id: String,
    /// The instance it is attached to, for an attached interface.
    instance_id: Option<String>,
    /// The subnet it sits in.
    subnet_id: String,
    /// The VPC that subnet belongs to.
    vpc_id: String,
}

impl ScopeEndpoint {
    /// Whether `named` — an id or an ARN, read through the crate's one reader of a member
    /// that admits either form — names this interface or a resource it belongs to.
    fn is_named(&self, named: &str) -> bool {
        let id = arn_or_bare_resource_id(named);
        id == self.network_interface_id
            || id == self.subnet_id
            || id == self.vpc_id
            || self.instance_id.as_deref() == Some(id)
    }

    /// Whether `resource_type` is the type of this interface or of a resource it belongs to.
    fn has_type(&self, resource_type: &str) -> bool {
        match resource_type {
            SCOPE_RESOURCE_TYPE_NETWORK_INTERFACE | SCOPE_RESOURCE_TYPE_SUBNET
            | SCOPE_RESOURCE_TYPE_VPC => true,
            SCOPE_RESOURCE_TYPE_INSTANCE => self.instance_id.is_some(),
            _ => false,
        }
    }

    /// Whether a scope's resource statement selects this end.
    ///
    /// "The resources" and "the resource types" (`ResourceStatement`) are alternatives a
    /// statement may name either or both of, so an end is selected when it is named by one of
    /// them — and a statement that constrains NEITHER selects every end, there being nothing
    /// to narrow it by.
    fn selected_by(&self, statement: &ResourceStatement) -> bool {
        let resources = statement.resources.as_deref().unwrap_or_default();
        let resource_types = statement.resource_types.as_deref().unwrap_or_default();
        if resources.is_empty() && resource_types.is_empty() {
            return true;
        }
        resources.iter().any(|named| self.is_named(named))
            || resource_types
                .iter()
                .any(|resource_type| self.has_type(resource_type))
    }
}

/// Whether the path statement `statement` selects the end `endpoint`.
///
/// A statement the scope left out constrains nothing, and one that carries only a packet
/// header statement constrains the PACKET rather than the end ([`access_scope_packet`]), so
/// both select every end.
fn statement_selects(statement: Option<&PathStatement>, endpoint: &ScopeEndpoint) -> bool {
    statement.is_none_or(|statement| {
        statement
            .resource_statement
            .as_ref()
            .is_none_or(|resources| endpoint.selected_by(resources))
    })
}

/// Whether every member of the packet header the path statement `statement` carries is one
/// this walk READS.
///
/// The walk reads the protocol and the destination port of a packet
/// ([`access_scope_packet`]); `PacketHeaderStatement` also names source and destination
/// ADDRESSES, SOURCE ports and PREFIX LISTS, and its protocols may name any IANA protocol. A
/// scope that constrains the packet by something this walk does not read is asking about fewer
/// packets than the walk evaluates, so the analysis says its report is PARTIAL rather than
/// reporting the wider answer as the one the scope asked for.
fn packet_header_is_read(statement: Option<&PathStatement>) -> bool {
    let Some(header) = statement.and_then(|statement| statement.packet_header_statement.as_ref())
    else {
        return true;
    };
    let unread = [
        header.source_addresses.as_deref(),
        header.destination_addresses.as_deref(),
        header.source_ports.as_deref(),
        header.source_prefix_lists.as_deref(),
        header.destination_prefix_lists.as_deref(),
    ]
    .into_iter()
    .flatten()
    .any(|named| !named.is_empty());
    let protocols_read = header
        .protocols
        .as_deref()
        .unwrap_or_default()
        .iter()
        .all(|protocol| {
            [IP_PROTOCOL_TCP, IP_PROTOCOL_UDP]
                .into_iter()
                .any(|known| protocol.eq_ignore_ascii_case(known))
        });
    !unread && protocols_read
}

/// Whether both ends of `path` carry only packet-header members this walk reads.
fn path_packet_is_read(path: &AccessScopePath) -> bool {
    packet_header_is_read(path.source.as_ref()) && packet_header_is_read(path.destination.as_ref())
}

/// Whether `path` carries a PACKET header at all, on either end.
///
/// What an EXCLUDE path is asked: a packet header there excludes some PACKETS of a pair of
/// ends rather than the pair itself, and this walk evaluates one packet per pair — so such a
/// path's resource statements are applied and its packet header is not, with the analysis
/// saying the report is partial.
fn path_names_a_packet_header(path: &AccessScopePath) -> bool {
    [path.source.as_ref(), path.destination.as_ref()]
        .into_iter()
        .flatten()
        .any(|statement| statement.packet_header_statement.is_some())
}

/// Whether `path` constrains either end by the RESOURCES it names.
///
/// An exclude path that names none would exclude every pair the walk could report, which is
/// what a path carrying only a packet header would do if its header were read as a constraint
/// on the ends: such a path excludes nothing here, and the analysis says so.
fn path_names_a_resource(path: &AccessScopePath) -> bool {
    [path.source.as_ref(), path.destination.as_ref()]
        .into_iter()
        .flatten()
        .any(|statement| statement.resource_statement.is_some())
}

/// The packet one match path asks about: its protocol, and the destination port when the path
/// names one.
///
/// "The packet header statement" (`PathStatement$PacketHeaderStatement`) is where a scope
/// names the protocols and ports it asks about; the DESTINATION end's statement describes the
/// packet as it arrives, so it is read first and the source end's only when the destination
/// names none. A path that names no protocol this trace reads asks about TCP, the protocol a
/// security group rule and a network ACL entry are written for by default, and a path that
/// names no port asks about every port — which is what the trace reads a `None` port as.
fn access_scope_packet(path: &AccessScopePath) -> (&'static str, Option<i32>) {
    let headers = [path.destination.as_ref(), path.source.as_ref()]
        .into_iter()
        .flatten()
        .filter_map(|statement| statement.packet_header_statement.as_ref());
    for header in headers {
        let protocol = header
            .protocols
            .as_deref()
            .unwrap_or_default()
            .iter()
            .find_map(|protocol| {
                [IP_PROTOCOL_TCP, IP_PROTOCOL_UDP]
                    .into_iter()
                    .find(|known| protocol.eq_ignore_ascii_case(known))
            });
        let port = header
            .destination_ports
            .as_deref()
            .unwrap_or_default()
            .iter()
            .find_map(|port| port.parse::<i32>().ok());
        if protocol.is_some() || port.is_some() {
            return (protocol.unwrap_or(IP_PROTOCOL_TCP), port);
        }
    }
    (IP_PROTOCOL_TCP, None)
}

/// What one analysis of a Network Access Scope found.
#[derive(Debug, Clone, Default)]
pub struct AccessScopeAnalysisOutcome {
    /// The findings, in the order the analysis walked the candidate paths — which is the
    /// order `GetNetworkInsightsAccessScopeAnalysisFindings` reports and pages over.
    pub findings: Vec<AccessScopeAnalysisFinding>,
    /// "The number of network interfaces analyzed" (`$AnalyzedEniCount`).
    pub analyzed_eni_count: i32,
    /// Whether the report is PARTIAL, which the analysis says in its `WarningMessage`
    /// ([`ACCESS_SCOPE_ANALYSIS_PARTIAL_WARNING`]).
    pub partial: bool,
}

impl ComAmazonawsEc2Amazonec2State {
    /// Analyze the Network Access Scope `scope_id` against the network this account holds, as
    /// the analysis `analysis_id`, and answer what it found.
    ///
    /// A READ: every component is read and nothing is written, so two analyses of one scope
    /// over an unchanged network find the same paths. `region` is the Region the analysis was
    /// started in, which is where a prefix list a security group rule names is read from.
    pub fn analyze_network_insights_access_scope(
        &self,
        scope_id: &str,
        analysis_id: &str,
        region: &str,
    ) -> AccessScopeAnalysisOutcome {
        // Every interface of the account is one end a path can have, and so one of the
        // interfaces this analysis analyzed. An interface the trace cannot place — one
        // carrying no address, subnet or VPC — is no end of a path, so it is not a candidate.
        let endpoints: Vec<ScopeEndpoint> = self
            .network_interfaces
            .values()
            .filter_map(|interface| {
                Some(ScopeEndpoint {
                    network_interface_id: interface.network_interface_id.clone()?,
                    instance_id: interface
                        .attachment
                        .as_ref()
                        .and_then(|attachment| attachment.instance_id.clone()),
                    subnet_id: interface.subnet_id.clone()?,
                    vpc_id: interface.vpc_id.clone()?,
                })
            })
            .collect();
        let mut outcome = AccessScopeAnalysisOutcome {
            findings: Vec::new(),
            analyzed_eni_count: i32::try_from(endpoints.len()).unwrap_or(i32::MAX),
            partial: false,
        };

        // "The paths to match" and "the paths to exclude"
        // (`NetworkInsightsAccessScopeContent`): the content as the scope was created with,
        // which is what this analysis is OF. A scope the store holds no content for asks
        // about every path, the same as one that names no match path.
        let content = self.network_insights_access_scope_contents.get(scope_id);
        let match_paths: Vec<AccessScopePath> = content
            .and_then(|content| content.match_paths.clone())
            .unwrap_or_default();
        let exclude_paths: Vec<AccessScopePath> = content
            .and_then(|content| content.exclude_paths.clone())
            .unwrap_or_default();
        // A scope that names no match path matches every path, so the analysis walks one
        // unconstrained path rather than none at all.
        let unconstrained = AccessScopePath::default();
        let matched: Vec<&AccessScopePath> = if match_paths.is_empty() {
            vec![&unconstrained]
        } else {
            match_paths.iter().collect()
        };
        // The exclusions this walk can APPLY: those that constrain the ends by the resources
        // they name. An exclude path that names none would exclude every pair the walk could
        // report — which is what a path carrying only a packet header would do — and a packet
        // header on an exclude path excludes some PACKETS of a pair rather than the pair, so
        // the header is not applied. Either way the report is partial and says so, rather
        // than reporting fewer or more pairs as the ones the scope asked for.
        let excluded: Vec<&AccessScopePath> = exclude_paths
            .iter()
            .filter(|path| path_names_a_resource(path))
            .collect();
        if exclude_paths.len() != excluded.len()
            || exclude_paths.iter().any(path_names_a_packet_header)
        {
            outcome.partial = true;
        }

        let mut candidates = 0usize;
        for path in matched {
            // "The through resources" (`AccessScopePath$ThroughResources`) name resources the
            // path must pass through, which this analysis does not walk: the paths that
            // statement selects are left out of the report rather than reported as matched or
            // as absent, and the analysis says so.
            if path
                .through_resources
                .as_deref()
                .is_some_and(|through| !through.is_empty())
            {
                outcome.partial = true;
                continue;
            }
            // A packet header member this walk does not read — an address, a source port, a
            // prefix list, a protocol outside the two it reads — constrains the packet to
            // fewer than the walk evaluates, so the path is still walked (with the packet it
            // reads) and the analysis says its report is partial rather than reporting the
            // wider answer as the one the scope asked for.
            if !path_packet_is_read(path) {
                outcome.partial = true;
            }
            let (protocol, port) = access_scope_packet(path);
            for source in &endpoints {
                for destination in &endpoints {
                    // EVERY pair this walk INSPECTS is charged to the one budget, before any
                    // of it is looked at: the pairs are the product of the account's
                    // interfaces with themselves and of the scope's match paths, so charging
                    // only the pairs that survive a filter would leave that product
                    // uncapped. Past the budget — or once the report is full — the analysis
                    // stops and says the report is partial, which is the same contract the
                    // crate's filtered describes have ([`MatchBudget`]).
                    candidates = candidates.saturating_add(1);
                    if candidates > MAX_ACCESS_SCOPE_CANDIDATE_PATHS
                        || outcome.findings.len() >= MAX_ACCESS_SCOPE_FINDINGS
                    {
                        outcome.partial = true;
                        return outcome;
                    }
                    // A path runs BETWEEN two resources, and the trace walks one VPC: two
                    // ends in different VPCs are reached through a peering connection, a
                    // transit gateway or a gateway of their own, none of which it walks.
                    if source.network_interface_id == destination.network_interface_id
                        || source.vpc_id != destination.vpc_id
                    {
                        continue;
                    }
                    if !statement_selects(path.source.as_ref(), source)
                        || !statement_selects(path.destination.as_ref(), destination)
                    {
                        continue;
                    }
                    // An excluded path is one the scope asked NOT to be told about, so a pair
                    // any APPLIED exclusion selects is dropped even when a match path
                    // selected it.
                    if excluded.iter().any(|excluded| {
                        statement_selects(excluded.source.as_ref(), source)
                            && statement_selects(excluded.destination.as_ref(), destination)
                    }) {
                        continue;
                    }
                    // Whether the stored network CARRIES the packet from this end to that
                    // one: the route, the security groups and the network ACLs the packet
                    // meets, walked by the one trace of this crate. A pair it cannot walk
                    // contributes no finding.
                    let candidate = NetworkInsightsPath {
                        source: Some(source.network_interface_id.clone()),
                        destination: Some(destination.network_interface_id.clone()),
                        protocol: Some(protocol.to_string()),
                        destination_port: port,
                        ..NetworkInsightsPath::default()
                    };
                    let Some(trace) = self.trace_network_insights_path(&candidate, region) else {
                        continue;
                    };
                    if !trace.network_path_found {
                        continue;
                    }
                    outcome.findings.push(AccessScopeAnalysisFinding {
                        network_insights_access_scope_analysis_id: Some(analysis_id.to_string()),
                        network_insights_access_scope_id: Some(scope_id.to_string()),
                        // "The ID of the finding" (`AccessScopeAnalysisFinding$FindingId`):
                        // the analysis that produced it and the position of the finding in
                        // that analysis's report, so one finding has one id for as long as
                        // the analysis holds it and no two findings share one. The position
                        // is written to a fixed width, so the ids of one report sort in the
                        // order the analysis found them — which is the order the findings are
                        // reported and paged over, and what a page cursor names a position
                        // in.
                        finding_id: Some(format!(
                            "{analysis_id}/{position:0width$}",
                            position = outcome.findings.len() + 1,
                            width = FINDING_POSITION_DIGITS,
                        )),
                        // "The finding components" (`$FindingComponents`): the hops the trace
                        // walked from the one end to the other, in the order the packet meets
                        // them.
                        finding_components: Some(trace.forward_path_components),
                    });
                }
            }
        }
        outcome
    }
}
