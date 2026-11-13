// SPDX-License-Identifier: CC0-1.0

//! `ListDeployments` as a REPORT over stored state: it answers one summary per deployment
//! the addressed environment holds, in descending deployment number order, cut into pages
//! a `NextToken` walks exactly once.
//!
//! Every case drives real HTTP requests through the router the emulator's front door uses,
//! so what is asserted is what an AppConfig client sees on the wire. The rules under test,
//! each asserted over a SET of inputs rather than one recorded call:
//!
//! 1. a summary reports the stored deployment — every member `DeploymentSummary` models
//!    exactly as `GetDeployment` reports it for that same deployment — and carries none of
//!    the members the summary shape does not model;
//! 2. every member of a summary that targets `Iso8601DateTime` goes onto the restJson1
//!    wire as an RFC 3339 / ISO-8601 STRING, never as an epoch number;
//! 3. a listing reports exactly the deployments of the environment it addresses, newest
//!    deployment number first;
//! 4. `MaxResults` bounds a page and `NextToken` resumes after it, so a walk reports every
//!    deployment exactly once, still newest first, at any page size;
//! 5. an application or an environment that does not exist is 404
//!    `ResourceNotFoundException`, a `NextToken` this operation did not issue is 400
//!    `BadRequestException`, and a `MaxResults` outside the published range is refused;
//! 6. listing is a read — it changes nothing a later caller sees, and it reports what the
//!    deployments and stops before it left.

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_appconfig_amazonappconfig::ops::types::{
    DEPLOYMENT_STATE_VALUES, DEPLOYMENT_TYPE_VALUES,
};

use crate::appconfig_workflow::{
    create_application, create_environment, create_profile, json_call, router, send, Answer,
};

/// The published bounds of `MaxResults` on this operation (`range 1..50`,
/// `smithy_query(query="operation",operation="ListDeployments")`).
const MAX_RESULTS_MIN: i64 = 1;
/// See [`MAX_RESULTS_MIN`].
const MAX_RESULTS_MAX: i64 = 50;

/// Every member `DeploymentSummary` models, from
/// `smithy_query(query="shape",shape="DeploymentSummary")`. A summary and a
/// `GetDeployment` of the same deployment are two reports of one record, so each of these
/// must agree between them.
const SUMMARY_MEMBERS: [&str; 14] = [
    "DeploymentNumber",
    "ConfigurationProfileId",
    "ConfigurationName",
    "ConfigurationVersion",
    "DeploymentDurationInMinutes",
    "GrowthType",
    "GrowthFactor",
    "FinalBakeTimeInMinutes",
    "State",
    "PercentageComplete",
    "StartedAt",
    "CompletedAt",
    "VersionLabel",
    "Type",
];

/// Members the `Deployment` document carries that `DeploymentSummary` does NOT model
/// (same source as [`SUMMARY_MEMBERS`]): a summary is the catalog entry, not the document.
const MEMBERS_A_SUMMARY_OMITS: [&str; 8] = [
    "ApplicationId",
    "EnvironmentId",
    "DeploymentStrategyId",
    "Description",
    "ConfigurationLocationUri",
    "EventLog",
    "AppliedExtensions",
    "KmsKeyIdentifier",
];

/// Where the deployments of one environment live, from this operation's modelled URI
/// (`smithy_query(query="operation",operation="ListDeployments")`).
fn deployments_uri(application: &str, environment: &str) -> String {
    format!("/applications/{application}/environments/{environment}/deployments")
}

/// A member of a document, or `Value::Null` when it is absent.
fn member(body: &Value, name: &str) -> Value {
    body.get(name).cloned().unwrap_or(Value::Null)
}

