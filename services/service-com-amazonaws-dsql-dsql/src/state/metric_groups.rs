// SPDX-License-Identifier: CC0-1.0

//! The ADVANCED MONITORING a cluster collects: which metric groups are enabled on it.
//!
//! `MetricGroup` names "an advanced monitoring metric group… You choose which metric
//! groups to collect for each cluster" (`smithy_query(query="shape",
//! shape="MetricGroup")`), and three operations carry the choice: `CreateCluster`
//! enables the groups its `metricGroups` names ("If you omit this parameter, no metric
//! groups are enabled"), `UpdateCluster` takes "the complete set… This list replaces
//! the current set, so any metric group that you don't include is disabled. To disable
//! all metric groups, specify an empty list… If you omit this parameter, the current
//! setting doesn't change", and `GetCluster` reports "the metric groups that you
//! enabled… If you haven't enabled any metric groups, the list is empty"
//! (`smithy_query(query="doc", shape="CreateClusterInput|UpdateClusterInput|GetClusterOutput",
//! member="metricGroups")`).
//!
//! So what a cluster holds is a SET of group names and not the list a request spelled:
//! `MetricGroupRequestList` is `@uniqueItems`, so no group can be named twice and the
//! order a caller sends them in carries nothing — two requests naming the same groups
//! ask for the same configuration. The set is kept sorted for the same reason a
//! cluster's tags are ([`crate::state::reported_tags`]): every read of one cluster
//! reports the groups in one order.
//!
//! The member a read reports is a list of `MetricGroupResponse` rather than of the
//! request shape, so the two are converted here and nowhere else — the create, the
//! update and the read cannot disagree about which groups a cluster collects or how
//! they are reported. The one constraint on the member that the boundary walk does
//! not reach is refused here too ([`duplicated_metric_group`]).

use std::collections::BTreeSet;

use ::smithy_server_runtime::{Violation, ViolationKind};

use crate::ops::types::{MetricGroupRequest, MetricGroupResponse, ValidationException};

/// The modelled refusal a `metricGroups` that names one group twice is answered
/// with, or `None` for a list that names each group at most once.
///
/// `MetricGroupRequestList` is `@uniqueItems` — "Each metric group can appear only
/// once" (`smithy_query(query="shape", shape="MetricGroupRequestList")`) — so a list
/// naming a group twice is a member outside a modelled constraint, refused before
/// anything is written and worded exactly as the boundary constraint walk words that
/// class ([`crate::service::validation::fields_that_did_not_validate`]). The walk
/// itself does not reach it: it compares the renderings of a list's elements, and
/// this list's elements are structures, which it records none for.
///
/// `member` is the input member the list arrived in ([`crate::state::METRIC_GROUPS_MEMBER`]
/// on every operation that accepts one), so the refusal names what the caller has to
/// fix.
#[must_use]
pub fn duplicated_metric_group(
    member: &str,
    requested: Option<&[MetricGroupRequest]>,
) -> Option<ValidationException> {
    let mut seen = BTreeSet::new();
    let names_one_twice = requested?
        .iter()
        .any(|group| !seen.insert(group.metric_group.as_str()));
    if !names_one_twice {
        return None;
    }
    crate::service::validation::fields_that_did_not_validate(&[Violation {
        path: member.to_string(),
        kind: ViolationKind::UniqueItems,
        // The violated requirement is about the LIST and not about one value of it,
        // and this service echoes no rejected value back to the caller anyway.
        value: None,
        sensitive: false,
    }])
}

/// The metric groups a request's `metricGroups` asks for: one entry per group it
/// names, deduplicated and sorted ([the module docs](self)).
///
/// `None` — the member absent — asks for no group at all, which is what a cluster
/// created without the member collects. An update tells the absent member from the
/// empty one before it reaches here, because there the two mean different things.
#[must_use]
pub fn requested_metric_groups(requested: Option<&[MetricGroupRequest]>) -> BTreeSet<String> {
    requested
        .unwrap_or_default()
        .iter()
        .map(|group| group.metric_group.clone())
        .collect()
}

/// The metric groups a read of one cluster REPORTS: one `MetricGroupResponse` per
/// group the cluster collects, and the EMPTY list for a cluster collecting none.
///
/// The empty list is a value the member carries rather than an absence, as
/// `GetCluster` documents it ("If you haven't enabled any metric groups, the list is
/// empty"): a reader that left the member out would leave a client unable to tell a
/// cluster collecting nothing from a reply that does not report the configuration.
#[must_use]
pub fn reported_metric_groups(enabled: &BTreeSet<String>) -> Vec<MetricGroupResponse> {
    enabled
        .iter()
        .map(|metric_group| MetricGroupResponse {
            metric_group: metric_group.clone(),
        })
        .collect()
}

/// Render a `metricGroups` into one fingerprint part, in group order (the member is a
/// set of groups, so the order a caller sends them in is not part of the request's
/// meaning).
///
/// A member the request OMITS and one it carries empty are different requests — on
/// `UpdateCluster` they ask for different changes — so the absent member renders as
/// the empty string [`crate::state::fingerprint_optional`] reserves for it, and a
/// member that is there renders as a digest, which is never empty.
#[must_use]
pub fn fingerprint_metric_groups(requested: Option<&[MetricGroupRequest]>) -> String {
    match requested {
        None => String::new(),
        Some(groups) => crate::state::fingerprint_sequence(requested_metric_groups(Some(groups))),
    }
}

