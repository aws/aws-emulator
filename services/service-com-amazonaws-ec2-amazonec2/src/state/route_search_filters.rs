// SPDX-License-Identifier: CC0-1.0

//! The `route-search.*` filters an EC2 route search is narrowed by: which CIDR relation each
//! documented name asks for, and the evaluation of that relation against a route's destination.
//!
//! The ONE implementation of those relations for this crate. Every operation that searches a
//! route table publishes the same four names — "route-search.exact-match", "…
//! longest-prefix-match", "…subnet-of-match" and "…supernet-of-match" — and relates them to the
//! destination each route matches, so the relation belongs here beside [`super::CompiledFilter`]
//! and [`super::MatchBudget`] rather than once per operation, where two copies could drift on
//! the orientation of a containment or on how a comparison is charged.
//!
//! What stays with the operation is what differs between them: which filter names it documents
//! at all, how it words a refusal of a value that is no CIDR block, and how it reaches a
//! route's destination. These take a destination as text, so nothing here is shaped by one
//! operation's route type.
//!
//! Re-exported from [`crate::state`], so a handler reaches these as `crate::state::<item>` like
//! every other primitive there.

use super::{MatchBudget, cidr_block_contains_block, cidr_block_prefix_length};

/// Which CIDR relation a `route-search.*` filter asks for between a route's destination and the
/// block the filter names.
///
/// These are the filters that are NOT text comparisons: each one relates two CIDR blocks, so a
/// value is read as a block and compared by address range rather than by the `*`/`?` pattern an
/// attribute filter uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouteSearchKind {
    /// "route-search.exact-match - The exact match of the specified filter": the route's
    /// destination IS the block.
    Exact,
    /// "route-search.longest-prefix-match - The longest prefix that matches the route": of the
    /// destinations that contain the block, the most specific one.
    LongestPrefix,
    /// "route-search.subnet-of-match - The routes with a subnet that match the specified CIDR
    /// filter": the route's destination is inside the block.
    SubnetOf,
    /// "route-search.supernet-of-match - The routes with a CIDR that encompass the CIDR filter.
    /// For example, if you have 10.0.1.0/29 and 10.0.1.0/31 routes in your route table and you
    /// specify supernet-of-match as 10.0.1.0/30, then the result returns 10.0.1.0/29": the
    /// route's destination encompasses the block, which is that worked example's relation and
    /// not [`Self::SubnetOf`]'s.
    SupernetOf,
}

impl RouteSearchKind {
    /// The relation `name` asks for, or `None` when it is not a `route-search.*` filter at all
    /// (an attribute filter, compared as text by the operation's own value reader).
    pub fn of_filter(name: &str) -> Option<Self> {
        match name {
            "route-search.exact-match" => Some(Self::Exact),
            "route-search.longest-prefix-match" => Some(Self::LongestPrefix),
            "route-search.subnet-of-match" => Some(Self::SubnetOf),
            "route-search.supernet-of-match" => Some(Self::SupernetOf),
            _ => None,
        }
    }
}

/// One `route-search.*` filter: the relation it asks for and its values read as CIDR blocks
/// ONCE, before any route is compared against them.
#[derive(Debug)]
pub struct RouteSearch {
    /// The relation the filter asks for.
    kind: RouteSearchKind,
    /// The filter's values, canonical ([`super::canonical_cidr_block`]), in the order the
    /// request gave them.
    blocks: Vec<String>,
    /// For a [`RouteSearchKind::LongestPrefix`] filter, the longest destination prefix in the
    /// table that contains each value — one entry per entry of [`Self::blocks`], and `None`
    /// where no route's destination contains that value. Left all-`None` for every other
    /// relation, which needs no cross-route comparison.
    longest: Vec<Option<u32>>,
}

impl RouteSearch {
    /// The filter asking `kind` of each of `blocks`, which the caller has already read as
    /// canonical CIDR blocks — refusing a value that is not one in the words its own operation
    /// answers.
    pub fn new(kind: RouteSearchKind, blocks: Vec<String>) -> Self {
        let longest = vec![None; blocks.len()];
        Self {
            kind,
            blocks,
            longest,
        }
    }

    /// Find, for a [`RouteSearchKind::LongestPrefix`] filter, the most specific of `destinations`
    /// containing each of this filter's values; a no-op for every other relation.
    ///
    /// "route-search.longest-prefix-match - The longest prefix that matches the route": unlike
    /// every other filter this is not a property of one route but a comparison BETWEEN the
    /// table's routes, so it is resolved over the WHOLE table before any route is selected —
    /// each filter against the table independently, before the filters are joined with an AND.
    ///
    /// Every comparison is charged to `budget`, so this cannot cost more than the ceiling
    /// however many values and destinations the request meets.
    pub fn resolve_longest_prefix<S: AsRef<str>>(
        &mut self,
        destinations: &[S],
        budget: &mut MatchBudget,
    ) {
        if self.kind != RouteSearchKind::LongestPrefix {
            return;
        }
        for (index, block) in self.blocks.iter().enumerate() {
            let mut best: Option<u32> = None;
            for destination in destinations {
                if !budget.spend() {
                    break;
                }
                let destination = destination.as_ref();
                if cidr_block_contains_block(destination, block) {
                    let length = cidr_block_prefix_length(destination);
                    best = match (best, length) {
                        (Some(best), Some(length)) => Some(best.max(length)),
                        (best, length) => best.or(length),
                    };
                }
            }
            if let Some(slot) = self.longest.get_mut(index) {
                *slot = best;
            }
        }
    }

    /// Does a route whose canonical destination block is `destination` satisfy this filter?
    ///
    /// "If you specify multiple values for a filter, the values are joined with an OR"
    /// (`Filter$Values`), so one value holding is enough — and a filter carrying no value at all
    /// matches nothing, there being no block for the destination to relate to. A route that
    /// carries no destination BLOCK at all (one whose destination is a prefix list) matches no
    /// `route-search.*` filter, the same answer EC2 gives for any attribute a resource holds no
    /// value for.
    ///
    /// Every comparison is charged to `budget`, so one request's searching cannot cost more than
    /// the ceiling however it combines filters and values.
    pub fn matches(&self, destination: Option<&str>, budget: &mut MatchBudget) -> bool {
        let Some(destination) = destination else {
            return false;
        };
        self.blocks.iter().enumerate().any(|(index, block)| {
            if !budget.spend() {
                return false;
            }
            match self.kind {
                RouteSearchKind::Exact => destination == block.as_str(),
                RouteSearchKind::SubnetOf => cidr_block_contains_block(block, destination),
                RouteSearchKind::SupernetOf => cidr_block_contains_block(destination, block),
                // The longest prefix was found over the whole table by
                // [`Self::resolve_longest_prefix`]: this route is selected when its destination
                // contains the value and is exactly that long, so two equally specific
                // destinations are both reported.
                RouteSearchKind::LongestPrefix => self
                    .longest
                    .get(index)
                    .copied()
                    .flatten()
                    .is_some_and(|best| {
                        cidr_block_prefix_length(destination) == Some(best)
                            && cidr_block_contains_block(destination, block)
                    }),
            }
        })
    }
}
