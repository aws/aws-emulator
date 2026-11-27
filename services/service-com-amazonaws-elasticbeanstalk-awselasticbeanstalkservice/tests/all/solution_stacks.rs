// SPDX-License-Identifier: CC0-1.0

//! `ListAvailableSolutionStacks`, driven end to end over the service's awsQuery
//! HTTP surface.
//!
//! The rules under test:
//!
//! * the operation reports the service's whole solution-stack catalog — every
//!   stack as a name in `SolutionStacks` and as a `SolutionStackDescription` in
//!   `SolutionStackDetails`, so the two lists agree — and reports the same
//!   catalog to every caller, a read changing nothing;
//! * every reported stack is a version of a platform branch
//!   `ListPlatformBranches` reports and that has not retired, so the service's
//!   two views of its own platform catalog cannot disagree;
//! * every reported name is a `SolutionStackName` that survives a
//!   `CreateEnvironment` / `DescribeEnvironments` round trip.
//!
//! The HTTP harness (`Emulator`, `Answer`, the element reader) is the one
//! `all/lifecycle.rs` owns.

use crate::lifecycle::{elements, Emulator};
use service_com_amazonaws_elasticbeanstalk_awselasticbeanstalkservice as eb;

/// The application the environments below belong to.
const APPLICATION: &str = "solution-stack-app";

/// The catalog the service itself holds (`crate::state::SOLUTION_STACKS`).
fn service_catalog() -> &'static [eb::state::SolutionStack] {
    eb::state::SOLUTION_STACKS
}

#[tokio::test]
async fn the_catalog_is_reported_whole_and_does_not_change_when_read() {
    let emulator = Emulator::new();
    let case = "list available solution stacks";

    let listed = emulator.call("ListAvailableSolutionStacks", &[]).await;
    assert_eq!(
        listed.status, 200,
        "case {case:?}: expected 200 from ListAvailableSolutionStacks, got {} and body {}",
        listed.status, listed.body
    );

    // Every stack in the service's table is reported, under both members.
    let expected_names: Vec<String> = service_catalog()
        .iter()
        .map(eb::state::SolutionStack::name)
        .collect();
    assert_eq!(
        listed.values_at("SolutionStacks/member"),
        expected_names,
        "case {case:?}: SolutionStacks must report the service's whole catalog, got {}",
        listed.body
    );
    assert_eq!(
        listed.values_at("SolutionStackDetails/member/SolutionStackName"),
        expected_names,
        "case {case:?}: SolutionStackDetails must cover the same stacks as SolutionStacks, got {}",
        listed.body
    );

    // Each stack's permitted file types are the ones the table holds, in order.
    for stack in service_catalog() {
        let name = stack.name();
        assert_eq!(
            permitted_file_types(&listed.body, &name, &name),
            stack.permitted_file_types.to_vec(),
            "case {name:?}: unexpected PermittedFileTypes in {}",
            listed.body
        );
    }

    // Pinned LITERALLY, so a wholesale edit to the catalog is visible here
    // rather than absorbed by the table-derived expectations above: the
    // catalog's size, one entry per permitted-file-type family (zip only, the
    // Java SE `jar`, the Tomcat `war`), each named in full.
    assert_eq!(
        listed.values_at("SolutionStacks/member").len(),
        40,
        "case {case:?}: expected the 40 stacks the catalog holds, got {}",
        listed.body
    );
    let pinned: &[(&str, &[&str])] = &[
        (
            "64bit Amazon Linux 2023 v4.7.0 running Python 3.13",
            &["zip"],
        ),
        (
            "64bit Amazon Linux 2023 v4.5.0 running Corretto 21",
            &["jar", "zip"],
        ),
        (
            "64bit Amazon Linux 2023 v5.4.0 running Tomcat 10 with Corretto 21",
            &["war", "zip"],
        ),
    ];
    for (name, file_types) in pinned {
        assert!(
            listed.values_at("SolutionStacks/member").contains(name),
            "case {name:?}: expected the catalog to report this stack, got {}",
            listed.body
        );
        assert_eq!(
            permitted_file_types(&listed.body, name, name),
            file_types.to_vec(),
            "case {name:?}: unexpected PermittedFileTypes in {}",
            listed.body
        );
    }

    // A read changes nothing a later caller sees.
    let again = emulator.call("ListAvailableSolutionStacks", &[]).await;
    assert_eq!(
        again.values_at("SolutionStacks/member"),
        expected_names,
        "case {case:?}: a second list must report the same catalog, got {}",
        again.body
    );
}

