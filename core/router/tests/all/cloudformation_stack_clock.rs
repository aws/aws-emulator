// SPDX-License-Identifier: CC0-1.0

//! The stack CLOCK a CloudFormation suite drives, for every suite that asserts a SETTLED
//! stack.
//!
//! A stack operation's terminal status becomes due a moment after the call that starts it.
//! Waiting past each due instant makes a case a race against the machine's load, so the due
//! instants are pulled into the past instead and an ordinary read settles the store exactly as
//! the clock would have: what such a case then asserts is the emulator's own settling, not the
//! wall clock. Where a case is about an in-flight state it must NOT use this — it has to hold
//! the store before its due instant, as the suites that assert `*_IN_PROGRESS` do.
//!
//! It lives here, ONCE, because every suite that deploys a stack and then reads it needs
//! exactly this, and because `ComAmazonawsCloudformationCloudformationState::settle` is global
//! over the store: the settling read is signed for one Region and settles the stacks of every
//! Region, so a suite deploying elsewhere calls the same helper.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use bytes::Bytes;
use service_com_amazonaws_cloudformation_cloudformation::state::ComAmazonawsCloudformationCloudformationState;
use smithy_server_runtime::MultiServiceRouter;

/// The Region the settling read below is signed for. Which Region it names decides only
/// WHICH stacks that read reports, never which ones it settles.
const REGION: &str = "us-east-1";

/// How far into the past a due instant is pulled: far enough that the read below finds it
/// due however coarse the host clock is.
const ALREADY_DUE: Duration = Duration::from_secs(1);

/// Make every stack transition this store owes due NOW, then let a read materialise them.
///
/// Call it once per transition the case is waiting on: a failed create owes its own
/// conclusion and then the rollback that conclusion makes due.
pub(crate) async fn settle_the_stacks(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    case: &str,
) {
    {
        let state = engine.state::<ComAmazonawsCloudformationCloudformationState>();
        let mut held = state.lock();
        let due = SystemTime::now() - ALREADY_DUE;
        for stack in &mut held.stacks {
            if let Some(pending) = stack.pending.as_mut() {
                pending.due_at = due;
            }
        }
    }
    // Every operation settles the store first, so any read materialises what is now due.
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("cloudformation.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{REGION}/\
                 cloudformation/aws4_request, SignedHeaders=host, Signature=0000000000000000"
            ),
        )
        .body(Bytes::from_static(b"Action=ListStacks&Version=2010-05-15"))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let body = String::from_utf8_lossy(response.body()).to_string();
    assert_eq!(
        status, 200,
        "case {case:?}: the settling read must succeed, got {status} with body {body}"
    );
}
