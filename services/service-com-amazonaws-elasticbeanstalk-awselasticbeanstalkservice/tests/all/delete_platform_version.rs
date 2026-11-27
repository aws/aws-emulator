// SPDX-License-Identifier: CC0-1.0

//! `DeletePlatformVersion`, driven end to end over this service's awsQuery HTTP
//! surface with the shared harness in `crate::harness`.
//!
//! The rule under test: the operation deletes a CUSTOM platform version this
//! account created and nothing else. It resolves the `PlatformArn` against the
//! account's own versions; an ARN naming a version the account does not own —
//! every version Elastic Beanstalk publishes itself — is refused as an ownership
//! failure, one naming no version at all as missing, one whose version has an
//! operation still running as an operation in progress, and one still carrying a
//! running environment as a reference that has to go first. Nothing is deleted by
//! any of those refusals. A delete that IS accepted starts an asynchronous
//! deletion: the version reports the non-terminal `Deleting` status the call
//! answers with, and the record stops being held once the clock has carried the
//! deletion out — after which the same name and version can be created again.
//!
//! The rows span what the rule generalises over: the name and version of the
//! platform (plain, dotted, several versions of one platform), the Region the
//! create was signed for and the ARN then carries, the published catalogue entry a
//! managed ARN names (the first, the last and a superseded release of a branch),
//! the form an unresolvable ARN arrives in, and the state the version is in when
//! the delete arrives (building, ready, already deleting, launched into a running
//! environment, launched into a terminated one).
//!
//! The refusal templates are written out HERE rather than imported from the
//! service, so a change to the service's wording fails these tests instead of
//! moving with them.

use service_com_amazonaws_elasticbeanstalk_awselasticbeanstalkservice as eb;

use crate::harness::{wait_for_status, Emulator, PAST_DUE, SOLUTION_STACK};
use crate::lifecycle::{account, TEST_REGION};

/// Another Region, so a Region-derived ARN is asserted against something that is
/// not the one the harness signs with.
const OTHER_REGION: &str = "eu-west-1";

/// How long the poll below waits for a change the CLOCK owes to be materialised
/// before it gives up: generous, so a loaded host is not what decides the
/// outcome. The change itself is the service's own span, which is far shorter.
const PLATFORM_DEADLINE: std::time::Duration = std::time::Duration::from_secs(60);

/// How long that poll waits between two looks.
const PLATFORM_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(50);

/// Look again until the platform version `platform_arn` reports `expected` — the
/// CONDITION a client waiting for a build polls for, never a fixed wait for the
/// clock — or until [`PLATFORM_DEADLINE`] has passed.
async fn wait_for_platform_status(
    emulator: &Emulator,
    case: &str,
    region: &str,
    platform_arn: &str,
    expected: &str,
) {
    let started = std::time::Instant::now();
    loop {
        let status = reported_status(emulator, case, region, platform_arn).await;
        if status.as_deref() == Some(expected) {
            return;
        }
        assert!(
            started.elapsed() < PLATFORM_DEADLINE,
            "case {case:?}: platform version {platform_arn} never reached {expected}, last \
             status {status:?}"
        );
        tokio::time::sleep(PLATFORM_POLL_INTERVAL).await;
    }
}

/// The template the service refuses a `PlatformArn` whose version this account
/// does not own with.
fn foreign_platform(platform_arn: &str) -> String {
    format!("Platform '{platform_arn}' does not belong to your account.")
}

/// The template the service refuses a `PlatformArn` that answers to no platform
/// version at all with.
fn missing_platform(platform_arn: &str) -> String {
    format!("Platform '{platform_arn}' does not exist.")
}

/// One custom platform version a row drives the operation against: the name and
/// version it is created under, and the Region the create is signed for.
struct Row {
    case: &'static str,
    platform_name: &'static str,
    platform_version: &'static str,
    region: &'static str,
}

