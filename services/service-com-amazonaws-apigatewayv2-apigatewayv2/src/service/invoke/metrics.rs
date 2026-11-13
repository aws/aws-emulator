// SPDX-License-Identifier: CC0-1.0

//! WHERE this data plane's own datapoints land: the CloudWatch metric store, and the alarm
//! wake-up each datapoint written there owes.
//!
//! [`super::request_metrics`] decides WHAT one answered request observes and under which
//! dimensions; the WRITE that stores those observations is not this plane's own, because the
//! WEBSOCKET plane publishes into the same namespace: a datapoint of either must land in the
//! same store, at the same resolution, owing the same alarms their evaluation. So the write
//! lives with the rest of what the two planes share ([`crate::service::metrics`]), and this is
//! the handle this plane reaches it by — the reasoning for the alarm wake-up is on
//! [`crate::service::metrics::store_datapoints`].

pub(super) use crate::service::metrics::stored;
