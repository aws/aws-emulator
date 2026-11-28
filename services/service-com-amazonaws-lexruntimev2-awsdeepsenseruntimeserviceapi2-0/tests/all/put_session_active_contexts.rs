// SPDX-License-Identifier: CC0-1.0

//! What a supplied `sessionState` says about a session's ACTIVE CONTEXTS when it
//! carries no `activeContexts` member.
//!
//! `ActiveContext$contextAttributes` states the two cases apart: "If you don't
//! specify a list of contexts, Amazon Lex V2 will use the current list of
//! contexts for the session. If you specify an empty list, all contexts for the
//! session are cleared." An ABSENT member and an EMPTY list are therefore two
//! different instructions, on every path that writes a caller-supplied dialog
//! state — `PutSession` as much as the `RecognizeText` / `RecognizeUtterance`
//! turn — so the contexts a conversation holds, with the time and turn budgets
//! they have already spent, survive a write that does not mention them.
//!
//! Each suite drives the rule over a TABLE of what the conversation held and
//! what the second write carried, and reads the session back through
//! `GetSession` so what is asserted is the stored conversation and not one
//! response.

use serde_json::{json, Value};

use crate::harness::{Emulator, Reply};

/// The labels every session here is addressed by: a `BotIdentifier` of
/// `len 10..10` matching `^[0-9a-zA-Z]+$`, the bot's test alias, and a locale
/// the harness gives a built model.
const BOT_ID: &str = "ABCDEFGHIJ";
const ALIAS_ID: &str = "TSTALIASID";
const LOCALE_ID: &str = "en_US";

/// One context a caller may set, on the full seconds budget
/// `ActiveContextTimeToLive$timeToLiveInSeconds` admits ("between 5 and 86400
/// seconds") and with `turnsToLive` inside its `range 1..20`.
fn context(name: &str, turns_to_live: i64) -> Value {
    json!({
        "name": name,
        "timeToLive": {"timeToLiveInSeconds": 86400, "turnsToLive": turns_to_live},
        "contextAttributes": {"step": "payment"},
    })
}

/// The emulator with a built locale for [`BOT_ID`], and the session path for
/// `session_id` in it.
fn fixture(session_id: &str) -> (Emulator, String) {
    let emulator = Emulator::new();
    emulator.built_locale(BOT_ID, ALIAS_ID, LOCALE_ID);
    let path = format!(
        "/bots/{BOT_ID}/botAliases/{ALIAS_ID}/botLocales/{LOCALE_ID}/sessions/{session_id}"
    );
    (emulator, path)
}