/// The versions under test. The rows differ along the name's form (plain, dotted,
/// hyphenated), the version's form (a one-digit release, a patch release, a
/// two-digit one), the Region the ARN is built in, and whether another version of
/// the SAME platform is created beside it.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a plainly named version",
            platform_name: "deletable-platform",
            platform_version: "1.0.0",
            region: TEST_REGION,
        },
        Row {
            case: "a dotted platform name at a patch release",
            platform_name: "team.platform.base",
            platform_version: "0.0.1",
            region: TEST_REGION,
        },
        Row {
            case: "a second version of a platform that keeps another one",
            platform_name: "two-version-platform",
            platform_version: "10.20.30",
            region: TEST_REGION,
        },
        Row {
            case: "a version created in another Region",
            platform_name: "regional-platform",
            platform_version: "2.1.0",
            region: OTHER_REGION,
        },
    ]
}

/// Create the custom platform version `platform_name`/`platform_version` in
/// `region`, answering the ARN it was recorded under.
///
/// The bundle members are the two the create requires; nothing here reads the
/// archive, which is why no object storage is mounted.
async fn create(
    emulator: &Emulator,
    case: &str,
    region: &str,
    platform_name: &str,
    platform_version: &str,
) -> String {
    let created = emulator
        .call_in_region(
            region,
            "CreatePlatformVersion",
            &[
                ("PlatformName", platform_name),
                ("PlatformVersion", platform_version),
                ("PlatformDefinitionBundle.S3Bucket", "platform-bundles"),
                ("PlatformDefinitionBundle.S3Key", "platform.zip"),
            ],
        )
        .await;
    assert_eq!(
        created.status, 200,
        "case {case:?}: expected 200 from CreatePlatformVersion, got {} and body {}",
        created.status, created.body
    );
    eb::state::platform_version_arn(region, platform_name, platform_version)
}

/// The `PlatformStatus` `DescribePlatformVersion` reports for `platform_arn`, or
/// `None` once the version answers no read at all — in which case the refusal is
/// asserted to be the missing-platform one, so a read that failed for another
/// reason cannot pass for a completed deletion.
async fn reported_status(
    emulator: &Emulator,
    case: &str,
    region: &str,
    platform_arn: &str,
) -> Option<String> {
    let described = emulator
        .call_in_region(region, "DescribePlatformVersion", &[("PlatformArn", platform_arn)])
        .await;
    if described.status == 200 {
        return Some(
            described
                .expect("PlatformDescription/PlatformStatus", case)
                .to_string(),
        );
    }
    assert_eq!(
        described.status, 400,
        "case {case:?}: expected 200 or 400 from DescribePlatformVersion, got {} and body {}",
        described.status, described.body
    );
    assert_eq!(
        described.error_code(),
        "InvalidParameterValue",
        "case {case:?}: expected InvalidParameterValue, got body {}",
        described.body
    );
    let expected = missing_platform(platform_arn);
    assert_eq!(
        described.error_message(),
        expected,
        "case {case:?}: expected {expected:?}, got {:?}",
        described.error_message()
    );
    None
}

/// Every custom platform version of this account `ListPlatformVersions` reports
/// in `region`, selected by the `PlatformOwner` filter so the published
/// catalogue's own versions are not in the answer.
async fn own_versions(emulator: &Emulator, case: &str, region: &str) -> Vec<String> {
    let listed = emulator
        .call_in_region(
            region,
            "ListPlatformVersions",
            &[
                ("Filters.member.1.Type", "PlatformOwner"),
                ("Filters.member.1.Operator", "="),
                ("Filters.member.1.Values.member.1", account()),
            ],
        )
        .await;
    assert_eq!(
        listed.status, 200,
        "case {case:?}: expected 200 from ListPlatformVersions, got {} and body {}",
        listed.status, listed.body
    );
    listed
        .values_at("PlatformSummaryList/member/PlatformArn")
        .into_iter()
        .map(str::to_string)
        .collect()
}

