// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for `ListPipelineExecutions`.
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.<Op>`,
//! `awsJson1_1`) through the same `MultiServiceRouter` the production router uses, so
//! what these tests exercise is what a real SDK client gets.
//!
//! The tests assert RULES over TABLES of inputs rather than one recorded call: the
//! listing reports one summary per STORED execution OF THE PIPELINE IT NAMES, projecting
//! what the start stored and what the clock has since made of the run; the creation-time
//! bounds select which runs it reports and `SortBy`/`SortOrder` order them, a value
//! outside either modelled enum being refused; `MaxResults`/`NextToken` walk the result
//! set exactly once whatever the page size; and a pipeline the account does not hold —
//! never created or deleted since — is refused with the modelled `ResourceNotFound`
//! rather than answered with an empty list. The status a summary reports is the one the
//! run's own steps and the clock gave it: a run that reaches a fail step ends `Failed`
//! with that step's message, and one whose reached steps were all performed succeeds.

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_sagemaker_sagemaker::state;

use super::inference_deployment_lifecycle::{
    error_code, message, str_field, Emulator, ACCOUNT, CREATE_SPACING,
};
use super::pipeline_lifecycle::{create_body, definition, step, token};

/// How long a listing may be polled for a run's clock-owed move before the test gives up.
/// Generous on purpose: the test waits for the CONDITION (the run left `Executing`), not
/// for a span, so a loaded host only polls more often.
const SETTLE_DEADLINE: Duration = Duration::from_secs(30);

/// How long to wait between two polls of the same listing.
const POLL_SPACING: Duration = Duration::from_millis(20);

/// `PipelineExecutionStatus`'s non-terminal member a start reports a run in
/// (`smithy_query(query="shape",shape="PipelineExecutionStatus")` answers
/// `CompletedWithFailures | Executing | Failed | Stopped | Stopping | Succeeded`).
const EXECUTING: &str = "Executing";

/// The summaries of a listing response, in the order it reported them.
fn summaries(response: &Value) -> Vec<Value> {
    response
        .get("PipelineExecutionSummaries")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("expected a PipelineExecutionSummaries list in {response}"))
        .clone()
}

/// The `PipelineExecutionArn`s of a listing response, in the order it reported them.
fn listed_arns(response: &Value) -> Vec<String> {
    summaries(response)
        .iter()
        .map(|summary| str_field(summary, "PipelineExecutionArn"))
        .collect()
}

/// Create a one-step pipeline under `name`, answering its `PipelineArn`.
async fn pipeline(emulator: &Emulator, name: &str) -> String {
    let created = emulator
        .ok(
            "CreatePipeline",
            &create_body(
                name,
                &definition(json!([step("Only", &[])])),
                &token(name),
            ),
        )
        .await;
    str_field(&created, "PipelineArn")
}

/// Start one execution of `name`, carrying `extra`'s optional members, and answer its
/// `PipelineExecutionArn`.
async fn start(emulator: &Emulator, name: &str, seed: &str, extra: &Value) -> String {
    let mut body = json!({ "PipelineName": name, "ClientRequestToken": token(seed) });
    if let Some(members) = extra.as_object() {
        for (member, value) in members {
            body[member.as_str()] = value.clone();
        }
    }
    let started = emulator.ok("StartPipelineExecution", &body).await;
    // The starts are spaced, so `StartTime` orders the runs by when they began.
    tokio::time::sleep(CREATE_SPACING).await;
    str_field(&started, "PipelineExecutionArn")
}

/// The listing's summary for the run `arn`, or `None` when it reports no such run.
async fn summary_of(emulator: &Emulator, pipeline_name: &str, arn: &str) -> Option<Value> {
    let listing = emulator
        .ok(
            "ListPipelineExecutions",
            &json!({ "PipelineName": pipeline_name }),
        )
        .await;
    summaries(&listing)
        .into_iter()
        .find(|summary| str_field(summary, "PipelineExecutionArn") == arn)
}

/// RULE: the listing reports ONE summary per stored execution of the pipeline it names —
/// projecting the ARN the start minted, the instant it began, the run's status and
/// failure reason, and the display name and description the start supplied — and reports
/// no run of any other pipeline, whether the request named the pipeline by name or by ARN.
#[tokio::test]
async fn a_listing_projects_every_stored_execution_of_the_pipeline_it_names() {
    let emulator = Emulator::new();
    let pipeline_arn = pipeline(&emulator, "listed").await;
    // A second pipeline with a run of its own, so a listing that reported the whole store
    // could not be mistaken for one scoped to the pipeline it names.
    pipeline(&emulator, "other").await;
    let other_run = start(&emulator, "other", "other-run", &json!({})).await;

    // Rows differ along which optional members the start carried.
    let cases: Vec<(&str, Value)> = vec![
        ("no-optional-members", json!({})),
        (
            "display-name",
            json!({ "PipelineExecutionDisplayName": "nightly-run" }),
        ),
        (
            "description",
            json!({ "PipelineExecutionDescription": "the nightly run" }),
        ),
        (
            "display-name-and-description",
            json!({
                "PipelineExecutionDisplayName": "both-members",
                "PipelineExecutionDescription": "a run carrying both",
            }),
        ),
    ];

    let mut runs = Vec::new();
    for (case, extra) in &cases {
        runs.push(start(&emulator, "listed", case, extra).await);
    }

    // Either form of `PipelineNameOrArn` denotes the same pipeline, so both report the
    // same runs.
    for (addressing, pipeline_name) in [("by-name", "listed"), ("by-arn", pipeline_arn.as_str())] {
        let listing = emulator
            .ok(
                "ListPipelineExecutions",
                &json!({ "PipelineName": pipeline_name }),
            )
            .await;
        let reported = summaries(&listing);
        assert_eq!(
            reported.len(),
            cases.len(),
            "case {addressing}: one summary per stored execution of the pipeline, got \
             {listing}"
        );
        assert!(
            !listed_arns(&listing).contains(&other_run),
            "case {addressing}: another pipeline's run is not reported, got {listing}"
        );

        for ((case, extra), arn) in cases.iter().zip(&runs) {
            let summary = reported
                .iter()
                .find(|summary| str_field(summary, "PipelineExecutionArn") == *arn)
                .unwrap_or_else(|| panic!("case {addressing}/{case}: no summary in {listing}"));
            assert_eq!(
                summary.get("PipelineExecutionDisplayName"),
                extra.get("PipelineExecutionDisplayName"),
                "case {addressing}/{case}: the display name the start stored, got {summary}"
            );
            assert_eq!(
                summary.get("PipelineExecutionDescription"),
                extra.get("PipelineExecutionDescription"),
                "case {addressing}/{case}: the description the start stored, got {summary}"
            );
            // The summary projects the SAME record the reader replays, so the two cannot
            // disagree about one run.
            let described = emulator
                .ok(
                    "DescribePipelineExecution",
                    &json!({ "PipelineExecutionArn": arn }),
                )
                .await;
            assert_eq!(
                (
                    summary.get("StartTime"),
                    summary.get("PipelineExecutionStatus"),
                    summary.get("PipelineExecutionFailureReason"),
                ),
                (
                    described.get("CreationTime"),
                    described.get("PipelineExecutionStatus"),
                    described.get("FailureReason"),
                ),
                "case {addressing}/{case}: the summary and the reader report one run, got \
                 {summary} vs {described}"
            );
            assert!(
                arn.starts_with(&format!("{pipeline_arn}/execution/")),
                "case {addressing}/{case}: the run's ARN extends the pipeline's, got {arn}"
            );
        }
    }

    // A pipeline the account holds that has never run lists the EMPTY list rather than an
    // absent member or a refusal.
    pipeline(&emulator, "never-ran").await;
    let listing = emulator
        .ok(
            "ListPipelineExecutions",
            &json!({ "PipelineName": "never-ran" }),
        )
        .await;
    assert_eq!(
        listing.get("PipelineExecutionSummaries"),
        Some(&json!([])),
        "case never-ran: the listing is empty, not absent, got {listing}"
    );
    assert!(
        listing.get("NextToken").is_none(),
        "case never-ran: an exhausted listing carries no token, got {listing}"
    );
}

/// RULE: the terminal status a run reports is decided by the steps it PERFORMED: a run
/// that reaches a fail step ends `Failed`, reporting the service's own step-failure
/// sentence while the fail step's declared message stays on that STEP; a run whose
/// reached steps were all performed and succeeded ends `Succeeded`; and a fail step in
/// the branch a condition step sent control away from is never reached, so it fails
/// nothing.
#[tokio::test]
async fn the_steps_a_run_performed_decide_the_status_the_listing_reports() {
    let emulator = Emulator::new();

    /// A `Fail` step named `name` declaring `error_message`, as SageMaker Pipelines
    /// serialises one (`Type`/`ErrorMessage`).
    fn fail_step(name: &str, error_message: &str) -> Value {
        json!({
            "Name": name,
            "Type": "Fail",
            "Arguments": { "ErrorMessage": error_message },
        })
    }

    /// A `Condition` step comparing the pipeline parameter `Outcome` with `"go"` — the
    /// `Equals`/`LeftValue`/`RightValue` condition keys SageMaker Pipelines publishes —
    /// and branching to `if_steps` or `else_steps`.
    fn gate(name: &str, if_steps: Value, else_steps: Value) -> Value {
        json!({
            "Name": name,
            "Type": "Condition",
            "Arguments": {
                "Conditions": [{
                    "Type": "Equals",
                    "LeftValue": { "Get": "Parameters.Outcome" },
                    "RightValue": "go",
                }],
                "IfSteps": if_steps,
                "ElseSteps": else_steps,
            },
        })
    }

    /// A definition declaring the `Outcome` parameter and `steps`.
    fn declaring(steps: Value) -> String {
        json!({
            "Version": "2020-12-01",
            "Metadata": {},
            "Parameters": [{ "Name": "Outcome", "Type": "String", "DefaultValue": "stop" }],
            "Steps": steps,
        })
        .to_string()
    }

    // Rows differ along what the run reaches: a fail step on its own, a fail step the
    // condition step hands control TO, the same definition with control handed to the
    // other branch, and a definition of condition steps alone.
    let cases: Vec<(&str, Value, &str, &str, Option<&str>)> = vec![
        (
            "a-fail-step-on-its-own",
            json!([fail_step("Halt", "the data did not validate")]),
            "stop",
            "Failed",
            Some("the data did not validate"),
        ),
        (
            "the-condition-reaches-the-fail-step",
            json!([gate("Check", json!([fail_step("Halt", "the metric regressed")]), json!([]))]),
            "go",
            "Failed",
            Some("the metric regressed"),
        ),
        (
            "the-condition-sends-control-past-the-fail-step",
            json!([gate("Check", json!([fail_step("Halt", "the metric regressed")]), json!([]))]),
            "stop",
            "Succeeded",
            None,
        ),
        (
            "condition-steps-alone",
            json!([
                gate("Outer", json!([]), json!([gate("Inner", json!([]), json!([]))])),
            ]),
            "stop",
            "Succeeded",
            None,
        ),
    ];

    for (case, steps, outcome, expected_status, expected_message) in cases {
        emulator
            .ok(
                "CreatePipeline",
                &create_body(case, &declaring(steps), &token(case)),
            )
            .await;
        let started = emulator
            .ok(
                "StartPipelineExecution",
                &json!({
                    "PipelineName": case,
                    "ClientRequestToken": token(case),
                    "PipelineParameters": [{ "Name": "Outcome", "Value": outcome }],
                }),
            )
            .await;
        let arn = str_field(&started, "PipelineExecutionArn");

        // Wait for the CONDITION — the run left `Executing` — rather than for a span.
        let deadline = Instant::now() + SETTLE_DEADLINE;
        let settled = loop {
            let summary = summary_of(&emulator, case, &arn)
                .await
                .unwrap_or_else(|| panic!("case {case}: the listing reports the run"));
            if str_field(&summary, "PipelineExecutionStatus") != EXECUTING {
                break summary;
            }
            assert!(
                Instant::now() < deadline,
                "case {case}: the run never left {EXECUTING}, last summary {summary}"
            );
            tokio::time::sleep(POLL_SPACING).await;
        };

        assert_eq!(
            str_field(&settled, "PipelineExecutionStatus"),
            expected_status,
            "case {case}: the steps the run performed decide its status, got {settled}"
        );
        // A run that failed because a step of it failed reports the service's one sentence
        // for that class; the step's own message is on the step execution below.
        assert_eq!(
            settled
                .get("PipelineExecutionFailureReason")
                .and_then(Value::as_str),
            expected_message.map(|_| state::PIPELINE_EXECUTION_STEP_FAILURE),
            "case {case}: the reason a run that a step failed reports, got {settled}"
        );
        if let Some(message) = expected_message {
            // The recorded wording, pinned independently of the constant above.
            assert_eq!(
                settled
                    .get("PipelineExecutionFailureReason")
                    .and_then(Value::as_str),
                Some("Step failure: One or multiple steps failed."),
                "case {case}: the sentence AWS words this failure class in, got {settled}"
            );
            let steps = emulator
                .ok(
                    "ListPipelineExecutionSteps",
                    &json!({ "PipelineExecutionArn": &arn }),
                )
                .await;
            let halt = steps
                .get("PipelineExecutionSteps")
                .and_then(Value::as_array)
                .unwrap_or_else(|| panic!("case {case}: a step list in {steps}"))
                .iter()
                .find(|step| str_field(step, "StepName") == "Halt")
                .unwrap_or_else(|| panic!("case {case}: the fail step's execution in {steps}"))
                .clone();
            assert_eq!(
                (
                    halt.get("StepStatus").and_then(Value::as_str),
                    halt.get("FailureReason").and_then(Value::as_str),
                ),
                (Some("Failed"), Some(message)),
                "case {case}: the fail step reports its own declared message, got {halt}"
            );
            // "A message that you define and then is processed and rendered by the Fail
            // step when the error occurs" (`FailStepMetadata$ErrorMessage`).
            assert_eq!(
                halt.get("Metadata")
                    .and_then(|metadata| metadata.get("Fail"))
                    .and_then(|fail| fail.get("ErrorMessage"))
                    .and_then(Value::as_str),
                Some(message),
                "case {case}: the fail step's metadata carries the message it declared, \
                 got {halt}"
            );
        }
    }
}

/// RULE: the status and the failure reason a summary reports are the run's own, moved by
/// the CLOCK and not by the listing — so the listing eventually reports the terminal
/// status with its reason, and the terminal record is then stable across further
/// listings, which the reader of the same run agrees with.
#[tokio::test]
async fn the_summary_reports_the_status_the_clock_gave_the_run() {
    let emulator = Emulator::new();
    pipeline(&emulator, "clocked").await;

    // Rows differ along how many times the caller lists BEFORE the run's move falls due:
    // a listing must make no difference to the result.
    for listings in [0_usize, 1, 3] {
        let case = format!("listed-{listings}-times");
        let arn = start(&emulator, "clocked", &case, &json!({})).await;
        for listing in 0..listings {
            let summary = summary_of(&emulator, "clocked", &arn)
                .await
                .unwrap_or_else(|| panic!("case {case}/listing {listing}: no summary"));
            let status = str_field(&summary, "PipelineExecutionStatus");
            assert!(
                !status.is_empty(),
                "case {case}/listing {listing}: a run reports a status, got {summary}"
            );
        }

        // Wait for the CONDITION — the run left `Executing` — rather than for a span.
        let deadline = Instant::now() + SETTLE_DEADLINE;
        let settled = loop {
            let summary = summary_of(&emulator, "clocked", &arn)
                .await
                .unwrap_or_else(|| panic!("case {case}: the listing reports the run"));
            if str_field(&summary, "PipelineExecutionStatus") != EXECUTING {
                break summary;
            }
            assert!(
                Instant::now() < deadline,
                "case {case}: the run never left {EXECUTING}, last summary {summary}"
            );
            tokio::time::sleep(POLL_SPACING).await;
        };

        // A run that ended other than `Succeeded` says why, in the member the model
        // reserves for it.
        let status = str_field(&settled, "PipelineExecutionStatus");
        assert_ne!(
            status, EXECUTING,
            "case {case}: past the due instant the run is terminal, got {settled}"
        );
        if status != "Succeeded" {
            assert!(
                settled
                    .get("PipelineExecutionFailureReason")
                    .and_then(Value::as_str)
                    .is_some_and(|reason| !reason.is_empty()),
                "case {case}: a run that did not succeed reports why, got {settled}"
            );
        }

        // And the terminal record is stable: listing it again changes nothing.
        let again = summary_of(&emulator, "clocked", &arn)
            .await
            .unwrap_or_else(|| panic!("case {case}: the listing still reports the run"));
        assert_eq!(
            again, settled,
            "case {case}: a terminal run's summary is stable across listings"
        );
        let described = emulator
            .ok(
                "DescribePipelineExecution",
                &json!({ "PipelineExecutionArn": &arn }),
            )
            .await;
        assert_eq!(
            (
                described.get("PipelineExecutionStatus"),
                described.get("FailureReason"),
            ),
            (
                settled.get("PipelineExecutionStatus"),
                settled.get("PipelineExecutionFailureReason"),
            ),
            "case {case}: the listing and the reader agree about the settled run, got \
             {described}"
        );
    }
}

/// RULE: the creation-time bounds select the runs by their stored start instant, and
/// `SortBy` with `SortOrder` orders the result set — by the start instant or by the run's
/// own ARN — while a value outside either modelled enum is refused with 400
/// `ValidationException` rather than silently ordered by another key.
#[tokio::test]
async fn filters_and_sort_order_decide_which_runs_come_back_and_in_what_order() {
    let emulator = Emulator::new();
    pipeline(&emulator, "ordered").await;

    let mut started = Vec::new();
    for index in 0..4 {
        started.push(start(&emulator, "ordered", &format!("run-{index}"), &json!({})).await);
    }

    // The documented defaults: by `CreationTime`, `Descending` — the newest run first,
    // which is the reverse of the start order above.
    let newest_first: Vec<String> = started.iter().rev().cloned().collect();
    // Sorting by `PipelineExecutionArn` orders by the ARN's own text, whose run id the
    // service generates: the expectation is the sorted order of the ARNs themselves.
    let mut by_arn = started.clone();
    by_arn.sort();
    let by_arn_descending: Vec<String> = by_arn.iter().rev().cloned().collect();

    let cases: Vec<(&str, Value, Vec<String>)> = vec![
        ("no-sort-members", json!({}), newest_first.clone()),
        (
            "creation-time-descending",
            json!({ "SortBy": "CreationTime", "SortOrder": "Descending" }),
            newest_first,
        ),
        (
            "creation-time-ascending",
            json!({ "SortBy": "CreationTime", "SortOrder": "Ascending" }),
            started.clone(),
        ),
        (
            "execution-arn-ascending",
            json!({ "SortBy": "PipelineExecutionArn", "SortOrder": "Ascending" }),
            by_arn,
        ),
        (
            "execution-arn-descending",
            json!({ "SortBy": "PipelineExecutionArn", "SortOrder": "Descending" }),
            by_arn_descending,
        ),
    ];

    for (case, extra, expected) in cases {
        let mut request = json!({ "PipelineName": "ordered" });
        for (member, value) in extra.as_object().expect("an object of members") {
            request[member.as_str()] = value.clone();
        }
        let listing = emulator.ok("ListPipelineExecutions", &request).await;
        assert_eq!(
            listed_arns(&listing),
            expected,
            "case {case}: the runs the request selects, in order, got {listing}"
        );
    }

    // The bounds select by the run's STORED start instant: a bound between two starts
    // splits the result set there, and the two bounds are complementary over one instant.
    let ascending = emulator
        .ok(
            "ListPipelineExecutions",
            &json!({
                "PipelineName": "ordered",
                "SortBy": "CreationTime",
                "SortOrder": "Ascending",
            }),
        )
        .await;
    let reported = summaries(&ascending);
    let started_at = |index: usize| -> f64 {
        reported[index]
            .get("StartTime")
            .and_then(Value::as_f64)
            .unwrap_or_else(|| panic!("a StartTime on summary {index} of {ascending}"))
    };
    // Halfway between the second and third start, so the split falls between them
    // whatever precision the reported instants carry.
    let between = (started_at(1) + started_at(2)) / 2.0;
    for (case, bound, expected) in [
        ("created-after-the-split", "CreatedAfter", &started[2..]),
        ("created-before-the-split", "CreatedBefore", &started[..2]),
    ] {
        let listing = emulator
            .ok(
                "ListPipelineExecutions",
                &json!({
                    "PipelineName": "ordered",
                    bound: between,
                    "SortBy": "CreationTime",
                    "SortOrder": "Ascending",
                }),
            )
            .await;
        assert_eq!(
            listed_arns(&listing),
            expected.to_vec(),
            "case {case}: the bound selects by the stored start instant, got {listing}"
        );
    }

    // A sort key or order outside the modelled enums is refused, not quietly replaced: the
    // model's own constraint, reported in the envelope AWS words a constraint violation in
    // over the member path the request names and that member's enum value set.
    for (case, extra, member_path, value) in [
        ("unknown-sort-by", json!({ "SortBy": "Name" }), "sortBy", "Name"),
        (
            "unknown-sort-order",
            json!({ "SortOrder": "Rising" }),
            "sortOrder",
            "Rising",
        ),
    ] {
        let mut request = json!({ "PipelineName": "ordered" });
        for (member, value) in extra.as_object().expect("an object of members") {
            request[member.as_str()] = value.clone();
        }
        let (status, body) = emulator.call("ListPipelineExecutions", &request).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {case}: a value outside the modelled enum is refused, got {status} \
             {body:?}"
        );
        let reported = message(&body);
        let clause = format!(
            "1 validation error detected: Value '{value}' at '{member_path}' failed to \
             satisfy constraint: Member must satisfy enum value set: ["
        );
        assert!(
            reported.starts_with(&clause),
            "case {case}: the refusal names the member and the value it could not accept, \
             got {reported:?}"
        );
        // The value set the clause closes with is that member's own enum, whichever order
        // the model declares its members in.
        let expected_members: Vec<&str> = match member_path {
            "sortBy" => vec!["CreationTime", "PipelineExecutionArn"],
            _ => vec!["Ascending", "Descending"],
        };
        for member in expected_members {
            assert!(
                reported.contains(member),
                "case {case}: the value set lists the model's member {member}, got \
                 {reported:?}"
            );
        }
    }
}

/// RULE: `MaxResults` caps one page and `NextToken` resumes after the page boundary, so a
/// client walking the token chain sees the whole result set exactly once, in the sort
/// order asked for, whatever the page size — and the last page carries no token.
#[tokio::test]
async fn pagination_walks_the_whole_result_set_exactly_once() {
    let emulator = Emulator::new();
    pipeline(&emulator, "paged").await;

    let mut started = Vec::new();
    for index in 0..7 {
        started.push(start(&emulator, "paged", &format!("page-run-{index}"), &json!({})).await);
    }

    // Rows differ along the page size: one at a time, an uneven split, the whole set, and
    // more than the set holds.
    for page_size in [1_usize, 3, 7, 50] {
        let mut walked: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request = json!({
                "PipelineName": "paged",
                "MaxResults": page_size,
                "SortBy": "CreationTime",
                "SortOrder": "Ascending",
            });
            if let Some(token) = &next_token {
                request["NextToken"] = json!(token);
            }
            let listing = emulator.ok("ListPipelineExecutions", &request).await;
            let page = listed_arns(&listing);
            assert!(
                page.len() <= page_size,
                "case page-size-{page_size}: a page is capped at MaxResults, got {listing}"
            );
            walked.extend(page);
            pages += 1;
            next_token = listing
                .get("NextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            if next_token.is_none() {
                break;
            }
            assert!(
                pages <= started.len(),
                "case page-size-{page_size}: the token chain must terminate, walked \
                 {walked:?}"
            );
        }
        assert_eq!(
            walked, started,
            "case page-size-{page_size}: the token chain reports every run exactly once, \
             in order"
        );
    }

    // A token this listing never issued is refused rather than silently restarting.
    let (status, body) = emulator
        .call(
            "ListPipelineExecutions",
            &json!({ "PipelineName": "paged", "NextToken": "not-a-token" }),
        )
        .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "ValidationException"),
        "case unknown-token: a token no listing issued is refused, got {status} {body:?}"
    );
}

/// RULE: a listing of a pipeline the account does not hold is REFUSED with the modelled
/// `ResourceNotFound` naming that pipeline, rather than answered with an empty list — a
/// name nobody created and one `DeletePipeline` removed are equally unresolvable, and the
/// deleted pipeline's runs go with it.
#[tokio::test]
async fn a_listing_of_a_pipeline_nothing_holds_is_refused() {
    let emulator = Emulator::new();
    pipeline(&emulator, "doomed").await;
    let run = start(&emulator, "doomed", "doomed-run", &json!({})).await;
    assert_eq!(
        listed_arns(
            &emulator
                .ok(
                    "ListPipelineExecutions",
                    &json!({ "PipelineName": "doomed" })
                )
                .await
        ),
        vec![run],
        "case before-the-delete: the stored run is listed"
    );

    emulator
        .ok(
            "DeletePipeline",
            &json!({ "PipelineName": "doomed", "ClientRequestToken": token("gone") }),
        )
        .await;

    // Rows differ along WHY the pipeline is unresolvable, and how the request names it.
    let cases: Vec<(&str, String)> = vec![
        ("never-created", "no-such-pipeline".to_string()),
        ("deleted-since", "doomed".to_string()),
        (
            "deleted-since-by-arn",
            format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:pipeline/doomed"),
        ),
    ];
    for (case, pipeline_name) in cases {
        let (status, body) = emulator
            .call(
                "ListPipelineExecutions",
                &json!({ "PipelineName": pipeline_name }),
            )
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ResourceNotFound"),
            "case {case}: expected 400 ResourceNotFound, got {status} {body:?}"
        );
        let reported = message(&body);
        let name = pipeline_name.rsplit('/').next().unwrap_or(&pipeline_name);
        assert!(
            reported.contains(name) && reported.contains("does not exist"),
            "case {case}: the refusal names the pipeline it could not resolve, got \
             {reported:?}"
        );
    }

    // A pipeline recreated under the freed name inherits no run of the deleted one.
    pipeline(&emulator, "doomed").await;
    let listing = emulator
        .ok(
            "ListPipelineExecutions",
            &json!({ "PipelineName": "doomed" }),
        )
        .await;
    assert_eq!(
        listing.get("PipelineExecutionSummaries"),
        Some(&json!([])),
        "case recreated: the deleted pipeline's runs stay gone, got {listing}"
    );
}
