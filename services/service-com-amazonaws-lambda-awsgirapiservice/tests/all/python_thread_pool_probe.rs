// SPDX-License-Identifier: CC0-1.0

//! End-to-end test for the WORK a `python3.x` function's thread pool does: `CreateFunction`
//! then `Invoke` (`POST /2015-03-31/functions/{FunctionName}/invocations`) over the router.
//!
//! The rule under test: the callables a thread pool is handed do the handler's REAL work, and
//! each future carries what its own callable computed — so a pool mapped over work that reaches
//! the rest of the standard library (here: compressing a payload and reading it back) answers a
//! result per item, in the order the items were handed over.
//!
//! The sibling suites read the pool's own surface ([`crate::python_thread_pool_executor`]) and
//! the compression modules' own ([`crate::python_compression_streams`]); this one drives one
//! invocation in which the pool's items are that work, which is what a handler fans out for.
//! The rows differ along which module each item used and how large the payload it carried was.

use std::sync::Arc;

use serde_json::{json, Value};

use crate::invoke_request_id::{is_host_bound, one_invocation_stream_at_a_time};
use crate::python_frame_source_lines::{create_function, invoke, poll_until_active, router_on};

/// The payload sizes the probe maps its pool over, which is what each row's item compresses.
/// Each is a count of repetitions of a short piece of text, so every row carries a payload of
/// its own length.
const SIZES: &[u64] = &[0, 1, 64, 4096];

/// The whole rule, driven END TO END as one test: one real `CreateFunction` and one real
/// `Invoke` over the router.
#[tokio::test]
async fn a_python_thread_pool_does_the_handlers_own_work() {
    let _stream = one_invocation_stream_at_a_time().await;
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let probe = "probe-thread-pool-work";
    create_function(&router, probe, &[("index.py", PROBE_SOURCE)], "index.handler").await;
    poll_until_active(&router, probe).await;
    let answer = invoke(
        &router,
        probe,
        &serde_json::to_vec(&json!({"sizes": SIZES})).expect("the event serializes"),
    )
    .await;
    if is_host_bound(answer.status, &answer.text()) {
        // The run was ended by a bound this whole test binary shares rather than by anything
        // this case asked for, so it says nothing about the rule.
        return;
    }
    assert_eq!(
        answer.status,
        200,
        "expected the invocation to be answered 200, got {} {}",
        answer.status,
        answer.text()
    );
    let document = answer.json();
    assert_eq!(
        document.get("failed"),
        None,
        "expected the handler to answer rather than fail, got {document}"
    );
    let items = document
        .get("items")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("expected the answer to carry items, got {document}"))
        .clone();
    assert_eq!(
        items.len(),
        SIZES.len() * 2,
        "expected one item per payload and module, got {document}"
    );
    for item in &items {
        let case = item
            .get("case")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("expected every item to name its case, got {document}"));
        // Each item's own work: the payload it carried is the one it read back, and the stream
        // it wrote is the work the item really did.
        assert_eq!(
            item.get("restored"),
            item.get("payload"),
            "case {case:?}: expected the item to have read its own payload back, got {item}"
        );
        assert!(
            item.get("compressed")
                .and_then(Value::as_u64)
                .is_some_and(|written| written > 0),
            "case {case:?}: expected the item to have written a stream, got {item}"
        );
    }
}

/// The probe: a handler that fans a compression round trip out across a pool.
const PROBE_SOURCE: &str = r#"
import bz2
import lzma
from concurrent.futures import ThreadPoolExecutor

# The modules an item of the pool may carry its payload through.
MODULES = (("bz2", bz2), ("lzma", lzma))


def _round_trip(asked):
    """One work item: compress the payload it was given and read it back."""
    label, module, size = asked
    payload = b"lambda compression " * size
    squeezed = module.compress(payload)
    return {
        "case": label + " / " + str(size),
        "payload": payload.hex(),
        "restored": module.decompress(squeezed).hex(),
        "compressed": len(squeezed),
    }


def handler(event, context):
    try:
        work = [
            (label, module, size)
            for size in event["sizes"]
            for label, module in MODULES
        ]
        with ThreadPoolExecutor(max_workers=2) as pool:
            return {"items": list(pool.map(_round_trip, work))}
    except BaseException as error:
        return {"failed": type(error).__name__ + ": " + str(error)}
"#;