/// A delete of a ready custom version starts the deletion, reports it, and the
/// record is held only until the clock has carried that deletion out.
#[tokio::test]
async fn a_delete_starts_the_deletion_and_the_clock_completes_it() {
    let emulator = Emulator::new();

    for row in rows() {
        let case = row.case;
        let arn = create(
            &emulator,
            case,
            row.region,
            row.platform_name,
            row.platform_version,
        )
        .await;
        // A second version of the same platform, so the delete is asserted to
        // take the version it named and not the platform.
        let sibling = create(
            &emulator,
            case,
            row.region,
            row.platform_name,
            "9.9.9",
        )
        .await;

        // The build the create owes has to have completed before the delete, so
        // the refusal for a version with an operation in progress is not what is
        // under test here.
        std::thread::sleep(PAST_DUE);
        assert_eq!(
            reported_status(&emulator, case, row.region, &arn).await,
            Some("Ready".to_string()),
            "case {case:?}: the version must be Ready before the delete"
        );

        let deleted = emulator
            .call_in_region(row.region, "DeletePlatformVersion", &[("PlatformArn", &arn)])
            .await;
        assert_eq!(
            deleted.status, 200,
            "case {case:?}: expected 200 from DeletePlatformVersion, got {} and body {}",
            deleted.status, deleted.body
        );
        assert_eq!(
            deleted.expect("PlatformSummary/PlatformArn", case),
            arn,
            "case {case:?}: the answer must name the version deleted, got {}",
            deleted.body
        );
        assert_eq!(
            deleted.expect("PlatformSummary/PlatformVersion", case),
            row.platform_version,
            "case {case:?}: unexpected PlatformVersion in {}",
            deleted.body
        );
        assert_eq!(
            deleted.expect("PlatformSummary/PlatformOwner", case),
            account(),
            "case {case:?}: a custom version is owned by this account, got {}",
            deleted.body
        );
        // The deletion has STARTED, so the version reports the transient status
        // the real service reports while it runs.
        assert_eq!(
            deleted.expect("PlatformSummary/PlatformStatus", case),
            "Deleting",
            "case {case:?}: unexpected PlatformStatus in {}",
            deleted.body
        );

        // The write went to the store, so the read answers the same status —
        // and the record is still there while the deletion runs.
        assert_eq!(
            reported_status(&emulator, case, row.region, &arn).await,
            Some("Deleting".to_string()),
            "case {case:?}: a deletion in progress must still be described"
        );
        let listed = own_versions(&emulator, case, row.region).await;
        assert!(
            listed.contains(&arn),
            "case {case:?}: a deletion in progress is still listed, got {listed:?}"
        );

        // Once the clock has carried the deletion out the version is gone, and
        // the version beside it is untouched.
        std::thread::sleep(PAST_DUE);
        assert_eq!(
            reported_status(&emulator, case, row.region, &arn).await,
            None,
            "case {case:?}: the completed deletion must have removed the version"
        );
        let listed = own_versions(&emulator, case, row.region).await;
        assert!(
            !listed.contains(&arn),
            "case {case:?}: the deleted version must be gone, got {listed:?}"
        );
        assert!(
            listed.contains(&sibling),
            "case {case:?}: the other version of the platform must survive, got {listed:?}"
        );

        // And the ARN is free again: the same name and version can be created.
        let recreated = create(
            &emulator,
            case,
            row.region,
            row.platform_name,
            row.platform_version,
        )
        .await;
        assert_eq!(
            recreated, arn,
            "case {case:?}: the recreated version must take the same ARN"
        );
    }
}

