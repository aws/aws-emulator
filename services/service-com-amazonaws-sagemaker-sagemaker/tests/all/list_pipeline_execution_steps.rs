// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for `ListPipelineExecutionSteps`.
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.<Op>`,
//! `awsJson1_1`) through the same `MultiServiceRouter` the production router uses, so
//! what these tests exercise is what a real SDK client gets.
//!
//! The tests assert RULES over a TABLE of inputs rather than one recorded call:
//!
//! * a listing reports one step per step of the run — the steps the STORED definition
//!   declares, restricted to the ones a selective run selected — carrying the name,
//!   display name and description that definition declares and the RUN's own status and
//!   failure reason, and nothing about work nothing performed (no `StartTime`, `EndTime`,
//!   `Metadata`, `AttemptCount` or `CacheHitResult`);
//! * a run that has reached no step (one still `Executing`) lists none;
//! * a condition step is a step the run PERFORMS, so it reports its own `Succeeded` status
//!   and instants, and only the steps of the branch it hands control to are reported;
//! * the steps come back in the order they are reached, reversed by `SortOrder`, and
//!   `MaxResults`/`NextToken` walk that order exactly once whatever the page size;
//! * a request the store cannot resolve is refused and the listing changes nothing a
//!   later caller sees.

use std::collections::BTreeSet;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_sagemaker_sagemaker::state;

use super::inference_deployment_lifecycle::{error_code, str_field, Emulator, ACCOUNT};
// The definition and condition-step fixtures live beside the pipeline-lifecycle suite
// that first needed them, so the shape of a stored definition is written once.
use super::pipeline_lifecycle::{create_body, definition, token};

/// One step as the listing reports it: the name it is addressed by and the optional
/// display name and description its declaration carries.
type ExpectedStep = (&'static str, Option<&'static str>, Option<&'static str>);

/// A `Lambda` step named `name`, depending on `depends_on` and carrying whichever of the
/// optional `DisplayName` / `Description` keys are supplied. `pipeline_lifecycle::step` is
/// this same step without those two keys, which only this suite reads back.
fn labelled_step(
    name: &str,
    depends_on: &[&str],
    display_name: Option<&str>,
    description: Option<&str>,
) -> Value {
    let mut built = json!({
        "Name": name,
        "Type": "Lambda",
        "Arguments": {
            "FunctionArn": format!("arn:aws:lambda:us-east-1:{ACCOUNT}:function:{name}"),
        },
    });
    if !depends_on.is_empty() {
        built["DependsOn"] = json!(depends_on);
    }
    if let Some(display_name) = display_name {
        built["DisplayName"] = json!(display_name);
    }
    if let Some(description) = description {
        built["Description"] = json!(description);
    }
    built
}

/// Create the pipeline `name` with `steps` and start one execution of it, answering the
/// `PipelineExecutionArn` the start minted. `extra` carries whichever optional start
/// members the case needs.
async fn started(emulator: &Emulator, name: &str, steps: Value, extra: &Value) -> String {
    emulator
        .ok(
            "CreatePipeline",
            &create_body(name, &definition(steps), &token(name)),
        )
        .await;
    let mut body = json!({ "PipelineName": name, "ClientRequestToken": token(name) });
    if let Some(members) = extra.as_object() {
        for (member, value) in members {
            body[member.as_str()] = value.clone();
        }
    }
    let started = emulator.ok("StartPipelineExecution", &body).await;
    str_field(&started, "PipelineExecutionArn")
}

/// The `PipelineExecutionSteps` of a listing response, as a list of values.
fn listed(response: &Value) -> Vec<Value> {
    response
        .get("PipelineExecutionSteps")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("expected a PipelineExecutionSteps list in {response}"))
        .clone()
}

/// The `StepName`s of a listing response, in the order it reported them.
fn listed_names(response: &Value) -> Vec<String> {
    listed(response)
        .iter()
        .map(|step| str_field(step, "StepName"))
        .collect()
}

/// RULE: a listing of a run that has reached its steps reports ONE step per step the
/// stored definition declares — restricted to the steps a selective run selected —
/// carrying the `StepName`, `StepDisplayName` and `StepDescription` that declaration
/// gives it and the RUN's own `StepStatus` and `FailureReason`, in the order the steps
/// are reached; a run that has reached no step lists none; and nothing is reported about
/// work nothing performed.
#[tokio::test]
async fn a_listing_reports_the_steps_of_the_run_the_stored_definition_declares() {
    let emulator = Emulator::new();

    // Rows differ along what the rule generalises over: how many steps the definition
    // declares, which optional members each declares, whether the declaration order
    // matches the dependency direction, whether the steps are nested in a condition
    // step's branches, and whether the run selected a subset of them.
    let cases: Vec<(&str, Value, Vec<ExpectedStep>, Value)> = vec![
        (
            "single-step-no-optional-members",
            json!([labelled_step("Only", &[], None, None)]),
            vec![("Only", None, None)],
            json!({}),
        ),
        (
            "two-steps-with-display-name-and-description",
            json!([
                labelled_step("Prepare", &[], Some("Prepare the data"), Some("step one")),
                labelled_step("Train", &["Prepare"], Some("Train the model"), None),
            ]),
            vec![
                ("Prepare", Some("Prepare the data"), Some("step one")),
                ("Train", Some("Train the model"), None),
            ],
            json!({}),
        ),
        (
            "chain-declared-against-the-dependency-direction",
            json!([
                labelled_step("Last", &["Middle"], None, Some("the tail")),
                labelled_step("Middle", &["Head"], None, None),
                labelled_step("Head", &[], Some("the head"), None),
            ]),
            vec![
                ("Last", None, Some("the tail")),
                ("Middle", None, None),
                ("Head", Some("the head"), None),
            ],
            json!({}),
        ),
        (
            "selective-run-reports-only-the-steps-it-selected",
            json!([
                labelled_step("Prepare", &[], None, None),
                labelled_step("Train", &["Prepare"], Some("Train the model"), None),
                labelled_step("Evaluate", &["Train"], None, None),
            ]),
            vec![
                ("Prepare", None, None),
                ("Train", Some("Train the model"), None),
            ],
            json!({
                "SelectiveExecutionConfig": {
                    "SelectedSteps": [{ "StepName": "Train" }, { "StepName": "Prepare" }],
                },
            }),
        ),
    ];

    // Each run is started and read BEFORE the clock owes it anything, so the
    // reached-no-step row is observed on every case.
    let mut runs: Vec<(&str, String, Vec<ExpectedStep>)> = Vec::new();
    for (case, steps, expected, extra) in cases {
        let execution_arn = started(&emulator, case, steps, &extra).await;
        let read = json!({ "PipelineExecutionArn": execution_arn });
        let listing = emulator.ok("ListPipelineExecutionSteps", &read).await;
        assert!(
            listed(&listing).is_empty(),
            "case {case}: a run that has reached no step lists none, got {listing:?}"
        );
        assert!(
            listing.get("NextToken").is_none(),
            "case {case}: nothing is left to page over, got {listing:?}"
        );
        runs.push((case, execution_arn, expected));
    }

    for (case, execution_arn, expected) in runs {
        let read = json!({ "PipelineExecutionArn": execution_arn });
        // Wait for the CONDITION — the run left `Executing`, so it says what became of its
        // steps — rather than for a span.
        poll_until_terminal(&emulator, &read).await;
        let described = emulator.ok("DescribePipelineExecution", &read).await;
        let run_status = str_field(&described, "PipelineExecutionStatus");
        let run_reason = str_field(&described, "FailureReason");

        let ascending = emulator
            .ok(
                "ListPipelineExecutionSteps",
                &json!({ "PipelineExecutionArn": execution_arn, "SortOrder": "Ascending" }),
            )
            .await;
        let reported = listed(&ascending);
        let expected_names: Vec<String> = expected
            .iter()
            .map(|(name, _, _)| (*name).to_string())
            .collect();
        assert_eq!(
            listed_names(&ascending),
            expected_names,
            "case {case}: the run reports its declared steps in the order they are \
             reached, got {ascending:?}"
        );
        assert!(
            ascending.get("NextToken").is_none(),
            "case {case}: the whole result set fits in one page, got {ascending:?}"
        );

        for (step, (name, display_name, description)) in reported.iter().zip(expected.iter()) {
            assert_eq!(
                (
                    step.get("StepDisplayName").and_then(Value::as_str),
                    step.get("StepDescription").and_then(Value::as_str),
                ),
                (*display_name, *description),
                "case {case}/{name}: the declaration's display name and description are \
                 reported, and a key it does not declare is absent, got {step:?}"
            );
            assert_eq!(
                (
                    step.get("StepStatus").and_then(Value::as_str),
                    step.get("FailureReason").and_then(Value::as_str),
                ),
                (Some(run_status.as_str()), Some(run_reason.as_str())),
                "case {case}/{name}: the step reports the run's own status and reason, \
                 got {step:?}"
            );
            // Nothing ran this step, so it spent no time, created no job, was attempted
            // no number of times and hit no cache.
            for member in [
                "StartTime",
                "EndTime",
                "Metadata",
                "AttemptCount",
                "CacheHitResult",
                "SelectiveExecutionResult",
                "IterationContext",
            ] {
                assert!(
                    step.get(member).is_none(),
                    "case {case}/{name}: {member} is not reported for work nothing ran, \
                     got {step:?}"
                );
            }
        }

        // `SortOrder` reverses the order the steps are reached, and the service's own
        // default is the descending one, so an absent `SortOrder` answers that same order.
        let descending = emulator
            .ok(
                "ListPipelineExecutionSteps",
                &json!({ "PipelineExecutionArn": execution_arn, "SortOrder": "Descending" }),
            )
            .await;
        let mut reversed = expected_names.clone();
        reversed.reverse();
        assert_eq!(
            listed_names(&descending),
            reversed,
            "case {case}: Descending reverses the order the steps are reached, got \
             {descending:?}"
        );
        let defaulted = emulator.ok("ListPipelineExecutionSteps", &read).await;
        assert_eq!(
            defaulted, descending,
            "case {case}: an absent SortOrder takes the service's documented default, \
             got {defaulted:?}"
        );

        // The listing is a REPORT: reading it again answers the same thing, and the run
        // it read is unchanged.
        let again = emulator.ok("ListPipelineExecutionSteps", &read).await;
        assert_eq!(
            again, defaulted,
            "case {case}: the listing is stable across reads, got {again:?}"
        );
        let described_again = emulator.ok("DescribePipelineExecution", &read).await;
        assert_eq!(
            described_again, described,
            "case {case}: listing the steps changes nothing a later caller sees, got \
             {described_again:?}"
        );
    }
}

/// RULE: a condition step is a step the run PERFORMS — it evaluates the conditions it
/// declares, so it reports its own `Succeeded` status and the instants it ran between —
/// and the steps of the branch it hands control to are the only ones of its branches the
/// run reaches: the other branch's steps have no step execution in this run, so the
/// listing leaves them out.
#[tokio::test]
async fn a_condition_step_reports_only_the_branch_the_run_reached() {
    let emulator = Emulator::new();

    /// A condition step named `name` whose single condition compares the pipeline
    /// parameter `Outcome` with `expected`, branching to `if_steps` or `else_steps`.
    /// `Equals`/`LeftValue`/`RightValue` are the condition keys SageMaker Pipelines
    /// publishes (the shape `pipeline_lifecycle::condition_step` writes).
    fn parameterised_condition(name: &str, expected: &str, if_steps: Value, else_steps: Value) -> Value {
        json!({
            "Name": name,
            "Type": "Condition",
            "Arguments": {
                "Conditions": [{
                    "Type": "Equals",
                    "LeftValue": { "Get": "Parameters.Outcome" },
                    "RightValue": expected,
                }],
                "IfSteps": if_steps,
                "ElseSteps": else_steps,
            },
        })
    }

    // Rows differ along which branch the conditions select, so each branch is the taken
    // one in one row and the untaken one in the other.
    /// One case: the pipeline (and the `Outcome` value its run assigns), the condition
    /// outcome that selects, the steps the run reaches and the steps it does not.
    type Row = (
        &'static str,
        &'static str,
        &'static str,
        Vec<&'static str>,
        Vec<&'static str>,
    );
    let cases: Vec<Row> = vec![
        (
            "the-if-branch-is-taken",
            "go",
            "True",
            vec!["Prepare", "Check", "WhenTrue", "After"],
            vec!["WhenFalse"],
        ),
        (
            "the-else-branch-is-taken",
            "stop",
            "False",
            vec!["Prepare", "Check", "WhenFalse", "After"],
            vec!["WhenTrue"],
        ),
    ];

    for (case, outcome, expected_outcome, expected, unreached) in cases {
        let steps = json!([
            labelled_step("Prepare", &[], None, None),
            parameterised_condition(
                "Check",
                "go",
                json!([labelled_step("WhenTrue", &["Prepare"], Some("if"), None)]),
                json!([labelled_step("WhenFalse", &["Prepare"], None, Some("else"))]),
            ),
            labelled_step("After", &["Prepare"], None, None),
        ]);
        let declaring = json!({
            "Version": "2020-12-01",
            "Metadata": {},
            "Parameters": [{ "Name": "Outcome", "Type": "String", "DefaultValue": "stop" }],
            "Steps": steps,
        })
        .to_string();
        emulator
            .ok(
                "CreatePipeline",
                &create_body(case, &declaring, &token(case)),
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
        let read = json!({ "PipelineExecutionArn": str_field(&started, "PipelineExecutionArn") });

        // The condition step was performed by the start itself, so it is reported with its
        // own outcome while the run is still executing.
        let running = emulator.ok("ListPipelineExecutionSteps", &read).await;
        assert_eq!(
            listed_names(&running),
            vec!["Check".to_string()],
            "case {case}: the run reports the step it performed, got {running:?}"
        );
        let check = listed(&running)
            .into_iter()
            .next()
            .expect("the condition step");
        assert_eq!(
            check.get("StepStatus").and_then(Value::as_str),
            Some("Succeeded"),
            "case {case}: a condition step that evaluated its conditions succeeded, got \
             {check:?}"
        );
        for member in ["StartTime", "EndTime", "AttemptCount"] {
            assert!(
                check.get(member).is_some(),
                "case {case}: {member} is reported for the step the run performed, got \
                 {check:?}"
            );
        }
        // "The outcome of the condition evaluation that was run by this step execution"
        // (`PipelineExecutionStepMetadata$Condition`), whose two members the model names
        // `True` and `False`.
        assert_eq!(
            check
                .get("Metadata")
                .and_then(|metadata| metadata.get("Condition"))
                .and_then(|condition| condition.get("Outcome"))
                .and_then(Value::as_str),
            Some(expected_outcome),
            "case {case}: the condition step reports the outcome it came to, got {check:?}"
        );

        poll_until_terminal(&emulator, &read).await;
        let settled = emulator
            .ok(
                "ListPipelineExecutionSteps",
                &json!({
                    "PipelineExecutionArn": read["PipelineExecutionArn"].clone(),
                    "SortOrder": "Ascending",
                }),
            )
            .await;
        let reported = listed_names(&settled);
        assert_eq!(
            reported,
            expected
                .iter()
                .map(|name| (*name).to_string())
                .collect::<Vec<String>>(),
            "case {case}: only the steps the run reached are reported, in the order they \
             are reached, got {settled:?}"
        );
        for step in unreached {
            assert!(
                !reported.contains(&step.to_string()),
                "case {case}: {step} is in the branch the run did not take, so it has no \
                 step execution, got {settled:?}"
            );
        }
    }
}

/// Poll the run `read` addresses until it leaves `Executing`, waiting for the CONDITION
/// rather than for a span; panics once a generous deadline passes.
async fn poll_until_terminal(emulator: &Emulator, read: &Value) {
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        let described = emulator.ok("DescribePipelineExecution", read).await;
        if str_field(&described, "PipelineExecutionStatus") != "Executing" {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the run never left Executing, last read {described:?}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// RULE: `MaxResults` caps one page and `NextToken` resumes exactly after the step the
/// page stopped at, so following the tokens walks the whole result set once, in the
/// requested order, whatever the page size — and a token that names no boundary of the
/// result set is refused.
#[tokio::test]
async fn pagination_walks_every_step_exactly_once() {
    let emulator = Emulator::new();

    // More steps than the largest page this suite asks for, so every page size below
    // leaves something to resume from.
    let names: Vec<String> = (0..12).map(|index| format!("Step{index:02}")).collect();
    let steps: Vec<Value> = names
        .iter()
        .map(|name| labelled_step(name, &[], None, None))
        .collect();
    let execution_arn = started(&emulator, "paged", json!(steps), &json!({})).await;
    // Wait for the CONDITION — the run left `Executing` — rather than for a span.
    poll_until_terminal(
        &emulator,
        &json!({ "PipelineExecutionArn": execution_arn.clone() }),
    )
    .await;

    let mut descending: Vec<String> = names.clone();
    descending.reverse();

    // Rows differ along the page size — one at a time, a few at a time, the whole set in
    // one page, and one past the service's own ceiling — and along the sort order.
    let ceiling = state::MAX_PAGE_SIZE;
    for (order, expected) in [("Ascending", &names), ("Descending", &descending)] {
        for page_size in [1_i32, 2, 5, 12, ceiling, ceiling + 1] {
            let case = format!("{order}/max-results-{page_size}");
            let mut walked: Vec<String> = Vec::new();
            let mut next: Option<String> = None;
            let mut pages = 0_usize;
            loop {
                let mut body = json!({
                    "PipelineExecutionArn": execution_arn,
                    "SortOrder": order,
                    "MaxResults": page_size,
                });
                if let Some(token) = &next {
                    body["NextToken"] = json!(token);
                }
                let page = emulator.ok("ListPipelineExecutionSteps", &body).await;
                let reported = listed_names(&page);
                assert!(
                    !reported.is_empty(),
                    "case {case}: a page that carries a token has records, got {page:?}"
                );
                assert!(
                    reported.len() <= page_size.min(ceiling) as usize,
                    "case {case}: a page holds at most MaxResults records capped by the \
                     service ceiling, got {reported:?}"
                );
                walked.extend(reported);
                pages += 1;
                assert!(
                    pages <= names.len() + 1,
                    "case {case}: following the tokens terminates, walked {walked:?}"
                );
                match page.get("NextToken").and_then(Value::as_str) {
                    Some(token) => next = Some(token.to_string()),
                    None => break,
                }
            }
            assert_eq!(
                &walked, expected,
                "case {case}: following the tokens walks the whole result set once, in \
                 the requested order, got {walked:?}"
            );
            assert_eq!(
                walked.iter().collect::<BTreeSet<&String>>().len(),
                walked.len(),
                "case {case}: no step is reported twice, got {walked:?}"
            );
        }
    }

    // A token is opaque, but it must name a boundary of THIS result set: one minted for a
    // step the set does not hold, and one that is not a token at all, are both refused.
    for (case, token) in [
        ("token-naming-no-step", state::encode_page_token("Step99")),
        ("not-a-token", "not-a-page-token".to_string()),
    ] {
        let (status, parsed) = emulator
            .call(
                "ListPipelineExecutionSteps",
                &json!({ "PipelineExecutionArn": execution_arn, "NextToken": token }),
            )
            .await;
        assert_eq!(
            (status, error_code(&parsed).as_str()),
            (400, "ValidationException"),
            "case {case}: expected 400 ValidationException, got {status} {parsed:?}"
        );
    }
}

/// RULE: a request the store cannot resolve, or one outside what the model admits, is
/// REFUSED — an execution nobody holds with the modelled `ResourceNotFound`, and a
/// missing execution ARN or a sort order outside the modelled enum as a bad request —
/// while `MaxResults` is accepted on each published bound and refused one past it.
#[tokio::test]
async fn a_listing_the_store_cannot_resolve_is_refused() {
    let emulator = Emulator::new();
    let execution_arn = started(
        &emulator,
        "listable",
        json!([labelled_step("Only", &[], None, None)]),
        &json!({}),
    )
    .await;

    // A pipeline whose executions go with it when it is deleted, so an execution ARN that
    // resolved a moment ago resolves no longer.
    let deleted_execution = started(
        &emulator,
        "deleted",
        json!([labelled_step("Only", &[], None, None)]),
        &json!({}),
    )
    .await;
    emulator
        .ok(
            "DeletePipeline",
            &json!({
                "PipelineName": "deleted",
                "ClientRequestToken": token("delete-deleted"),
            }),
        )
        .await;

    let unknown = format!(
        "arn:aws:sagemaker:us-east-1:{ACCOUNT}:pipeline/listable/execution/0123456789abcdef"
    );
    let cases: Vec<(&str, Value, &str)> = vec![
        (
            "execution-nobody-started",
            json!({ "PipelineExecutionArn": unknown }),
            "ResourceNotFound",
        ),
        (
            "execution-of-a-deleted-pipeline",
            json!({ "PipelineExecutionArn": deleted_execution }),
            "ResourceNotFound",
        ),
        ("no-execution-named", json!({}), "ValidationException"),
        (
            "sort-order-outside-the-enum",
            json!({ "PipelineExecutionArn": execution_arn, "SortOrder": "Sideways" }),
            "ValidationException",
        ),
        (
            "sort-order-in-the-wrong-case",
            json!({ "PipelineExecutionArn": execution_arn, "SortOrder": "ascending" }),
            "ValidationException",
        ),
        (
            // `MaxResults` is modelled `range 1..100`.
            "max-results-one-below-the-published-bound",
            json!({ "PipelineExecutionArn": execution_arn, "MaxResults": 0 }),
            "ValidationException",
        ),
        (
            "max-results-one-past-the-published-bound",
            json!({ "PipelineExecutionArn": execution_arn, "MaxResults": 101 }),
            "ValidationException",
        ),
    ];

    for (case, body, expected_code) in cases {
        let (status, parsed) = emulator.call("ListPipelineExecutionSteps", &body).await;
        assert_eq!(
            (status, error_code(&parsed).as_str()),
            (400, expected_code),
            "case {case}: expected 400 {expected_code}, got {status} {parsed:?}"
        );
    }

    // Each published `MaxResults` bound itself is accepted.
    for bound in [1, 100] {
        let case = format!("max-results-{bound}");
        let listing = emulator
            .ok(
                "ListPipelineExecutionSteps",
                &json!({ "PipelineExecutionArn": execution_arn, "MaxResults": bound }),
            )
            .await;
        assert!(
            listing.get("PipelineExecutionSteps").is_some(),
            "case {case}: a published bound is accepted and answers the member, got \
             {listing:?}"
        );
    }
}
