// SPDX-License-Identifier: CC0-1.0

//! How a graph's COMPUTE is configured: the one value every graph record reports it
//! through both of the members that name it, and the rules a request naming it must
//! satisfy.
//!
//! A graph is served either by dedicated compute that is always available (a
//! *provisioned graph*, sized in m-NCUs) or by ephemeral compute bootstrapped when a
//! query arrives (a *graph session*, floored in s-NCUs) —
//! `smithy_query(query="doc",shape="CreateGraph")`. The model carries that choice as the
//! `ComputeConfiguration` union, and keeps the older `provisionedMemory` member beside
//! it as a DEPRECATED second spelling of the same quantity, which every graph record
//! reports as "`memory` for a `provisionedGraph` and `minMemory` for a `graphSession`"
//! (`smithy_query(query="doc",shape="GraphSummary",member="provisionedMemory")`).
//!
//! So a stored graph holds ONE compute configuration ([`GraphCompute`]) and both members
//! are rendered from it, rather than a memory number beside a configuration that could
//! disagree with it.

use super::*;

/// The least provisioned memory a request may ask a graph for, from the member's own
/// documentation: "The provisioned memory-optimized Neptune Capacity Units (m-NCUs) to
/// use for the graph. Min = 16"
/// (`smithy_query(query="doc",shape="CreateGraphInput",member="provisionedMemory")`, and
/// the same sentence on `UpdateGraphInput` and `RestoreGraphFromSnapshotInput`).
///
/// The modelled `@range` on `ProvisionedMemory` is `min: 1, max: 24576`
/// (`smithy_query(query="shape",shape="ProvisionedMemory")`) — FIFTEEN below the number
/// those sentences publish — because the deprecated member also carries a graph
/// session's `minMemory` out (4 s-NCUs at the least, see [`GraphCompute::reported_memory`]),
/// which no range floored at the provisioned minimum could hold. So the boundary
/// validator admits a request asking for less memory than the service documents, and the
/// published floor is the bound enforced here ([`provisioned_memory_clause`]), exactly as
/// the published vector-search maximum is ([`MAX_VECTOR_SEARCH_DIMENSION`]). Every value
/// past the modelled maximum is already refused at the boundary by the `@range` trait, so
/// this is the only part of the documented range nothing else applies.
pub const MIN_PROVISIONED_MEMORY: i32 = 16;

/// The compute serving one stored graph: the stored form of the modelled
/// `ComputeConfiguration` union (`smithy_query(query="shape",shape="ComputeConfiguration")`).
///
/// A graph has exactly one, so this is an enum rather than a pair of optional members:
/// the two members every graph record reports its compute through are both rendered from
/// this one value ([`GraphCompute::reported_memory`], [`GraphCompute::as_wire`]) and
/// cannot drift apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraphCompute {
    /// Dedicated compute that is always available, of this many memory-optimized
    /// Neptune Capacity Units.
    Provisioned { memory: i32 },
    /// Ephemeral compute bootstrapped when a query reaches the graph, with the floor of
    /// Session Neptune Capacity Units the request named, if it named one: `minMemory` is
    /// an OPTIONAL member whose documentation publishes no default
    /// (`smithy_query(query="shape",shape="GraphSessionConfiguration")`), so a session
    /// nobody floored holds no floor to report rather than one this emulator invented.
    Session { min_memory: Option<i32> },
}

impl GraphCompute {
    /// A provisioned graph of `memory` m-NCUs: what the deprecated `provisionedMemory`
    /// member asks for, and what a graph minted by an operation that models no
    /// `computeConfiguration` member at all (`CreateGraphUsingImportTask`) is served by.
    pub const fn provisioned(memory: i32) -> Self {
        Self::Provisioned { memory }
    }

    /// The value the DEPRECATED `provisionedMemory` member reports for this compute:
    /// "it returns `memory` for a `provisionedGraph` and `minMemory` for a
    /// `graphSession`"
    /// (`smithy_query(query="doc",shape="GraphSummary",member="provisionedMemory")`).
    ///
    /// `None` only for a graph session nobody named a floor for, which has no number to
    /// report through either member.
    pub const fn reported_memory(self) -> Option<i32> {
        match self {
            Self::Provisioned { memory } => Some(memory),
            Self::Session { min_memory } => min_memory,
        }
    }

    /// This compute as the modelled `ComputeConfiguration` union every graph record
    /// reports it through.
    pub fn as_wire(self) -> ComputeConfiguration {
        match self {
            Self::Provisioned { memory } => {
                ComputeConfiguration::provisionedGraph(ProvisionedGraphConfiguration { memory })
            }
            Self::Session { min_memory } => {
                ComputeConfiguration::graphSession(GraphSessionConfiguration { min_memory })
            }
        }
    }

    /// The compute a request asks for through either of the members that name it, or
    /// `None` when it names neither — which is a request that leaves the compute to the
    /// operation: unchanged for an update, the snapshotted graph's for a restore.
    ///
    /// The two members are mutually exclusive ([`compute_member_clauses`]), so a request
    /// that reaches here carries at most one of them and this cannot have to choose
    /// between two answers.
    pub fn requested(
        provisioned_memory: Option<i32>,
        configuration: Option<&ComputeConfiguration>,
    ) -> Option<Self> {
        match configuration {
            Some(ComputeConfiguration::provisionedGraph(provisioned)) => {
                Some(Self::provisioned(provisioned.memory))
            }
            Some(ComputeConfiguration::graphSession(session)) => Some(Self::Session {
                min_memory: session.min_memory,
            }),
            None => provisioned_memory.map(Self::provisioned),
        }
    }
}