/// `CreateDeploymentStrategy` with the given pacing, answering the minted id.
///
/// The bounds the rows use are the model's own: `MinutesBetween0And24Hours` is
/// `range 0..1440` and `Percentage` is `range 1..100`
/// (`smithy_query(query="shape",shape="DeploymentSummary")`).
async fn create_strategy(
    router: &MultiServiceRouter,
    name: &str,
    duration: i32,
    growth_factor: f64,
    growth_type: &str,
    bake: i32,
) -> String {
    let answer = json_call(
        router,
        "POST",
        "/deploymentstrategies",
        &json!({
            "Name": name,
            "DeploymentDurationInMinutes": duration,
            "GrowthFactor": growth_factor,
            "GrowthType": growth_type,
            "FinalBakeTimeInMinutes": bake,
            "ReplicateTo": "NONE",
        }),
    )
    .await;
    assert_eq!(
        answer.status,
        201,
        "case {name:?}: CreateDeploymentStrategy must answer 201, got {} / {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    answer
        .string("Id")
        .unwrap_or_else(|| panic!("case {name:?}: CreateDeploymentStrategy answered no Id"))
}

/// A strategy with no rollout window and no bake time: a deployment it paces has nothing
/// to wait out but the one growth interval every rollout takes, so it is over a span after
/// it started — which is what lets one environment hold many deployments
/// (`StartDeployment` refuses a second while one is still active).
async fn instant_strategy(router: &MultiServiceRouter, name: &str) -> String {
    create_strategy(router, name, 0, 100.0, "LINEAR", 0).await
}

/// Host one hosted configuration version of `profile`, answering its version number.
///
/// `label` is bound to the `VersionLabel` HEADER of that write
/// (`smithy_query(query="operation",operation="CreateHostedConfigurationVersion")`) and
/// must contain a non-digit character: the modelled `VersionLabel` pattern is `[^0-9]`.
async fn host_version(
    router: &MultiServiceRouter,
    application: &str,
    profile: &str,
    content: &[u8],
    label: Option<&str>,
) -> i32 {
    let headers: Vec<(&str, String)> = label
        .map(|label| vec![("VersionLabel", label.to_string())])
        .unwrap_or_default();
    let answer = send(
        router,
        "POST",
        &format!(
            "/applications/{application}/configurationprofiles/{profile}/hostedconfigurationversions"
        ),
        "application/json",
        content.to_vec(),
        &headers,
    )
    .await;
    assert_eq!(
        answer.status,
        201,
        "CreateHostedConfigurationVersion must answer 201, got {} / {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    answer
        .header("Version-Number")
        .expect("a hosted version carries a Version-Number")
        .parse()
        .expect("Version-Number is an integer")
}

/// `StartDeployment` of `version` into `environment`, answering the deployment document.
async fn start_deployment(
    router: &MultiServiceRouter,
    application: &str,
    environment: &str,
    case: &str,
    body: &Value,
) -> Answer {
    let answer = json_call(
        router,
        "POST",
        &deployments_uri(application, environment),
        body,
    )
    .await;
    assert_eq!(
        answer.status,
        201,
        "case {case:?}: StartDeployment must answer 201, got {} / {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    answer
}

/// `GET` the listing of one environment, with the query members in `query`.
async fn list(
    router: &MultiServiceRouter,
    application: &str,
    environment: &str,
    query: &[(&str, String)],
) -> Answer {
    let mut uri = deployments_uri(application, environment);
    if !query.is_empty() {
        let rendered: Vec<String> = query
            .iter()
            .map(|(name, value)| format!("{name}={value}"))
            .collect();
        uri.push('?');
        uri.push_str(&rendered.join("&"));
    }
    send(router, "GET", &uri, "application/json", Vec::new(), &[]).await
}

/// The `Items` of a successful listing.
fn items(answer: &Answer, case: &str) -> Vec<Value> {
    assert_eq!(
        answer.status,
        200,
        "case {case:?}: ListDeployments expected 200, got {} / {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    answer
        .json()
        .get("Items")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: no Items array in {}", answer.json()))
}

/// The `DeploymentNumber` of every summary in `page`, in the order the page reports them.
fn numbers(page: &[Value], case: &str) -> Vec<i64> {
    page.iter()
        .map(|summary| {
            summary
                .get("DeploymentNumber")
                .and_then(Value::as_i64)
                .unwrap_or_else(|| {
                    panic!("case {case:?}: a summary carries no DeploymentNumber: {summary}")
                })
        })
        .collect()
}

/// `GetDeployment` of one deployment: the other report of the same record.
async fn get_deployment(
    router: &MultiServiceRouter,
    application: &str,
    environment: &str,
    number: i64,
    case: &str,
) -> Value {
    let answer = json_call(
        router,
        "GET",
        &format!("{}/{number}", deployments_uri(application, environment)),
        &Value::Null,
    )
    .await;
    assert_eq!(
        answer.status,
        200,
        "case {case:?}: GetDeployment must answer 200, got {} / {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    answer.json()
}

/// RULE 2: the instant a summary member names, asserted to be on the wire as the modelled
/// `date-time` STRING. A JSON number here is the epoch-seconds fallback the protocol uses
/// for a member schema carrying no `@timestampFormat`.
fn iso_instant(case: &str, name: &str, value: &Value) -> DateTime {
    let text = value.as_str().unwrap_or_else(|| {
        panic!(
            "case {case:?}: a summary's {name} targets Iso8601DateTime, so it must be an \
             ISO-8601 string on the wire, got {value}"
        )
    });
    DateTime::from_str(text, Format::DateTime).unwrap_or_else(|error| {
        panic!("case {case:?}: {name} must parse as an RFC 3339 date-time, got {text:?}: {error}")
    })
}

/// One deployment a table row asks for: what a failing assertion calls it, the body
/// `StartDeployment` is given (minus the strategy, which the row names by pacing), and
/// whether the row stops the deployment with a revert afterwards.
struct Planned {
    case: &'static str,
    /// `DeploymentDurationInMinutes`, `GrowthFactor`, `GrowthType`,
    /// `FinalBakeTimeInMinutes` of the strategy that paces it.
    pacing: (i32, f64, &'static str, i32),
    /// The `ConfigurationVersion` to deploy — a version number or a version label, both of
    /// which the model's `ConfigurationVersion` accepts for a hosted configuration.
    version: &'static str,
    description: Option<&'static str>,
    /// Whether the row reverts the deployment after it completes, so the table spans a
    /// terminal `DeploymentState` other than `COMPLETE`.
    revert: bool,
    /// The `DeploymentState` the row's pacing and stop leave the deployment in.
    expected_state: &'static str,
}

/// The deployments one environment is built from. Rows differ along what the rules
/// generalise over: the pacing of the deployment (no rollout window at all, and the
/// published maximum of `DeploymentDurationInMinutes`), both `GrowthType` values, the
/// bake time, how the version to deploy is spelled (number or label), whether a
/// `Description` rides along, whether the hosted version carries a `VersionLabel`, and the
/// terminal state the deployment ends in.
///
/// The one in-flight row is LAST, because `StartDeployment` refuses a second deployment
/// while one is still active.
const PLAN: [Planned; 5] = [
    Planned {
        case: "the first deployment, by version number",
        pacing: (0, 100.0, "LINEAR", 0),
        version: "1",
        description: None,
        revert: false,
        expected_state: "COMPLETE",
    },
    Planned {
        case: "a deployment named by version label",
        pacing: (0, 100.0, "EXPONENTIAL", 0),
        version: "v2-beta",
        description: Some("the labelled revision"),
        revert: false,
        expected_state: "COMPLETE",
    },
    Planned {
        case: "a deployment of an unlabelled version",
        // A growth factor that takes more than one interval to reach every target, so the
        // table spans a multi-interval rollout. Not the model's minimum of 1%: each
        // interval of a rollout is a transition of its own, so a hundred of them is a
        // hundred spans to wait out, and this row waits for its deployment to be over.
        pacing: (0, 50.0, "LINEAR", 0),
        version: "3",
        description: Some(""),
        revert: false,
        expected_state: "COMPLETE",
    },
    Planned {
        case: "a completed deployment the caller reverted",
        pacing: (0, 100.0, "LINEAR", 0),
        version: "1",
        description: None,
        revert: true,
        expected_state: "REVERTED",
    },
    Planned {
        case: "a deployment still rolling out",
        pacing: (1440, 10.0, "LINEAR", 1440),
        version: "v2-beta",
        description: Some("the long rollout"),
        revert: false,
        expected_state: "DEPLOYING",
    },
];

/// Host the three configuration versions [`PLAN`] deploys, one of them labelled.
async fn host_the_planned_versions(
    router: &MultiServiceRouter,
    application: &str,
    profile: &str,
) {
    for (content, label) in [
        (br#"{"enabled":true}"#.to_vec(), None),
        (br#"{"enabled":false}"#.to_vec(), Some("v2-beta")),
        (br#"{"enabled":true,"limit":7}"#.to_vec(), None),
    ] {
        host_version(router, application, profile, &content, label).await;
    }
}

/// Run [`PLAN`] against one environment, answering each row's deployment number.
async fn deploy_the_plan(
    router: &MultiServiceRouter,
    application: &str,
    environment: &str,
    profile: &str,
    strategy_prefix: &str,
) -> Vec<i64> {
    let mut numbers = Vec::new();
    for (index, row) in PLAN.iter().enumerate() {
        let (duration, growth_factor, growth_type, bake) = row.pacing;
        let strategy = create_strategy(
            router,
            &format!("{strategy_prefix}-{index}"),
            duration,
            growth_factor,
            growth_type,
            bake,
        )
        .await;
        let mut body = json!({
            "DeploymentStrategyId": strategy,
            "ConfigurationProfileId": profile,
            "ConfigurationVersion": row.version,
        });
        if let Some(description) = row.description {
            body["Description"] = Value::from(description);
        }
        let started = start_deployment(router, application, environment, row.case, &body).await;
        let number = member(&started.json(), "DeploymentNumber")
            .as_i64()
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: StartDeployment must answer a DeploymentNumber, got {}",
                    row.case,
                    started.json()
                )
            });

        if row.revert {
            // Every stage after the one the start answers is owed by the CLOCK, and only a
            // deployment that COMPLETED can be reverted, so the row waits for the
            // condition rather than for a span.
            crate::appconfig_workflow::deployment_in_state(
                router,
                row.case,
                application,
                environment,
                number,
                &["COMPLETE"],
            )
            .await;
            let stopped = send(
                router,
                "DELETE",
                &format!("{}/{number}", deployments_uri(application, environment)),
                "application/json",
                Vec::new(),
                &[("Allow-Revert", "true".to_string())],
            )
            .await;
            // `StopDeployment` answers 202 Accepted
            // (`smithy_query(query="operation",operation="StopDeployment")`).
            assert_eq!(
                stopped.status,
                202,
                "case {:?}: StopDeployment must answer 202, got {} / {}",
                row.case,
                stopped.status,
                String::from_utf8_lossy(&stopped.body)
            );
        }
        // An environment holds one deployment at a time, so a row whose deployment is
        // meant to be over waits for it before the next row starts.
        if row.expected_state != "DEPLOYING" {
            crate::appconfig_workflow::deployment_in_state(
                router,
                row.case,
                application,
                environment,
                number,
                crate::appconfig_workflow::TERMINAL_DEPLOYMENT_STATES,
            )
            .await;
        }
        numbers.push(number);
    }
    numbers
}

/// Build `count` deployments that are over by the time this answers, in a fresh
/// environment, answering their deployment numbers. Used by the pagination walk, where
/// only the ORDER and the count matter.
async fn deploy_completed(
    router: &MultiServiceRouter,
    application: &str,
    environment: &str,
    profile: &str,
    strategy: &str,
    count: usize,
) -> Vec<i64> {
    let mut numbers = Vec::new();
    for index in 0..count {
        let case = format!("completed deployment {index}");
        let started = start_deployment(
            router,
            application,
            environment,
            &case,
            &json!({
                "DeploymentStrategyId": strategy,
                "ConfigurationProfileId": profile,
                "ConfigurationVersion": "1",
            }),
        )
        .await;
        let number = member(&started.json(), "DeploymentNumber")
            .as_i64()
            .unwrap_or_else(|| panic!("case {case:?}: no DeploymentNumber"));
        // The rollout is a transition of its own, so the next deployment into the same
        // environment waits for this one to be over — on the condition, not on a span.
        crate::appconfig_workflow::deployment_in_state(
            router,
            &case,
            application,
            environment,
            number,
            &["COMPLETE"],
        )
        .await;
        numbers.push(number);
    }
    numbers
}

/// RULES 1 and 2: each summary reports the stored deployment exactly as `GetDeployment`
/// reports it, carries none of the members the summary shape does not model, and renders
/// its instants as ISO-8601 strings.
#[tokio::test]
async fn a_summary_reports_the_stored_deployment_as_the_reader_does() {
    let router = router();
    let application = create_application(&router, "summaries").await;
    let profile = create_profile(&router, &application, "summaries").await;
    host_the_planned_versions(&router, &application, &profile).await;
    let environment = create_environment(&router, &application, "summaries").await;

    let planned = deploy_the_plan(&router, &application, &environment, &profile, "summary").await;

    let listed = list(&router, &application, &environment, &[]).await;
    let page = items(&listed, "every deployment");
    assert_eq!(
        page.len(),
        PLAN.len(),
        "case \"every deployment\": expected {} summaries, got {}: {}",
        PLAN.len(),
        page.len(),
        listed.json()
    );

    for (row, number) in PLAN.iter().zip(planned.iter()) {
        let case = row.case;
        let summary = page
            .iter()
            .find(|summary| {
                summary.get("DeploymentNumber").and_then(Value::as_i64) == Some(*number)
            })
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: no summary for deployment {number}: {}",
                    listed.json()
                )
            });

        // The reader is the other document describing this same deployment, so whatever it
        // reports for a member the summary also models must match: the summary is a report
        // over state rather than values made up at listing time.
        let read = get_deployment(&router, &application, &environment, *number, case).await;
        for name in SUMMARY_MEMBERS {
            assert_eq!(
                member(summary, name),
                member(&read, name),
                "case {case:?}: the summary and GetDeployment must report the same {name}, got \
                 {} then {}",
                member(summary, name),
                member(&read, name)
            );
        }
        for name in MEMBERS_A_SUMMARY_OMITS {
            assert!(
                summary.get(name).is_none(),
                "case {case:?}: DeploymentSummary does not model {name}, got {summary}"
            );
        }

        // The row's own pacing and stop decide the state and the pacing members, so the
        // expectation is derived from the row rather than copied from the answer.
        let (duration, growth_factor, growth_type, bake) = row.pacing;
        assert_eq!(
            member(summary, "State").as_str(),
            Some(row.expected_state),
            "case {case:?}: expected State {:?}, got {}",
            row.expected_state,
            member(summary, "State")
        );
        assert!(
            DEPLOYMENT_STATE_VALUES
                .contains(&member(summary, "State").as_str().unwrap_or_default()),
            "case {case:?}: State must be a modelled DeploymentState, got {}",
            member(summary, "State")
        );
        assert_eq!(
            member(summary, "DeploymentDurationInMinutes").as_i64(),
            Some(i64::from(duration)),
            "case {case:?}: expected DeploymentDurationInMinutes {duration}, got {}",
            member(summary, "DeploymentDurationInMinutes")
        );
        assert_eq!(
            member(summary, "FinalBakeTimeInMinutes").as_i64(),
            Some(i64::from(bake)),
            "case {case:?}: expected FinalBakeTimeInMinutes {bake}, got {}",
            member(summary, "FinalBakeTimeInMinutes")
        );
        assert_eq!(
            member(summary, "GrowthType").as_str(),
            Some(growth_type),
            "case {case:?}: expected GrowthType {growth_type:?}, got {}",
            member(summary, "GrowthType")
        );
        assert_eq!(
            member(summary, "GrowthFactor").as_f64(),
            Some(growth_factor),
            "case {case:?}: expected GrowthFactor {growth_factor}, got {}",
            member(summary, "GrowthFactor")
        );
        assert_eq!(
            member(summary, "ConfigurationVersion").as_str(),
            Some(row.version),
            "case {case:?}: the summary reports the version the caller asked to deploy, got {}",
            member(summary, "ConfigurationVersion")
        );
        assert_eq!(
            member(summary, "ConfigurationProfileId").as_str(),
            Some(profile.as_str()),
            "case {case:?}: every summary names the profile that was deployed, got {summary}"
        );
        assert_eq!(
            member(summary, "Type").as_str(),
            Some("USER"),
            "case {case:?}: a deployment a caller started is of DeploymentType USER, got {}",
            member(summary, "Type")
        );
        assert!(
            DEPLOYMENT_TYPE_VALUES.contains(&member(summary, "Type").as_str().unwrap_or_default()),
            "case {case:?}: Type must be a modelled DeploymentType, got {}",
            member(summary, "Type")
        );

        // RULE 2: the instants are ISO-8601 strings, and a deployment cannot complete
        // before it started. A deployment still in flight carries no completion instant.
        let started_at = iso_instant(case, "StartedAt", &member(summary, "StartedAt"));
        if row.expected_state == "DEPLOYING" {
            assert!(
                member(summary, "CompletedAt").is_null(),
                "case {case:?}: a deployment still in flight has no CompletedAt, got {}",
                member(summary, "CompletedAt")
            );
        } else {
            let completed_at = iso_instant(case, "CompletedAt", &member(summary, "CompletedAt"));
            assert!(
                completed_at.as_secs_f64() >= started_at.as_secs_f64(),
                "case {case:?}: a deployment cannot complete before it started, got \
                 {completed_at:?} after {started_at:?}"
            );
        }
    }
}

/// RULE 3: a listing reports exactly the deployments of the environment it addresses, in
/// descending deployment number order — and RULE 6: it is a read, so repeating it answers
/// the same thing.
#[tokio::test]
async fn a_listing_reports_only_that_environments_deployments_newest_first() {
    let router = router();
    let application = create_application(&router, "listings").await;
    let profile = create_profile(&router, &application, "listings").await;
    host_the_planned_versions(&router, &application, &profile).await;
    let strategy = instant_strategy(&router, "listing-instant").await;

    // A sibling environment under the same application, and a sibling application with its
    // own environment: a listing that read more than the environment it addresses fails
    // here.
    let sibling_environment = create_environment(&router, &application, "sibling-env").await;
    deploy_completed(
        &router,
        &application,
        &sibling_environment,
        &profile,
        &strategy,
        2,
    )
    .await;
    let sibling_application = create_application(&router, "sibling-app").await;
    let sibling_profile = create_profile(&router, &sibling_application, "sibling").await;
    host_version(
        &router,
        &sibling_application,
        &sibling_profile,
        br#"{"enabled":true}"#,
        None,
    )
    .await;
    let foreign_environment =
        create_environment(&router, &sibling_application, "foreign-env").await;
    deploy_completed(
        &router,
        &sibling_application,
        &foreign_environment,
        &sibling_profile,
        &instant_strategy(&router, "foreign-instant").await,
        3,
    )
    .await;

    // Rows differ along how many deployments the environment holds: none at all, one, and
    // several.
    for count in [0_usize, 1, 2, 7] {
        let case = format!("{count} deployments");
        let environment = create_environment(&router, &application, &case).await;
        let mut expected = deploy_completed(
            &router,
            &application,
            &environment,
            &profile,
            &strategy,
            count,
        )
        .await;
        // "Lists the deployments for an environment in descending deployment number
        // order", so the numbers come back largest first.
        expected.sort_unstable();
        expected.reverse();

        let answer = list(&router, &application, &environment, &[]).await;
        let page = items(&answer, &case);
        assert_eq!(
            numbers(&page, &case),
            expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            numbers(&page, &case)
        );
        assert!(
            answer.json().get("NextToken").is_none(),
            "case {case:?}: a listing that fits one page carries no NextToken, got {}",
            answer.json()
        );

        // A read changes nothing: the same call answers the same thing.
        let again = list(&router, &application, &environment, &[]).await;
        assert_eq!(
            again.json(),
            answer.json(),
            "case {case:?}: listing twice must answer the same, got {}",
            again.json()
        );
    }

    // The siblings are untouched by every listing above, and each reports only its own
    // deployments — numbered from 1, because a deployment number is a per-environment
    // sequence.
    for (case, target_application, target_environment, expected) in [
        (
            "sibling environment",
            &application,
            &sibling_environment,
            vec![2_i64, 1],
        ),
        (
            "foreign application",
            &sibling_application,
            &foreign_environment,
            vec![3_i64, 2, 1],
        ),
    ] {
        let answer = list(&router, target_application, target_environment, &[]).await;
        let page = items(&answer, case);
        assert_eq!(
            numbers(&page, case),
            expected,
            "case {case:?}: expected {expected:?}, got {}",
            answer.json()
        );
    }
}

/// RULE 4: `MaxResults` bounds a page, `NextToken` resumes after it, and the walk reports
/// every deployment exactly once and still newest first — at every page size, including the
/// published bounds of `MaxResults`.
#[tokio::test]
async fn a_next_token_walk_reports_every_deployment_exactly_once_newest_first() {
    let router = router();
    let application = create_application(&router, "walks").await;
    let profile = create_profile(&router, &application, "walks").await;
    host_version(
        &router,
        &application,
        &profile,
        br#"{"enabled":true}"#,
        None,
    )
    .await;
    let strategy = instant_strategy(&router, "walk-instant").await;

    // Rows span the deployment count and, for each, every page size from the published
    // minimum up past the count, plus the published maximum and no `MaxResults` at all.
    for count in [0_usize, 1, 2, 5, 6] {
        let environment = create_environment(&router, &application, &format!("walk-{count}")).await;
        let mut expected = deploy_completed(
            &router,
            &application,
            &environment,
            &profile,
            &strategy,
            count,
        )
        .await;
        expected.sort_unstable();
        expected.reverse();

        let mut page_sizes: Vec<Option<i64>> =
            (MAX_RESULTS_MIN..=(count as i64 + 1)).map(Some).collect();
        page_sizes.push(Some(MAX_RESULTS_MAX));
        page_sizes.push(None);

        for page_size in page_sizes {
            let case = format!("{count} deployments, MaxResults {page_size:?}");
            let mut seen: Vec<i64> = Vec::new();
            let mut next_token: Option<String> = None;
            let mut pages = 0;
            loop {
                let mut query: Vec<(&str, String)> = Vec::new();
                if let Some(page_size) = page_size {
                    query.push(("max_results", page_size.to_string()));
                }
                if let Some(token) = &next_token {
                    query.push(("next_token", token.clone()));
                }
                let answer = list(&router, &application, &environment, &query).await;
                let page = items(&answer, &case);
                if let Some(page_size) = page_size {
                    assert!(
                        page.len() as i64 <= page_size,
                        "case {case:?}: a page must carry at most {page_size} items, got {}",
                        page.len()
                    );
                }
                seen.extend(numbers(&page, &case));
                pages += 1;
                assert!(
                    pages <= count + 2,
                    "case {case:?}: pagination did not terminate after {pages} pages"
                );
                next_token = answer.string("NextToken");
                if next_token.is_none() {
                    break;
                }
                assert!(
                    !page.is_empty(),
                    "case {case:?}: a page that resumes must carry items, got {}",
                    answer.json()
                );
            }

            assert_eq!(
                seen, expected,
                "case {case:?}: the walk must report every deployment exactly once newest first, \
                 expected {expected:?}, got {seen:?}"
            );
        }
    }
}

/// RULE 5a: a listing is addressed under its application and environment, so an
/// unresolvable level of that path is 404 `ResourceNotFoundException` — never an empty
/// listing. RULE 6: every refused listing leaves the store alone.
#[tokio::test]
async fn listing_under_a_path_the_store_does_not_hold_is_not_found() {
    let router = router();
    let application = create_application(&router, "misses").await;
    let profile = create_profile(&router, &application, "misses").await;
    host_version(
        &router,
        &application,
        &profile,
        br#"{"enabled":true}"#,
        None,
    )
    .await;
    let environment = create_environment(&router, &application, "held").await;
    let strategy = instant_strategy(&router, "miss-instant").await;
    let held = deploy_completed(
        &router,
        &application,
        &environment,
        &profile,
        &strategy,
        1,
    )
    .await;

    // Rows differ along which level of the path is unresolvable and along the shape of the
    // id: an id no resource holds, a near miss of a real one, the id of the wrong kind of
    // resource, and a single character.
    let near_miss_application = format!("{application}x");
    let near_miss_environment = format!("{environment}x");
    let cases: Vec<(&str, String, String)> = vec![
        (
            "an application id no resource holds",
            "nosuchapp".to_string(),
            environment.clone(),
        ),
        (
            "a near miss of a real application",
            near_miss_application,
            environment.clone(),
        ),
        (
            "the id of an environment where an application goes",
            environment.clone(),
            environment.clone(),
        ),
        (
            "a single-character application id",
            "a".to_string(),
            environment.clone(),
        ),
        (
            "an environment id no resource holds",
            application.clone(),
            "nosuchenv".to_string(),
        ),
        (
            "a near miss of a real environment",
            application.clone(),
            near_miss_environment,
        ),
        (
            "the id of an application where an environment goes",
            application.clone(),
            application.clone(),
        ),
        (
            "the id of a profile where an environment goes",
            application.clone(),
            profile.clone(),
        ),
    ];
    for (case, target_application, target_environment) in &cases {
        let answer = list(&router, target_application, target_environment, &[]).await;
        assert_eq!(
            answer.status,
            404,
            "case {case:?}: expected 404, got {} / {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert!(
            answer.error_code().contains("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException, got {:?}",
            answer.error_code()
        );
        assert!(
            answer
                .string("Message")
                .is_some_and(|message| !message.is_empty()),
            "case {case:?}: the refusal must carry a Message, got {}",
            answer.json()
        );
    }

    let case = "the environment that exists";
    let answer = list(&router, &application, &environment, &[]).await;
    let page = items(&answer, case);
    assert_eq!(
        numbers(&page, case),
        held,
        "case {case:?}: expected the one started deployment, got {}",
        answer.json()
    );
}

/// RULE 5b: a `MaxResults` outside the published range is refused by the modelled
/// constraint rather than answered, and each published bound itself is accepted.
#[tokio::test]
async fn a_max_results_past_the_published_bounds_is_refused() {
    let router = router();
    let application = create_application(&router, "bad-max").await;
    let profile = create_profile(&router, &application, "bad-max").await;
    host_version(
        &router,
        &application,
        &profile,
        br#"{"enabled":true}"#,
        None,
    )
    .await;
    let environment = create_environment(&router, &application, "bad-max").await;
    let strategy = instant_strategy(&router, "bad-max-instant").await;
    deploy_completed(
        &router,
        &application,
        &environment,
        &profile,
        &strategy,
        1,
    )
    .await;

    // One past each published bound, and a value that is not a number at all.
    let cases: [(&str, &str); 3] = [
        ("one below the minimum", "0"),
        ("one above the maximum", "51"),
        ("not a number", "many"),
    ];
    for (case, max_results) in cases {
        let answer = list(
            &router,
            &application,
            &environment,
            &[("max_results", max_results.to_string())],
        )
        .await;
        assert_eq!(
            answer.status,
            400,
            "case {case:?}: expected 400 for MaxResults={max_results}, got {} / {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
    }

    for bound in [MAX_RESULTS_MIN, MAX_RESULTS_MAX] {
        let case = format!("MaxResults at the bound {bound}");
        let answer = list(
            &router,
            &application,
            &environment,
            &[("max_results", bound.to_string())],
        )
        .await;
        let page = items(&answer, &case);
        assert_eq!(
            page.len(),
            1,
            "case {case:?}: expected the one started deployment, got {}",
            answer.json()
        );
    }
}

/// RULE 5c: a `NextToken` this operation did not issue is 400 `BadRequestException`, and
/// one it did issue resumes the walk it was minted for.
#[tokio::test]
async fn a_next_token_this_operation_did_not_issue_is_a_bad_request() {
    let router = router();
    let application = create_application(&router, "bad-tokens").await;
    let profile = create_profile(&router, &application, "bad-tokens").await;
    host_version(
        &router,
        &application,
        &profile,
        br#"{"enabled":true}"#,
        None,
    )
    .await;
    let environment = create_environment(&router, &application, "bad-tokens").await;
    let strategy = instant_strategy(&router, "bad-token-instant").await;
    deploy_completed(
        &router,
        &application,
        &environment,
        &profile,
        &strategy,
        3,
    )
    .await;

    // A token this operation DID issue is accepted, so the rows below are refused for
    // being unminted rather than for the operation refusing every token.
    let first = list(
        &router,
        &application,
        &environment,
        &[("max_results", "1".to_string())],
    )
    .await;
    let issued = first
        .string("NextToken")
        .expect("three deployments at MaxResults=1 must leave a NextToken");
    let resumed = list(
        &router,
        &application,
        &environment,
        &[("next_token", issued)],
    )
    .await;
    let page = items(&resumed, "an issued token");
    assert_eq!(
        numbers(&page, "an issued token"),
        vec![2, 1],
        "case \"an issued token\": expected the 2 remaining deployments, got {}",
        resumed.json()
    );

    // Rows differ along HOW the token fails to be one this operation minted: not the
    // codec's alphabet at all, the right alphabet at the wrong length, the codec but not
    // the payload shape, and the payload shape under another listing's name.
    let cases: Vec<(&str, String)> = vec![
        ("not hex at all", "not-a-token".to_string()),
        ("hex of an odd length", "abc".to_string()),
        ("hex with no listing kind in it", hex::encode("abcd")),
        (
            "hex naming another listing",
            hex::encode("ListEnvironments\u{1f}abcd"),
        ),
        (
            "hex naming this listing with no key at all",
            hex::encode("ListDeployments"),
        ),
    ];
    for (case, token) in &cases {
        let answer = list(
            &router,
            &application,
            &environment,
            &[("next_token", token.clone())],
        )
        .await;
        assert_eq!(
            answer.status,
            400,
            "case {case:?}: expected 400, got {} / {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert!(
            answer.error_code().contains("BadRequestException"),
            "case {case:?}: expected BadRequestException, got {:?}",
            answer.error_code()
        );
    }
}

/// RULE 6: the listing reports what the deployments and stops before it left, and changes
/// nothing a later caller sees — the next deployment continues the per-environment
/// sequence past every number listing ever reported.
#[tokio::test]
async fn the_listing_reports_what_the_deployments_before_it_left() {
    let router = router();
    let application = create_application(&router, "sequences").await;
    let profile = create_profile(&router, &application, "sequences").await;
    host_the_planned_versions(&router, &application, &profile).await;
    let strategy = instant_strategy(&router, "sequence-instant").await;

    // Rows differ along how many deployments precede the listing.
    for count in [1_usize, 2, 4] {
        let case = format!("{count} deployments before the listing");
        let environment = create_environment(&router, &application, &case).await;
        let before = deploy_completed(
            &router,
            &application,
            &environment,
            &profile,
            &strategy,
            count,
        )
        .await;

        let listed = list(&router, &application, &environment, &[]).await;
        let page = items(&listed, &case);
        let highest = before.iter().copied().max().expect("each row deploys once");
        assert_eq!(
            numbers(&page, &case).first().copied(),
            Some(highest),
            "case {case:?}: the newest deployment must head the listing, got {}",
            listed.json()
        );

        // The listing consumed no deployment number: the next one continues the sequence.
        let next = deploy_completed(
            &router,
            &application,
            &environment,
            &profile,
            &strategy,
            1,
        )
        .await;
        assert_eq!(
            next,
            vec![highest + 1],
            "case {case:?}: a listing consumes no deployment number, so the next deployment is \
             {}, got {next:?}",
            highest + 1
        );

        let after = list(&router, &application, &environment, &[]).await;
        let page = items(&after, &case);
        assert_eq!(
            numbers(&page, &case).first().copied(),
            Some(highest + 1),
            "case {case:?}: the newest deployment must head the listing, got {}",
            after.json()
        );

        // A stop changes the state the listing reports for the deployment it names, and
        // nothing else about the listing.
        let reverted = send(
            &router,
            "DELETE",
            &format!(
                "{}/{}",
                deployments_uri(&application, &environment),
                highest + 1
            ),
            "application/json",
            Vec::new(),
            &[("Allow-Revert", "true".to_string())],
        )
        .await;
        assert_eq!(
            reverted.status,
            202,
            "case {case:?}: StopDeployment must answer 202, got {} / {}",
            reverted.status,
            String::from_utf8_lossy(&reverted.body)
        );
        let after_stop = list(&router, &application, &environment, &[]).await;
        let page = items(&after_stop, &case);
        assert_eq!(
            numbers(&page, &case),
            numbers(&items(&after, &case), &case),
            "case {case:?}: a stop removes no deployment from the listing, got {}",
            after_stop.json()
        );
        assert_eq!(
            member(&page[0], "State").as_str(),
            Some("REVERTED"),
            "case {case:?}: the listing must report the state the stop left, got {}",
            member(&page[0], "State")
        );
    }
}