/// An ARN the account does not own, or that names no version at all, is refused
/// and deletes nothing.
#[tokio::test]
async fn an_arn_this_account_does_not_own_is_refused() {
    let emulator = Emulator::new();

    // A live version beside the refusals, so "nothing was deleted" is asserted
    // against a record the store really holds.
    let live = create(&emulator, "live version", TEST_REGION, "live-platform", "1.0.0").await;

    // The managed rows are read from the service's OWN published catalogue
    // (`eb::state::SOLUTION_STACKS`, what `ListAvailableSolutionStacks` reports)
    // rather than written out: the first entry, the last one, and a release a
    // branch's current version superseded, which is published under its own ARN.
    let first = eb::state::SOLUTION_STACKS
        .first()
        .expect("the service publishes at least one solution stack");
    let last = eb::state::SOLUTION_STACKS
        .last()
        .expect("the service publishes at least one solution stack");
    let superseded = eb::state::SOLUTION_STACKS
        .iter()
        .find_map(|stack| {
            let version = stack.superseded_version()?;
            Some(eb::state::platform_arn_of(
                TEST_REGION,
                stack.branch_name,
                &version,
            ))
        })
        .expect("the catalogue publishes a branch with more than one version");

    let not_owned: Vec<(&str, String)> = vec![
        (
            "the first published catalogue entry",
            first.platform_arn(TEST_REGION),
        ),
        (
            "the last published catalogue entry",
            last.platform_arn(TEST_REGION),
        ),
        (
            "a superseded release of a published branch",
            superseded,
        ),
    ];
    for (case, arn) in &not_owned {
        let refused = emulator
            .call("DeletePlatformVersion", &[("PlatformArn", arn)])
            .await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected HTTP 400, got {} and body {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "InvalidParameterValue",
            "case {case:?}: expected InvalidParameterValue, got body {}",
            refused.body
        );
        let expected = foreign_platform(arn);
        assert_eq!(
            refused.error_message(),
            expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            refused.error_message()
        );
    }

    // An ARN that answers to no version at all — neither the account's nor the
    // catalogue's — is missing rather than foreign. The rows are the forms one
    // arrives in.
    let missing: Vec<(&str, String)> = vec![
        (
            "a custom platform ARN never created",
            eb::state::platform_version_arn(TEST_REGION, "ghost-platform", "1.0.0"),
        ),
        (
            "the live version's ARN in another Region",
            eb::state::platform_version_arn(OTHER_REGION, "live-platform", "1.0.0"),
        ),
        (
            "a version the live platform does not have",
            eb::state::platform_version_arn(TEST_REGION, "live-platform", "9.9.9"),
        ),
        (
            "a service-owned ARN naming no published platform",
            eb::state::platform_arn_of(TEST_REGION, "Nonexistent-Platform", "9.9.9"),
        ),
        (
            // A published version is addressed in the Region the request was
            // signed for, so a catalogue ARN carrying another Region's name is
            // no version this request can reach at all.
            "a published entry in another Region",
            first.platform_arn(OTHER_REGION),
        ),
        ("a value that is no ARN at all", "ghost-platform".to_string()),
    ];
    for (case, arn) in &missing {
        let refused = emulator
            .call("DeletePlatformVersion", &[("PlatformArn", arn)])
            .await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected HTTP 400, got {} and body {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "InvalidParameterValue",
            "case {case:?}: expected InvalidParameterValue, got body {}",
            refused.body
        );
        let expected = missing_platform(arn);
        assert_eq!(
            refused.error_message(),
            expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            refused.error_message()
        );
    }

    // A request that names no version at all cannot name one to delete.
    let unnamed = emulator.call("DeletePlatformVersion", &[]).await;
    assert_eq!(
        unnamed.status, 400,
        "case \"no PlatformArn\": expected HTTP 400, got {} and body {}",
        unnamed.status, unnamed.body
    );
    assert_eq!(
        unnamed.error_code(),
        "MissingRequiredParameter",
        "case \"no PlatformArn\": expected MissingRequiredParameter, got body {}",
        unnamed.body
    );
    assert_eq!(
        unnamed.error_message(),
        "Must specify a PlatformArn.",
        "case \"no PlatformArn\": unexpected message in {}",
        unnamed.body
    );

    // None of the refusals deleted anything: the version the account does hold is
    // still there, and so is every published entry a refusal named.
    std::thread::sleep(PAST_DUE);
    assert_eq!(
        reported_status(&emulator, "live version", TEST_REGION, &live).await,
        Some("Ready".to_string()),
        "case \"live version\": a refused delete must not touch the store"
    );
    for (case, arn) in &not_owned {
        let described = emulator
            .call("DescribePlatformVersion", &[("PlatformArn", arn)])
            .await;
        assert_eq!(
            described.status, 200,
            "case {case:?}: a refused delete must leave the published version readable, \
             got {} and body {}",
            described.status, described.body
        );
        assert_eq!(
            described.expect("PlatformDescription/PlatformOwner", case),
            eb::state::MANAGED_PLATFORM_OWNER,
            "case {case:?}: unexpected PlatformOwner in {}",
            described.body
        );
    }
}

