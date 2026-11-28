// SPDX-License-Identifier: CC0-1.0

//! WHICH input member a reader of ONE cluster OPERATION names in
//! `invalidParameter` when the reference it was handed resolves to no operation
//! this endpoint holds.
//!
//! A cluster-operation ARN references work done on a cluster OF the account and the
//! Region the ARN names, and an endpoint of this emulator serves exactly one of
//! each. So a reference naming another account or another Region refers to a cluster
//! no registry here could ever hold a record for, and the resource the request named
//! and this service cannot find is the CLUSTER; a reference of the account and Region
//! this endpoint does serve names work that could have been recorded here, so there
//! the operation reference itself is what resolved to nothing.
//!
//! WHETHER a record could live here is read off the request by the reader taking the
//! decision — each of the two states it over its own input, before it takes the
//! registry lock — and the member NAME that reading maps to lives here, in the one
//! place this service keeps its wire vocabulary, so `DescribeClusterOperation` and
//! `DescribeClusterOperationV2` cannot blame differently named members for one
//! condition.

use super::*;

/// The member a cluster-operation reader names in `invalidParameter`, given whether
/// this endpoint could hold a record for the reference it was handed:
/// [`CLUSTER_OPERATION_ARN_MEMBER`] where it could, [`CLUSTER_ARN_MEMBER`] where the
/// reference stands for a cluster no record here could belong to (see the module
/// docs).
pub fn missed_cluster_operation_member(records_could_live_here: bool) -> &'static str {
    if records_could_live_here {
        CLUSTER_OPERATION_ARN_MEMBER
    } else {
        CLUSTER_ARN_MEMBER
    }
}
