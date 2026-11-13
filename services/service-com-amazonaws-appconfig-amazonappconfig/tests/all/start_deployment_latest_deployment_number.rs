// SPDX-License-Identifier: CC0-1.0

//! `StartDeployment`'s `LatestDeploymentNumber` is advisory: no value of it refuses a
//! deployment.
//!
//! The rule under test: the member is optional, carries no default the service acts on and
//! is reported by no output member of this service
//! (`smithy_query(query="operation",operation="StartDeployment")` — `LatestDeploymentNumber`
//! targets `Integer` with `default: null`, and the `Deployment` output shape declares no
//! such member), so AppConfig stores nothing from it and refuses nothing over it: a request
//! that is otherwise valid starts a genuine new deployment whatever number it names, and
//! the deployment it starts is the one the environment's own sequence issues next.
//!
//! The table spans what the rule generalises over: the member omitted entirely, the
//! environment's current number, a number one deployment stale, zero, a negative number
//! and the largest `Integer` — `LatestDeploymentNumber` declares no range, so every `i32`
//! is a value a caller may send. Each row must start a REAL deployment: a new number, a
//! record the implemented reader answers, and the previous row's deployment still where it
//! was.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_appconfig_amazonappconfig::state;

use crate::appconfig_workflow::{
    create_application, create_environment, create_profile, json_call, router, Answer,
};
use crate::deployment_wire_fidelity::{create_strategy, host_version, member, Row};
use crate::stop_deployment::get_deployment;

/// What a row puts in `LatestDeploymentNumber`, resolved against the environment's latest
/// number at the instant the row runs — so a row can name the CURRENT number without
/// hard-coding one run's sequence.
#[derive(Debug, Clone, Copy)]
enum Supplied {
    /// The member is absent from the request.
    Omitted,
    /// The number of the environment's latest deployment: the value the documentation
    /// suggests a caller passes.
    Current,
    /// One deployment stale, the value a caller with a cached number sends after any
    /// concurrent deployment.
    Stale,
    /// A fixed number, whatever the environment's sequence is at.
    Fixed(i32),
}

impl Supplied {
    /// The value this row sends when the environment's latest deployment number is
    /// `latest`, or `None` for a row that sends no member.
    fn value(self, latest: i32) -> Option<i32> {
        match self {
            Supplied::Omitted => None,
            Supplied::Current => Some(latest),
            Supplied::Stale => Some(latest - 1),
            Supplied::Fixed(value) => Some(value),
        }
    }
}

/// `StartDeployment` of version 1 of `profile`, carrying `latest_deployment_number` when
/// the row names one.
async fn start_with(
    router: &MultiServiceRouter,
    application: &str,
    environment: &str,
    strategy: &str,
    profile: &str,
    latest_deployment_number: Option<i32>,
) -> Answer {
    let mut body = json!({
        "DeploymentStrategyId": strategy,
        "ConfigurationProfileId": profile,
        "ConfigurationVersion": "1",
    });
    if let Some(number) = latest_deployment_number {
        body["LatestDeploymentNumber"] = Value::from(number);
    }
    json_call(
        router,
        "POST",
        &format!("/applications/{application}/environments/{environment}/deployments"),
        &body,
    )
    .await
}

/// Every value of `LatestDeploymentNumber` starts a deployment: the call is accepted, the
/// number minted is the environment's next one, the answer echoes no such member, and the
/// implemented reader answers the record that was written.
#[tokio::test]
async fn no_latest_deployment_number_refuses_a_deployment() {
    let router = router();
    let application = create_application(&router, "advisory-number").await;
    let profile = create_profile(&router, &application, "config").await;
    host_version(&router, &application, &profile).await;

    // No rollout window and no bake time, so AppConfig has nothing to pace or monitor and
    // each deployment is over when its start answers — which is what lets the next row
    // deploy into the same environment.
    let row: Row = ("no rollout window, no bake", 0, 100.0, "LINEAR", 0);
    let strategy = create_strategy(&router, 0, &row).await;
    let environment = create_environment(&router, &application, "advisory").await;

    // Rows differ along the number supplied: absent, the current one, a stale one, zero, a
    // negative number, a number far past anything this environment issued, and the
    // published bounds of `Integer`.
    let rows: [(&str, Supplied); 8] = [
        ("the member omitted entirely", Supplied::Omitted),
        ("the environment's current number", Supplied::Current),
        ("one deployment stale", Supplied::Stale),
        ("zero, which no environment ever issued", Supplied::Fixed(0)),
        ("a negative number", Supplied::Fixed(-1)),
        ("a number far past the sequence", Supplied::Fixed(999_999_999)),
        ("the largest Integer", Supplied::Fixed(i32::MAX)),
        ("the smallest Integer", Supplied::Fixed(i32::MIN)),
    ];

    for (index, (case, supplied)) in rows.iter().enumerate() {
        // The environment numbers its deployments from 1, so the number this row's start
        // mints is its place in the table and the latest before it is one less.
        let expected_number = index as i32 + 1;
        let latest_before = expected_number - 1;
        let answer = start_with(
            &router,
            &application,
            &environment,
            &strategy,
            &profile,
            supplied.value(latest_before),
        )
        .await;
        assert_eq!(
            answer.status,
            201,
            "case {case:?}: StartDeployment must answer the modelled 201, got {} / {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        let body = answer.json();
        assert_eq!(
            member(&body, "DeploymentNumber").as_i64(),
            Some(i64::from(expected_number)),
            "case {case:?}: the deployment started is the environment's next one, got {}",
            member(&body, "DeploymentNumber")
        );
        assert!(
            body.get("LatestDeploymentNumber").is_none(),
            "case {case:?}: no output member reports the advisory number, got {body}"
        );
        assert_eq!(
            member(&body, "State").as_str(),
            Some(state::DEPLOYMENT_COMPLETE),
            "case {case:?}: this row's strategy paces no stage, so its deployment is over \
             when the start answers, got {}",
            member(&body, "State")
        );

        // The deployment is real: the implemented reader answers the record it wrote.
        let read_back = get_deployment(&router, &application, &environment, expected_number).await;
        assert_eq!(
            read_back.status,
            200,
            "case {case:?}: GetDeployment must answer the deployment that was started, got {} / {}",
            read_back.status,
            String::from_utf8_lossy(&read_back.body)
        );
        let stored = read_back.json();
        assert_eq!(
            member(&stored, "ConfigurationVersion").as_str(),
            Some("1"),
            "case {case:?}: the stored deployment names the version it deployed, got {}",
            member(&stored, "ConfigurationVersion")
        );
        assert!(
            stored.get("LatestDeploymentNumber").is_none(),
            "case {case:?}: the stored deployment holds no advisory number, got {stored}"
        );
    }

    // Every row's deployment is still listed under the environment, newest first: the
    // table started one deployment per row rather than replacing one.
    let listed = json_call(
        &router,
        "GET",
        &format!("/applications/{application}/environments/{environment}/deployments"),
        &Value::Null,
    )
    .await
    .json();
    let numbers: Vec<i64> = member(&listed, "Items")
        .as_array()
        .unwrap_or(&Vec::new())
        .iter()
        .filter_map(|item| member(item, "DeploymentNumber").as_i64())
        .collect();
    let expected: Vec<i64> = (1..=rows.len() as i64).rev().collect();
    assert_eq!(
        numbers, expected,
        "every row must have started its own deployment, newest first: expected \
         {expected:?}, got {numbers:?}"
    );
}