/// A version with a change the clock still owes has an operation in progress, so
/// a delete of it is refused — and so is a second delete of one already being
/// deleted.
#[tokio::test]
async fn a_version_with_an_operation_in_progress_is_refused() {
    let emulator = Emulator::new();

    // The build its create started is still running: the delete arrives before
    // the clock has made the move to `Ready` due.
    let building = create(
        &emulator,
        "a version still being built",
        TEST_REGION,
        "building-platform",
        "1.0.0",
    )
    .await;
    let refused = emulator
        .call("DeletePlatformVersion", &[("PlatformArn", &building)])
        .await;
    assert_eq!(
        refused.status, 400,
        "case \"a version still being built\": expected HTTP 400, got {} and body {}",
        refused.status, refused.body
    );
    // `OperationInProgressException` carries the awsQuery code
    // `OperationInProgressFailure` (its `awsQueryError` trait).
    assert_eq!(
        refused.error_code(),
        "OperationInProgressFailure",
        "case \"a version still being built\": unexpected error code in {}",
        refused.body
    );
    assert!(
        refused.error_message().contains(&building)
            && refused.error_message().contains("Creating"),
        "case \"a version still being built\": the message must name the version and its \
         status, got {:?}",
        refused.error_message()
    );

    // The refusal deleted nothing: the build completes on its own schedule and
    // the version is then deletable.
    std::thread::sleep(PAST_DUE);
    assert_eq!(
        reported_status(&emulator, "a version still being built", TEST_REGION, &building).await,
        Some("Ready".to_string()),
        "case \"a version still being built\": a refused delete must not touch the store"
    );
    let deleted = emulator
        .call("DeletePlatformVersion", &[("PlatformArn", &building)])
        .await;
    assert_eq!(
        deleted.status, 200,
        "case \"a built version\": expected 200 from DeletePlatformVersion, got {} and body {}",
        deleted.status, deleted.body
    );

    // A second delete of the same version: its deletion is the operation already
    // in progress.
    let again = emulator
        .call("DeletePlatformVersion", &[("PlatformArn", &building)])
        .await;
    assert_eq!(
        again.status, 400,
        "case \"a second delete\": expected HTTP 400, got {} and body {}",
        again.status, again.body
    );
    assert_eq!(
        again.error_code(),
        "OperationInProgressFailure",
        "case \"a second delete\": unexpected error code in {}",
        again.body
    );
    assert!(
        again.error_message().contains("Deleting"),
        "case \"a second delete\": the message must name the status, got {:?}",
        again.error_message()
    );
    // And the second refusal did not take the record away early either.
    assert_eq!(
        reported_status(&emulator, "a second delete", TEST_REGION, &building).await,
        Some("Deleting".to_string()),
        "case \"a second delete\": the deletion in progress must still be described"
    );
}