/// How a [`crate::state::RecordedCall`] keeps the `metricGroups` a create answered:
/// the group names as a JSON array, so a retry of that create answers the
/// configuration the original call reported rather than the one the cluster collects
/// by the time the retry arrives.
#[must_use]
pub fn encode_metric_groups(enabled: &BTreeSet<String>) -> String {
    ::serde_json::Value::Array(
        enabled
            .iter()
            .map(|group| ::serde_json::Value::String(group.clone()))
            .collect(),
    )
    .to_string()
}

/// Read back the metric groups [`encode_metric_groups`] wrote, or `None` for a string
/// it did not write.
#[must_use]
pub fn decode_metric_groups(recorded: &str) -> Option<BTreeSet<String>> {
    let encoded: ::serde_json::Value = ::serde_json::from_str(recorded).ok()?;
    let ::serde_json::Value::Array(groups) = encoded else {
        return None;
    };
    groups
        .iter()
        .map(|group| group.as_str().map(str::to_string))
        .collect()
}

#[cfg(test)]
mod tests {
    //! The conversions between the request shape, the stored set and the reported
    //! list. What reaches the wire, for which request, is pinned over the router in
    //! `tests/all/cluster_metric_groups.rs`.

    use super::*;

    /// A request naming `groups`.
    fn requested(groups: &[&str]) -> Vec<MetricGroupRequest> {
        groups
            .iter()
            .map(|group| MetricGroupRequest {
                metric_group: (*group).to_string(),
            })
            .collect()
    }

    /// A request's list becomes the set it names, whatever order and whatever
    /// repetition it spells it with, and a cluster collecting nothing reports the
    /// empty list.
    #[test]
    fn a_request_asks_for_the_set_of_groups_it_names() {
        let cases: [(&[&str], &[&str]); 4] = [
            (&[], &[]),
            (&["STATEMENT_STATISTICS"], &["STATEMENT_STATISTICS"]),
            (&["B", "A"], &["A", "B"]),
            (&["A", "A"], &["A"]),
        ];
        for (named, expected) in cases {
            let enabled = requested_metric_groups(Some(&requested(named)));
            let reported: Vec<String> = reported_metric_groups(&enabled)
                .into_iter()
                .map(|group| group.metric_group)
                .collect();
            assert_eq!(
                reported, expected,
                "case {named:?}: expected the groups {expected:?}, got {reported:?}"
            );
        }
        assert!(
            requested_metric_groups(None).is_empty(),
            "case the member absent: expected no group, got {:?}",
            requested_metric_groups(None)
        );
    }

    /// Two requests naming the same groups fingerprint alike however they spell
    /// them; an absent member, an empty list and a different set each fingerprint
    /// differently.
    #[test]
    fn the_fingerprint_is_decided_by_the_set_of_groups() {
        let one = requested(&["A", "B"]);
        let reordered = requested(&["B", "A"]);
        assert_eq!(
            fingerprint_metric_groups(Some(&one)),
            fingerprint_metric_groups(Some(&reordered)),
            "case the same groups in another order: expected one fingerprint"
        );
        let distinct = [
            fingerprint_metric_groups(None),
            fingerprint_metric_groups(Some(&[])),
            fingerprint_metric_groups(Some(&requested(&["A"]))),
            fingerprint_metric_groups(Some(&one)),
        ];
        for (left, right) in [(0, 1), (0, 2), (1, 2), (2, 3)] {
            assert_ne!(
                distinct[left], distinct[right],
                "case {left} against {right}: different requests must fingerprint differently"
            );
        }
    }

    /// The groups a create answered survive the ledger's string encoding.
    #[test]
    fn the_recorded_groups_round_trip() {
        for named in [&[][..], &["STATEMENT_STATISTICS"][..], &["A", "B"][..]] {
            let enabled = requested_metric_groups(Some(&requested(named)));
            let encoded = encode_metric_groups(&enabled);
            assert_eq!(
                decode_metric_groups(&encoded),
                Some(enabled.clone()),
                "case {named:?}: expected {enabled:?} back out of {encoded:?}"
            );
        }
        assert_eq!(
            decode_metric_groups("not an encoding this module wrote"),
            None,
            "case a string this module did not write: expected no groups"
        );
    }

    /// A list naming one group twice is refused, whatever else it names; a list
    /// naming each group once, an empty list and an absent member are not.
    #[test]
    fn a_group_named_twice_is_refused() {
        let member = "metricGroups";
        let refused: [&[&str]; 3] = [&["A", "A"], &["A", "B", "A"], &["B", "A", "A"]];
        for named in refused {
            let refusal = duplicated_metric_group(member, Some(&requested(named)))
                .unwrap_or_else(|| panic!("case {named:?}: must be refused"));
            assert!(
                refusal
                    .field_list
                    .as_deref()
                    .unwrap_or_default()
                    .iter()
                    .any(|field| field.name == member),
                "case {named:?}: the refusal must name {member}, got {:?}",
                refusal.field_list
            );
            assert!(
                !refusal.message.is_empty() && !refusal.reason.is_empty(),
                "case {named:?}: the modelled @required members must be filled, got {refusal:?}"
            );
        }
        let served: [&[&str]; 3] = [&[], &["A"], &["A", "B"]];
        for named in served {
            assert!(
                duplicated_metric_group(member, Some(&requested(named))).is_none(),
                "case {named:?}: a list naming each group once must not be refused"
            );
        }
        assert!(
            duplicated_metric_group(member, None).is_none(),
            "case the member absent: there is no list to refuse"
        );
    }
}
