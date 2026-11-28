// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the tag-enrichment setting of the account's vended-metric enrichment
//! configuration: the `EnableTagEnrichment` member of `StartOTelEnrichment`,
//! `UpdateOTelEnrichment` and the `GetOTelEnrichment` that reports what they stored.
//!
//! The rule under test: **the setting is part of the configuration a start stores, defaulting to
//! `true` when the start omits it; a start on an account where enrichment is already running
//! leaves it alone; an update replaces it only when the request carries it, and otherwise keeps
//! the stored value; and a report omits it while enrichment is stopped.**
//! (`smithy_query(query="doc",shape="StartOTelEnrichmentInput",member="EnableTagEnrichment")`:
//! "If you omit this parameter, the default is `true`";
//! `shape="UpdateOTelEnrichmentInput"`: "If you omit this parameter, the stored value is not
//! changed. The operation also replaces the stored filters, so to change only this setting,
//! specify the current `IncludeFilters` and `ExcludeFilters` in the same request";
//! `shape="GetOTelEnrichmentOutput"`: "This parameter is omitted when enrichment is stopped".)
//!
//! Rows differ along what the rule generalises over: which value the request asks for (`true`,
//! `false`, or the member left out entirely), whether the request also carries filters, and the
//! order the start/update/stop calls arrive in.

use crate::harness::{build_router, call, error_code, ok};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The member this suite is about, as the wire spells it.
const MEMBER: &str = "EnableTagEnrichment";

/// What a start stores for the setting when the request leaves it out: "If you omit this
/// parameter, the default is `true`." (`StartOTelEnrichmentInput$EnableTagEnrichment`)
const DEFAULT_WHEN_OMITTED: bool = true;

/// The setting a response reports, or `None` when the response omits it.
fn reported(value: &Value) -> Option<bool> {
    value.get(MEMBER).and_then(Value::as_bool)
}

/// A request body that asks for `asked` (or omits the member when it is `None`), optionally
/// carrying filters as well, so the setting is exercised both on its own and beside the pair the
/// same operations replace.
fn request(asked: Option<bool>, with_filters: bool) -> Value {
    let mut body = json!({});
    if let Some(asked) = asked {
        body[MEMBER] = json!(asked);
    }
    if with_filters {
        body["IncludeFilters"] = json!([{"Namespace": "AWS/EC2", "MetricNames": ["CPUUtilization"]}]);
        body["ExcludeFilters"] = json!([{"Namespace": "AWS/Lambda"}]);
    }
    body
}

/// Every form a request can take for this member, with the value each one asks for.
const ASKED: [(&str, Option<bool>); 3] = [
    ("the member left out", None),
    ("tag labels on", Some(true)),
    ("tag labels off", Some(false)),
];

/// What `GetOTelEnrichment` reports for the account.
async fn get(router: &MultiServiceRouter) -> Value {
    ok(router, "GetOTelEnrichment", &json!({})).await
}

#[tokio::test]
async fn a_start_stores_the_setting_it_was_given_and_reports_it_back() {
    for (asked_case, asked) in ASKED {
        for with_filters in [false, true] {
            let case = format!("{asked_case}, filters: {with_filters}");
            let expected = asked.unwrap_or(DEFAULT_WHEN_OMITTED);
            let router = build_router();

            let started = ok(
                &router,
                "StartOTelEnrichment",
                &request(asked, with_filters),
            )
            .await;
            assert_eq!(
                reported(&started),
                Some(expected),
                "case {case:?}: StartOTelEnrichment must report the {MEMBER} it stored, \
                 expected {expected}, got {started}"
            );

            let got = get(&router).await;
            assert_eq!(
                reported(&got),
                Some(expected),
                "case {case:?}: GetOTelEnrichment must report the stored {MEMBER}, expected \
                 {expected}, got {got}"
            );
            // The report is a read: asking twice is the same answer.
            let again = get(&router).await;
            assert_eq!(
                reported(&again),
                Some(expected),
                "case {case:?}: a second GetOTelEnrichment must report the same {MEMBER}, got \
                 {again}"
            );
        }
    }
}