/// "You cannot delete the platform version because there are still environments
/// running on it": a version an environment runs on is deletable only once that
/// environment has gone.
#[tokio::test]
async fn a_version_a_running_environment_uses_is_refused() {
    let emulator = Emulator::new();
    let case = "a version with a running environment";

    let arn = create(&emulator, case, TEST_REGION, "referenced-platform", "1.0.0").await;
    let created = emulator
        .call("CreateApplication", &[("ApplicationName", "platform-app")])
        .await;
    assert_eq!(
        created.status, 200,
        "case {case:?}: expected 200 from CreateApplication, got {} and body {}",
        created.status, created.body
    );

    // The version has to be built before an environment can be launched on it:
    // waited for as the CONDITION the version reaches, never as a fixed time.
    wait_for_platform_status(&emulator, case, TEST_REGION, &arn, "Ready").await;
    let launched = emulator
        .call(
            "CreateEnvironment",
            &[
                ("ApplicationName", "platform-app"),
                ("EnvironmentName", "running-env"),
                ("PlatformArn", &arn),
            ],
        )
        .await;
    assert_eq!(
        launched.status, 200,
        "case {case:?}: expected 200 from CreateEnvironment, got {} and body {}",
        launched.status, launched.body
    );

    // An environment on ANOTHER platform version — the published catalogue's,
    // named by its solution stack — is no reference to this one.
    let beside = emulator
        .call(
            "CreateEnvironment",
            &[
                ("ApplicationName", "platform-app"),
                ("EnvironmentName", "other-env"),
                ("SolutionStackName", SOLUTION_STACK.as_str()),
            ],
        )
        .await;
    assert_eq!(
        beside.status, 200,
        "case {case:?}: expected 200 from CreateEnvironment, got {} and body {}",
        beside.status, beside.body
    );

    let refused = emulator
        .call("DeletePlatformVersion", &[("PlatformArn", &arn)])
        .await;
    assert_eq!(
        refused.status, 400,
        "case {case:?}: expected HTTP 400, got {} and body {}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error_code(),
        "PlatformVersionStillReferencedException",
        "case {case:?}: unexpected error code in {}",
        refused.body
    );
    assert!(
        refused.error_message().contains("running-env"),
        "case {case:?}: the message must name the environment holding the version, got {:?}",
        refused.error_message()
    );
    assert!(
        !refused.error_message().contains("other-env"),
        "case {case:?}: an environment on another version is no reference, got {:?}",
        refused.error_message()
    );
    // Nothing was deleted by the refusal.
    assert_eq!(
        reported_status(&emulator, case, TEST_REGION, &arn).await,
        Some("Ready".to_string()),
        "case {case:?}: a refused delete must not touch the store"
    );

    // Terminate the environment — which a launch has to have got past its
    // initial phase for, waited for as a CONDITION — and once the termination
    // the clock owes has completed the version carries no running environment
    // any more.
    wait_for_status(&emulator, case, "running-env", eb::state::STATUS_READY).await;
    let terminated = emulator
        .call("TerminateEnvironment", &[("EnvironmentName", "running-env")])
        .await;
    assert_eq!(
        terminated.status, 200,
        "case {case:?}: expected 200 from TerminateEnvironment, got {} and body {}",
        terminated.status, terminated.body
    );
    wait_for_status(&emulator, case, "running-env", eb::state::STATUS_TERMINATED).await;
    let deleted = emulator
        .call("DeletePlatformVersion", &[("PlatformArn", &arn)])
        .await;
    assert_eq!(
        deleted.status, 200,
        "case {case:?}: expected 200 from DeletePlatformVersion once the environment has \
         gone, got {} and body {}",
        deleted.status, deleted.body
    );
    assert_eq!(
        deleted.expect("PlatformSummary/PlatformStatus", case),
        "Deleting",
        "case {case:?}: unexpected PlatformStatus in {}",
        deleted.body
    );
}