/// The member path the DEPRECATED spelling of a graph's provisioned memory is reported
/// at, which every operation that accepts it spells the same way.
const PROVISIONED_MEMORY_MEMBER: &str = "provisionedMemory";

/// The member path the CURRENT spelling of a provisioned graph's memory is reported at:
/// the configuration member, the union variant, then the memory inside it — the way the
/// walk in front of a handler spells a path into a nested member (lowerCamel segments
/// joined by dots).
const PROVISIONED_GRAPH_MEMORY_MEMBER: &str = "computeConfiguration.provisionedGraph.memory";

/// The `ValidationException` clause a provisioned-memory member fails, or `None` when
/// the value is at or above the documented floor ([`MIN_PROVISIONED_MEMORY`]) or absent.
///
/// Stated ONCE here and reached by every operation that accepts a member naming the
/// memory a graph is provisioned with — whichever of the members names it, deprecated or
/// current, and whether it IS that memory or merely BOUNDS the value the service chooses
/// (`CreateGraphUsingImportTask`'s two bounds) — so no two of them can drift into
/// different acceptance sets and no stored graph can hold less memory than the service's
/// own documentation puts in range.
///
/// The clause is the one the boundary validator renders for a `@range` minimum on the
/// same member — the value, its member path, and the bound as the requirement — so a
/// caller cannot tell the bound this handler applies from the bound the model carries.
pub fn provisioned_memory_clause(member_path: &str, memory: Option<i32>) -> Option<String> {
    let memory = memory?;
    if memory < MIN_PROVISIONED_MEMORY {
        return Some(constraint_clause(
            member_path,
            Some(&memory.to_string()),
            &min_value_requirement(MIN_PROVISIONED_MEMORY),
        ));
    }
    None
}

/// The requirement clause for the DEPRECATED member of a pair naming one quantity, when
/// the request supplied the current member as well: the two name the same thing, so a
/// request carrying both says it twice and the service cannot know which to believe.
///
/// Worded by a builder like every other requirement this service states, so the rule
/// reads the same whichever operation carried the pair.
fn exclusive_member_requirement(other_member: &str) -> String {
    format!("Member must not be provided together with {other_member}")
}

/// The requirement clause for a member that has to be provided because the deprecated
/// member it replaces was not: the pair is what names the quantity, so a request naming
/// neither half leaves it unnamed.
fn either_member_requirement(other_member: &str) -> String {
    format!("Member must be provided when {other_member} is not provided")
}

/// Every `ValidationException` clause the COMPUTE members of a request earn, in the order
/// the request spells them. Empty for a request whose compute members are well formed,
/// including one that names none of them.
///
/// Stated ONCE here and reached by every operation that accepts both spellings of a
/// graph's compute (`CreateGraph`, `UpdateGraph`, `RestoreGraphFromSnapshot`), so the
/// three cannot drift into different acceptance sets for one pair of documented members:
///
/// * `provisionedMemory` is "deprecated. Use `ComputeConfiguration` instead."
///   (`smithy_query(query="operation",operation="CreateGraph")` -> input.provisionedMemory
///   `@deprecated`), so it is the OLDER spelling of the memory
///   `computeConfiguration.provisionedGraph` carries — one quantity under two names, and
///   a request that names it twice is refused rather than served from whichever half
///   this emulator happened to read first.
/// * A provisioned graph's memory satisfies the documented floor whichever member named
///   it ([`provisioned_memory_clause`]).
///
/// A graph session's own floor is modelled (`GraphSessionMemory`, `@range min: 4, max: 8`),
/// so the boundary validator refuses one out of range before any handler runs and there
/// is nothing to restate about it here.
pub fn compute_member_clauses(
    provisioned_memory: Option<i32>,
    configuration: Option<&ComputeConfiguration>,
) -> Vec<String> {
    let mut clauses = Vec::new();
    if provisioned_memory.is_some() && configuration.is_some() {
        let named = provisioned_memory.map(|memory| memory.to_string());
        clauses.push(constraint_clause(
            PROVISIONED_MEMORY_MEMBER,
            named.as_deref(),
            &exclusive_member_requirement("computeConfiguration"),
        ));
    }
    let deprecated = provisioned_memory_clause(PROVISIONED_MEMORY_MEMBER, provisioned_memory);
    if let Some(clause) = deprecated {
        clauses.push(clause);
    }
    if let Some(ComputeConfiguration::provisionedGraph(provisioned)) = configuration {
        if let Some(clause) =
            provisioned_memory_clause(PROVISIONED_GRAPH_MEMORY_MEMBER, Some(provisioned.memory))
        {
            clauses.push(clause);
        }
    }
    clauses
}

/// The `ValidationException` clause a request that must name a graph's compute and names
/// NEITHER member earns.
///
/// Reached by the operation that MINTS a graph from nothing but the request
/// (`CreateGraph`): a graph is served by one compute configuration or the other, and
/// neither member's documentation publishes a default for an omitted value, so a request
/// that names neither is refused rather than served with a size nothing asked for. The
/// operations that mint or reconfigure a graph which ALREADY has a compute configuration
/// to fall back on (`RestoreGraphFromSnapshot`, `UpdateGraph`) do not reach this: an
/// omitted member there keeps the captured or stored compute.
pub fn compute_not_named_clause() -> String {
    constraint_clause(
        "computeConfiguration",
        None,
        &either_member_requirement(PROVISIONED_MEMORY_MEMBER),
    )
}