/// Write a session state through `PutSession`, which must be served.
async fn put(emulator: &Emulator, path: &str, case: &str, state: Value) -> Reply {
    let reply = emulator
        .send("POST", path, Some(json!({"sessionState": state})))
        .await;
    assert_eq!(
        reply.status,
        200,
        "case {case:?}: PutSession must succeed, got {} {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
    reply
}

/// The session state `GetSession` reports.
async fn stored_state(emulator: &Emulator, path: &str, case: &str) -> Value {
    let reply = emulator.send("GET", path, None).await;
    assert_eq!(
        reply.status,
        200,
        "case {case:?}: GetSession must find the session, got {} {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
    reply.json()["sessionState"].clone()
}

/// The names of the contexts a reported session state carries, in order.
fn context_names(state: &Value) -> Vec<String> {
    state
        .get("activeContexts")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("a reported session state carries activeContexts: {state}"))
        .iter()
        .map(|context| {
            context
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// One row of the table below: the case, the contexts the first write set, the
/// `activeContexts` member of the second write, and the context names the
/// session must hold afterwards.
type Row<'a> = (&'a str, Option<Value>, Option<Value>, &'a [&'a str]);

/// A second `PutSession` that omits `activeContexts` keeps the contexts the
/// session holds; one that carries a list — the empty list included — sets
/// exactly that list.
#[tokio::test]
async fn put_session_only_touches_active_contexts_when_the_request_carries_them() {
    let rows: [Row; 8] = [
        (
            "no member over one stored context",
            Some(json!([context("checkout", 20)])),
            None,
            &["checkout"],
        ),
        (
            "the empty list over one stored context",
            Some(json!([context("checkout", 20)])),
            Some(json!([])),
            &[],
        ),
        (
            "a list of its own over one stored context",
            Some(json!([context("checkout", 20)])),
            Some(json!([context("upsell", 20)])),
            &["upsell"],
        ),
        (
            "no member over two stored contexts",
            Some(json!([context("checkout", 20), context("upsell", 20)])),
            None,
            &["checkout", "upsell"],
        ),
        (
            "the empty list over two stored contexts",
            Some(json!([context("checkout", 20), context("upsell", 20)])),
            Some(json!([])),
            &[],
        ),
        (
            "a longer list over one stored context",
            Some(json!([context("checkout", 1)])),
            Some(json!([
                context("upsell", 20),
                context("winback", 5),
                context("survey", 1)
            ])),
            &["upsell", "winback", "survey"],
        ),
        (
            "no member over a session the empty list cleared",
            Some(json!([])),
            None,
            &[],
        ),
        (
            "no member over a session that never set one",
            None,
            None,
            &[],
        ),
    ];

    for (index, (case, seeded, second, expected)) in rows.iter().enumerate() {
        let (emulator, path) = fixture(&format!("ctx-{index}"));

        let mut first = json!({"dialogAction": {"type": "Delegate"}});
        if let Some(seeded) = seeded {
            first["activeContexts"] = seeded.clone();
        }
        put(&emulator, &path, case, first).await;
        let after_first = stored_state(&emulator, &path, case).await;
        let seeded_names: Vec<String> = seeded
            .as_ref()
            .map(|list| context_names(&json!({"activeContexts": list})))
            .unwrap_or_default();
        assert_eq!(
            context_names(&after_first),
            seeded_names,
            "case {case:?}: the first write sets the contexts it carried, got {after_first}"
        );

        // The second write carries a dialog action either way, so what differs
        // between the rows is only whether it mentions the contexts.
        let mut again = json!({"dialogAction": {"type": "Delegate"}});
        if let Some(second) = second {
            again["activeContexts"] = second.clone();
        }
        let reported = put(&emulator, &path, case, again)
            .await
            .lex_header("x-amz-lex-session-state")
            .unwrap_or_else(|| panic!("case {case:?}: PutSession reports the session state"));
        let after_second = stored_state(&emulator, &path, case).await;

        for (carrier, state) in [
            ("the PutSession session-state header", &reported),
            ("the GetSession session state", &after_second),
        ] {
            assert_eq!(
                context_names(state),
                expected.iter().map(|n| (*n).to_string()).collect::<Vec<_>>(),
                "case {case:?}: {carrier} must report {expected:?}, got {state}"
            );
        }
    }
}

/// A context kept across a `PutSession` that omits `activeContexts` keeps the
/// budget it has already spent, however it was set, and keeps its attributes:
/// the write uses "the current list of contexts for the session" rather than
/// re-setting one.
#[tokio::test]
async fn kept_active_contexts_keep_the_turn_budget_they_have_spent() {
    // Each row spends one turn of `turnsToLive`, so every value here is at
    // least 2 and at most the published maximum of `range 1..20`.
    for turns_to_live in [2_i64, 3, 20] {
        let case = format!("a context set for {turns_to_live} turn(s)");
        let (emulator, path) = fixture(&format!("turns-{turns_to_live}"));

        // The context is set by a TURN here, not by PutSession: the rule is
        // about what the session holds, not about which call put it there.
        let turn = emulator
            .send(
                "POST",
                &format!("{path}/text"),
                Some(json!({
                    "text": "hello",
                    "sessionState": {
                        "activeContexts": [context("checkout", turns_to_live)],
                    },
                })),
            )
            .await;
        assert_eq!(
            turn.status,
            200,
            "case {case:?}: RecognizeText must succeed, got {} {:?}",
            turn.status,
            String::from_utf8_lossy(&turn.body)
        );

        // One more turn spends one of the context's turns.
        let spent = emulator
            .send("POST", &format!("{path}/text"), Some(json!({"text": "next"})))
            .await;
        assert_eq!(
            spent.status,
            200,
            "case {case:?}: the second RecognizeText must succeed, got {} {:?}",
            spent.status,
            String::from_utf8_lossy(&spent.body)
        );
        let before = stored_state(&emulator, &path, &case).await;
        let remaining = before
            .pointer("/activeContexts/0/timeToLive/turnsToLive")
            .and_then(Value::as_i64);
        assert_eq!(
            remaining,
            Some(turns_to_live - 1),
            "case {case:?}: the context has one turn fewer after a turn, got {before}"
        );

        // A PutSession that only seeds a dialog action leaves the context, its
        // remaining budget and its attributes exactly as they are.
        put(
            &emulator,
            &path,
            &case,
            json!({"dialogAction": {"type": "Delegate"}}),
        )
        .await;
        let after = stored_state(&emulator, &path, &case).await;
        assert_eq!(
            context_names(&after),
            vec!["checkout".to_string()],
            "case {case:?}: the context survives a write that does not mention it, got {after}"
        );
        assert_eq!(
            after
                .pointer("/activeContexts/0/timeToLive/turnsToLive")
                .and_then(Value::as_i64),
            remaining,
            "case {case:?}: the kept context keeps the budget it had spent, got {after}"
        );
        assert_eq!(
            after.pointer("/activeContexts/0/contextAttributes"),
            Some(&json!({"step": "payment"})),
            "case {case:?}: the kept context keeps its attributes, got {after}"
        );
        assert_eq!(
            after
                .pointer("/activeContexts/0/timeToLive/timeToLiveInSeconds")
                .and_then(Value::as_i64),
            Some(86400),
            "case {case:?}: the kept context keeps its seconds budget, got {after}"
        );

        // And a following write that carries the empty list clears it, so the
        // keep above is the absent member and not an inability to clear.
        put(
            &emulator,
            &path,
            &case,
            json!({"dialogAction": {"type": "Delegate"}, "activeContexts": []}),
        )
        .await;
        let cleared = stored_state(&emulator, &path, &case).await;
        assert!(
            context_names(&cleared).is_empty(),
            "case {case:?}: the empty list clears the contexts, got {cleared}"
        );
    }
}