#[tokio::test]
async fn a_repeat_start_leaves_the_stored_setting_alone() {
    // "Calling StartOTelEnrichment for an account where enrichment is already running has no
    // effect and does not modify the filters or the tag enrichment setting that are applied."
    // Every (first, repeat) pair of request forms is a row, so a repeat that asks for the other
    // value and one that asks for nothing are both covered.
    for (first_case, first) in ASKED {
        for (repeat_case, repeat) in ASKED {
            let case = format!("first {first_case}, repeat {repeat_case}");
            let expected = first.unwrap_or(DEFAULT_WHEN_OMITTED);
            let router = build_router();

            ok(&router, "StartOTelEnrichment", &request(first, true)).await;
            let repeated = ok(&router, "StartOTelEnrichment", &request(repeat, false)).await;
            assert_eq!(
                reported(&repeated),
                Some(expected),
                "case {case:?}: a repeat start must report the {MEMBER} the first one stored, \
                 expected {expected}, got {repeated}"
            );

            let got = get(&router).await;
            assert_eq!(
                reported(&got),
                Some(expected),
                "case {case:?}: a repeat start must not modify the stored {MEMBER}, expected \
                 {expected}, got {got}"
            );
        }
    }
}

#[tokio::test]
async fn an_update_replaces_the_setting_only_when_the_request_carries_it() {
    // "If you omit this parameter, the stored value is not changed."
    // (`UpdateOTelEnrichmentInput$EnableTagEnrichment`) — unlike the filters, which the same
    // request replaces as a pair whether or not it carries them.
    for (started_case, started) in ASKED {
        for (update_case, update) in ASKED {
            for with_filters in [false, true] {
                let case = format!(
                    "started {started_case}, update {update_case}, filters: {with_filters}"
                );
                let stored = started.unwrap_or(DEFAULT_WHEN_OMITTED);
                let expected = update.unwrap_or(stored);
                let router = build_router();

                ok(&router, "StartOTelEnrichment", &request(started, true)).await;
                let updated = ok(
                    &router,
                    "UpdateOTelEnrichment",
                    &request(update, with_filters),
                )
                .await;
                assert_eq!(
                    reported(&updated),
                    Some(expected),
                    "case {case:?}: an update must report the {MEMBER} in force after it, \
                     expected {expected}, got {updated}"
                );
                // The filters are replaced as a pair, so an update that carries none clears
                // them whatever it asked for the setting.
                for member in ["IncludeFilters", "ExcludeFilters"] {
                    assert_eq!(
                        updated.get(member).is_some(),
                        with_filters,
                        "case {case:?}: an update must report {member} only when it carried \
                         them, got {updated}"
                    );
                }

                let got = get(&router).await;
                assert_eq!(
                    reported(&got),
                    Some(expected),
                    "case {case:?}: GetOTelEnrichment must report what the update left stored \
                     for {MEMBER}, expected {expected}, got {got}"
                );
            }
        }
    }
}

#[tokio::test]
async fn a_stopped_account_omits_the_setting_and_a_later_start_decides_it_again() {
    // "This parameter is omitted when enrichment is stopped."
    // (`GetOTelEnrichmentOutput$EnableTagEnrichment`) Rows pair each form the stopped
    // configuration was started with against each form the next start asks for, so the setting
    // a restart stores is shown to come from the restart rather than from what was stopped.
    for (stopped_case, stopped) in ASKED {
        for (restart_case, restart) in ASKED {
            let case = format!("stopped {stopped_case}, restarted {restart_case}");
            let router = build_router();

            ok(&router, "StartOTelEnrichment", &request(stopped, true)).await;
            ok(&router, "StopOTelEnrichment", &json!({})).await;

            let got = get(&router).await;
            assert_eq!(
                reported(&got),
                None,
                "case {case:?}: a stopped account must omit {MEMBER}, got {got}"
            );
            // An update needs enrichment running, so while it is stopped nothing can change the
            // setting through that door either.
            let (status, body) = call(
                &router,
                "UpdateOTelEnrichment",
                &request(restart, false),
            )
            .await;
            assert_eq!(
                status, 404,
                "case {case:?}: expected HTTP 404 updating a stopped account, got {status} \
                 with body {body}"
            );
            assert_eq!(
                error_code(&body),
                "ResourceNotFoundException",
                "case {case:?}: expected ResourceNotFoundException, got {body}"
            );

            let expected = restart.unwrap_or(DEFAULT_WHEN_OMITTED);
            let restarted = ok(&router, "StartOTelEnrichment", &request(restart, false)).await;
            assert_eq!(
                reported(&restarted),
                Some(expected),
                "case {case:?}: a start after a stop must store the {MEMBER} it was given, \
                 expected {expected}, got {restarted}"
            );
            let got = get(&router).await;
            assert_eq!(
                reported(&got),
                Some(expected),
                "case {case:?}: GetOTelEnrichment must report the restarted {MEMBER}, expected \
                 {expected}, got {got}"
            );
        }
    }
}
