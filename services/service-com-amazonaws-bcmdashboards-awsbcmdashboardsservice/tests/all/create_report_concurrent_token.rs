// SPDX-License-Identifier: CC0-1.0

//! What a fleet of CONCURRENT `CreateScheduledReport` calls sharing one `clientToken` is
//! answered, end to end over the wire.
//!
//! RULE: the `@idempotencyToken` keys at most ONE create, and a duplicate of it is never
//! served that create's success: of N calls presenting one token, exactly one is served
//! (HTTP 200 and one minted scheduled-report ARN) and every other is refused with the
//! modelled `ConflictException` (HTTP 409) — the creation-in-progress text while the
//! winner has not stored its report yet, and the already-exists text naming the stored ARN
//! once it has — so the account ends up holding exactly one report of that name.
//!
//! The rows differ along what the rule generalises over: how many calls race for the token
//! (two, three, five), and whether they are released from one barrier or in waves. The
//! second test is the sequential edge of the same rule, which the fleet cannot show on its
//! own: the winner's own caller presenting the token again afterwards is refused too.

use std::collections::BTreeSet;

use serde_json::Value;

use super::create_report_client_token::{first_request, stored_arns};
use super::create_report_conflict_message::{already_exists_message, IN_PROGRESS_MESSAGE};
use super::dashboard_lifecycle::{call, error_code, router};
use super::scheduled_report_lifecycle::source_dashboard;

/// The ARN pattern a minted scheduled-report ARN carries
/// (`smithy_query(query="shape",shape="ScheduledReportArn")`:
/// `^arn:aws[-a-z0-9]*:bcm-dashboards::[0-9]{12}:scheduled-report/(\*|[-a-z0-9]+)$`), as
/// much of it as this suite needs to tell a minted ARN from anything else.
const ARN_PREFIX: &str = "arn:aws:bcm-dashboards::";
const ARN_RESOURCE: &str = ":scheduled-report/";

/// One outcome of a racing call: its HTTP status and body.
type Outcome = (u16, Value);

/// Fire `racers` calls of `request` concurrently through one router and answer what each of
/// them was told, in completion order.
async fn race(
    router: &smithy_server_runtime::MultiServiceRouter,
    request: &Value,
    racers: usize,
) -> Vec<Outcome> {
    let mut handles = Vec::with_capacity(racers);
    for _ in 0..racers {
        let router = router.clone();
        let request = request.clone();
        handles.push(tokio::spawn(async move {
            call(&router, "CreateScheduledReport", &request).await
        }));
    }
    let mut outcomes = Vec::with_capacity(racers);
    for handle in handles {
        outcomes.push(handle.await.expect("a racing call does not panic"));
    }
    outcomes
}

/// Assert the rule over `outcomes`: exactly one success carrying a minted ARN, and every
/// other call refused with one of the two conflicts — the already-exists one naming that
/// ARN.
fn assert_one_winner(case: &str, outcomes: &[Outcome]) -> String {
    let winners: Vec<&Value> = outcomes
        .iter()
        .filter(|(status, _)| *status == 200)
        .map(|(_, body)| body)
        .collect();
    assert_eq!(
        winners.len(),
        1,
        "case {case:?}: exactly one call presenting the token is served, got {outcomes:?}"
    );
    let arn = winners[0]["arn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the winner answers an arn, got {outcomes:?}"))
        .to_string();
    assert!(
        arn.starts_with(ARN_PREFIX) && arn.contains(ARN_RESOURCE),
        "case {case:?}: the winner's arn is a minted scheduled-report arn, got {arn:?}"
    );

    let allowed = [IN_PROGRESS_MESSAGE.to_string(), already_exists_message(&arn)];
    for (status, body) in outcomes.iter().filter(|(status, _)| *status != 200) {
        assert_eq!(
            *status, 409,
            "case {case:?}: a loser is refused with 409, got {status} with {body}"
        );
        assert!(
            error_code(body).ends_with("ConflictException"),
            "case {case:?}: a loser is refused with ConflictException, got {body}"
        );
        let message = body["message"].as_str().unwrap_or_default();
        assert!(
            allowed.contains(&message.to_string()),
            "case {case:?}: expected one of {allowed:?}, got {message:?}"
        );
        assert_eq!(
            body["arn"],
            Value::Null,
            "case {case:?}: a refused create answers no arn, got {body}"
        );
    }
    arn
}

/// RULE: of N concurrent calls sharing one `clientToken` exactly one creates, and the
/// account holds exactly that one report.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_calls_sharing_one_client_token_leave_one_winner() {
    // The rows differ in how many callers race for the token and in how they are released:
    // one barrier for all of them, or in waves, so a token that only suppresses a
    // duplicate arriving after the first answer is caught either way.
    for (case, racers, waves) in [
        ("two callers, one wave", 2usize, 1usize),
        ("three callers, one wave", 3, 1),
        ("five callers, one wave", 5, 1),
        ("five callers, released in two waves", 5, 2),
    ] {
        let router = router();
        let (dashboard_arn, widget_id) = source_dashboard(&router, "ConcurrentTokenSource").await;
        let request = first_request(&dashboard_arn, &widget_id);

        let mut outcomes: Vec<Outcome> = Vec::new();
        for wave in 0..waves {
            // The last wave carries whatever is left over, so every row fires `racers`
            // calls in total.
            let in_this_wave = if wave + 1 == waves {
                racers - outcomes.len()
            } else {
                racers / waves
            };
            outcomes.extend(race(&router, &request, in_this_wave).await);
        }

        let arn = assert_one_winner(case, &outcomes);
        let stored = stored_arns(&router).await;
        assert_eq!(
            stored,
            vec![arn.clone()],
            "case {case:?}: the account holds exactly the report the winner created, got {stored:?}"
        );
        let distinct: BTreeSet<String> = outcomes
            .iter()
            .filter(|(status, _)| *status == 200)
            .filter_map(|(_, body)| body["arn"].as_str().map(str::to_string))
            .collect();
        assert_eq!(
            distinct,
            BTreeSet::from([arn]),
            "case {case:?}: no second ARN was handed out, got {distinct:?}"
        );
    }
}

/// The sequential edge of the same rule: once the winner has stored its report, its own
/// caller presenting the token again — the identical request, which an SDK retry resends —
/// is refused in the name of that report rather than served its success a second time.
#[tokio::test]
async fn the_winner_s_own_retry_is_refused_in_the_name_of_the_report_it_created() {
    let case = "the winner's identical retry";
    let router = router();
    let (dashboard_arn, widget_id) = source_dashboard(&router, "ConcurrentTokenRetry").await;
    let request = first_request(&dashboard_arn, &widget_id);

    let (status, created) = call(&router, "CreateScheduledReport", &request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the first call is served, got {status} with {created}"
    );
    let arn = created["arn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the create answers an arn, got {created}"))
        .to_string();

    let (status, refused) = call(&router, "CreateScheduledReport", &request).await;
    let expected = already_exists_message(&arn);
    assert_eq!(
        (status, refused["message"].as_str()),
        (409, Some(expected.as_str())),
        "case {case:?}: expected 409 {expected:?}, got {status} with {refused}"
    );
    let stored = stored_arns(&router).await;
    assert_eq!(
        stored,
        vec![arn],
        "case {case:?}: the retry created no second report, got {stored:?}"
    );
}