#[tokio::test]
async fn every_reported_stack_is_a_version_of_a_platform_branch_that_has_not_retired() {
    let emulator = Emulator::new();

    // The branches the SERVICE reports, with the lifecycle state of each:
    // `BranchName` -> `LifecycleState`.
    let branches = emulator.call("ListPlatformBranches", &[]).await;
    assert_eq!(
        branches.status, 200,
        "expected 200 from ListPlatformBranches, got {} and body {}",
        branches.status, branches.body
    );
    let lifecycle_states: Vec<(String, String)> = elements(&branches.body, "PlatformBranchSummaryList")
        .iter()
        .flat_map(|list| elements(list, "member"))
        .map(|member| {
            (
                elements(member, "BranchName")
                    .first()
                    .unwrap_or(&"")
                    .to_string(),
                elements(member, "LifecycleState")
                    .first()
                    .unwrap_or(&"")
                    .to_string(),
            )
        })
        .collect();
    assert!(
        !lifecycle_states.is_empty(),
        "expected ListPlatformBranches to report the platform catalog, got {}",
        branches.body
    );

    let listed = emulator.call("ListAvailableSolutionStacks", &[]).await;
    for name in listed.values_at("SolutionStacks/member") {
        // AWS publishes a branch as "<runtime> running on <os>" and a stack
        // within it as "<os> v<version> running <runtime>", so the stack name
        // reads back as a branch name.
        let (os_and_version, runtime) = name
            .split_once(" running ")
            .unwrap_or_else(|| panic!("case {name:?}: a stack name carries ' running '"));
        let (os, _version) = os_and_version
            .rsplit_once(" v")
            .unwrap_or_else(|| panic!("case {name:?}: a stack name carries a ' v<version>'"));
        let branch_name = format!("{runtime} running on {os}");
        let state = lifecycle_states
            .iter()
            .find(|(branch, _)| *branch == branch_name)
            .map(|(_, state)| state.as_str())
            .unwrap_or_else(|| {
                panic!(
                    "case {name:?}: expected the branch {branch_name:?} in {}",
                    branches.body
                )
            });
        // A branch life-cycle state goes on the wire capitalized (`Retired`),
        // the form every shape that reports one carries.
        assert_ne!(
            state, "Retired",
            "case {name:?}: a retired branch has no version an account can deploy on, \
             but {branch_name:?} is {state}"
        );
    }
}

#[tokio::test]
async fn every_reported_stack_is_a_stack_an_environment_can_be_created_on() {
    let emulator = Emulator::new();
    assert_eq!(
        emulator
            .call("CreateApplication", &[("ApplicationName", APPLICATION)])
            .await
            .status,
        200,
        "expected 200 from CreateApplication"
    );

    let listed = emulator.call("ListAvailableSolutionStacks", &[]).await;
    let names: Vec<String> = listed
        .values_at("SolutionStacks/member")
        .into_iter()
        .map(str::to_string)
        .collect();
    assert!(
        !names.is_empty(),
        "expected at least one solution stack, got body {}",
        listed.body
    );

    for (index, name) in names.iter().enumerate() {
        // Every name this catalogue reports is one `CreateEnvironment` accepts
        // — it refuses a solution stack the catalogue does not publish — and
        // what this row proves is that a reported name survives the
        // create/describe round trip unchanged.
        let environment_name = format!("stack-env-{index}");
        let created = emulator
            .call(
                "CreateEnvironment",
                &[
                    ("ApplicationName", APPLICATION),
                    ("EnvironmentName", environment_name.as_str()),
                    ("SolutionStackName", name.as_str()),
                ],
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case {name:?}: expected 200 from CreateEnvironment, got {} and body {}",
            created.status, created.body
        );
        let described = emulator
            .call(
                "DescribeEnvironments",
                &[("EnvironmentNames.member.1", environment_name.as_str())],
            )
            .await;
        assert_eq!(
            described.values_at("Environments/member/SolutionStackName"),
            vec![name.as_str()],
            "case {name:?}: the solution stack must survive the round trip, got {}",
            described.body
        );
    }
}

/// The `PermittedFileTypes` the answer reports for the stack named `name`, in
/// document order.
fn permitted_file_types<'a>(body: &'a str, name: &str, case: &str) -> Vec<&'a str> {
    let detail = elements(body, "SolutionStackDetails")
        .into_iter()
        .flat_map(|list| elements(list, "member"))
        .find(|member| {
            elements(member, "SolutionStackName")
                .first()
                .is_some_and(|reported| *reported == name)
        })
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected a SolutionStackDetails member, got body {body}")
        });
    elements(detail, "PermittedFileTypes")
        .into_iter()
        .flat_map(|list| elements(list, "member"))
        .collect()
}
