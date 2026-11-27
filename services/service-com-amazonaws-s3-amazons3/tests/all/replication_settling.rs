// SPDX-License-Identifier: CC0-1.0

//! The ONE way a suite of this crate waits for a version's REPLICATION to settle.
//!
//! "Amazon S3 replicates objects asynchronously" (`PutBucketReplication`), so a source version
//! reports `PENDING` until its replication is done and then one of the two statuses a settled
//! replication reports ([`SETTLED_STATUSES`]). A suite that needs the settled answer waits for
//! that CONDITION — it looks again until the status is terminal, under a deadline far longer
//! than the span this emulator gives AWS's asynchrony — so nothing it asserts depends on how
//! fast the host runs, and no suite waits a fixed span of its own instead.
//!
//! The wait is parameterised on the READ, because the suites reach a version through harnesses
//! of their own (one posts through the in-process router, another through a mounted emulator):
//! what they share is the rule, which is this module.

use std::time::{Duration, Instant};

/// The statuses a SETTLED replication reports — "COMPLETED" when every covering rule copied the
/// version and "FAILED" when one of them could not
/// (`smithy_query(query="shape",shape="com.amazonaws.s3#ReplicationStatus")` publishes both
/// beside the in-flight `PENDING` and the `REPLICA` a copy carries).
pub(crate) const SETTLED_STATUSES: [&str; 2] = ["COMPLETED", "FAILED"];

/// How long a poll waits for a replication to settle before the suite calls it stuck.
///
/// Far longer than the span this emulator gives AWS's asynchronous replication, so a loaded or
/// slow host cannot fail a row: what is asserted is the terminal status, never the time it took
/// to arrive.
pub(crate) const SETTLE_DEADLINE: Duration = Duration::from_secs(60);

/// How often that poll looks again.
pub(crate) const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// Look again through `read` until the replication status it answers is terminal
/// ([`SETTLED_STATUSES`]), and answer which — failing the test, naming `named`, only once
/// [`SETTLE_DEADLINE`] has passed without one.
///
/// `read` answers the `x-amz-replication-status` of the version the caller is waiting on, or
/// `None` when the answer carries none; `named` is what a failure calls the case.
pub(crate) async fn settled_replication_status<Read, Reading>(read: Read, named: &str) -> String
where
    Read: Fn() -> Reading,
    Reading: ::std::future::Future<Output = Option<String>>,
{
    let deadline = Instant::now() + SETTLE_DEADLINE;
    loop {
        let status = read().await.unwrap_or_default();
        if SETTLED_STATUSES.contains(&status.as_str()) {
            return status;
        }
        assert!(
            Instant::now() < deadline,
            "{named}: the replication never reached a terminal status; the last one read was \
             {status:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}
