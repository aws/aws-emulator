// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the Windows fast launch slice: `EnableFastLaunch`,
//! `DisableFastLaunch` and `DescribeFastLaunchImages`, driven over the wire the service
//! actually speaks — `ec2Query`, through the crate's shared [`super::ec2_query`] harness.
//!
//! The rules under test are:
//!
//! 1. `EnableFastLaunch` STORES THE CONFIGURATION THE REQUEST ASKS FOR against the image's
//!    id and `DescribeFastLaunchImages` REPORTS THE STORE: every member the request sets
//!    round-trips, a member it omits takes the value the model documents for it, and the
//!    members the service owns (the owner of the pre-provisioning resources, the state and
//!    the time that state was reached) are derived rather than taken;
//! 2. the CLOCK, not a caller, moves a configuration `enabling` -> `enabled` and finishes
//!    the cleanup a disable starts — after which the image is reported no more;
//! 3. a value outside what the model documents, and an id the account cannot resolve, are
//!    refused with EC2's own error code, and nothing is stored;
//! 4. `ImageIds`, `Filters` and `MaxResults` narrow and page the report, in `ImageId`
//!    order.
//!
//! Each table row is a distinct input along what the rule generalises over — which members
//! the enable names, the parallel-launch maximum at its published floor and below it, the
//! target snapshot count at its floor and below it, which key the launch template is named
//! by, which version alias, which filter, which values, which page size at each published
//! bound and one past it — so no row is the specification.

use std::time::Duration;

use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, FAST_LAUNCH_RESOURCE_TYPE_SNAPSHOT,
    FAST_LAUNCH_STATE_DISABLING, FAST_LAUNCH_STATE_ENABLED, FAST_LAUNCH_STATE_ENABLING,
    IMAGE_PLATFORM_WINDOWS, ImageSpec, LAUNCH_TEMPLATE_VERSION_DEFAULT,
    LAUNCH_TEMPLATE_VERSION_LATEST, MIN_FAST_LAUNCH_MAX_PARALLEL_LAUNCHES,
    MIN_FAST_LAUNCH_TARGET_RESOURCE_COUNT, PLATFORM_DETAILS_WINDOWS, TRANSITION_DELAY,
    USAGE_OPERATION_WINDOWS, WINDOWS_ROOT_DEVICE_NAME, WINDOWS_ROOT_VOLUME_SIZE,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{
    as_timestamp, call, element, elements, error_code, filters, indexed, unescaped,
};

/// The Region every call here is signed for. A fast launch configuration carries no Region
/// member, so the Region only selects the ec2Query envelope.
const REGION: &str = "us-west-2";

/// Another account, which publishes one public Windows AMI and keeps one private.
const PARTNER_ACCOUNT: &str = "111122223333";

/// A well-formed AMI id no image of the account is stored under.
const UNKNOWN_IMAGE_ID: &str = "ami-0ffffffffffffffff";

/// A launch template id of the right printed shape that no mint of this service ever
/// reached, so it is not an id of a launch template at all.
const UNISSUED_LAUNCH_TEMPLATE_ID: &str = "lt-0ffffffffffffffff";

/// The name the one launch template these tests create is held under.
const LAUNCH_TEMPLATE_NAME: &str = "fast-launch-template";

/// A `(key, value)` request parameter.
fn p(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

/// One ec2Query call through the shared harness.
async fn api(
    router: &MultiServiceRouter,
    action: &str,
    params: &[(String, String)],
) -> (u16, String) {
    call(router, REGION, action, params).await
}

/// One `DescribeFastLaunchImages` call.
async fn describe(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    api(router, "DescribeFastLaunchImages", params).await
}

/// Wait until every clock-owed transition started before now has certainly fallen due.
/// Derived from the service's own span, not a literal, so the two cannot drift.
async fn past_the_transition() {
    tokio::time::sleep(TRANSITION_DELAY + Duration::from_millis(250)).await;
}

/// How many fast launch configurations the store holds.
fn stored_count(engine: &engine::InMemoryEngine) -> usize {
    engine.state::<Ec2State>().lock().fast_launch_images.len()
}

/// A Windows AMI of the calling account, as a register operation would store one: "this
/// value is set to `windows` for Windows AMIs; otherwise, it is blank" (`Image$Platform`)
/// is what makes an image one Windows fast launch applies to.
fn windows_spec(name: &str) -> ImageSpec {
    ImageSpec {
        name: name.to_string(),
        platform: Some(IMAGE_PLATFORM_WINDOWS.to_string()),
        platform_details: PLATFORM_DETAILS_WINDOWS.to_string(),
        usage_operation: USAGE_OPERATION_WINDOWS.to_string(),
        root_device_name: WINDOWS_ROOT_DEVICE_NAME.to_string(),
        root_volume_size: WINDOWS_ROOT_VOLUME_SIZE,
        ..ImageSpec::default()
    }
}

/// The images the tests below work against.
struct Seeded {
    /// Windows AMIs of the calling account, in the order their ids sort in.
    windows: Vec<String>,
    /// A Linux AMI of the calling account: an image Windows fast launch does not apply to.
    linux: String,
    /// A public Windows AMI of another account — one "shared with you".
    shared_windows: String,
    /// A private Windows AMI of another account, which this account cannot see at all.
    private_windows: String,
}

/// Write the images into the engine's store, the way a register operation would, and report
/// the ids they were given.
fn seed(engine: &engine::InMemoryEngine, windows_images: usize) -> Seeded {
    let store = engine.state::<Ec2State>();
    let mut guard = store.lock();
    let windows: Vec<String> = (0..windows_images)
        .map(|index| guard.insert_image(windows_spec(&format!("fast-launch-windows-{index}"))))
        .collect();
    let linux = guard.insert_image(ImageSpec {
        name: "fast-launch-linux".to_string(),
        ..ImageSpec::default()
    });
    let shared_windows = guard.insert_image(ImageSpec {
        public: true,
        owner_id: PARTNER_ACCOUNT.to_string(),
        ..windows_spec("partner-public-windows")
    });
    let private_windows = guard.insert_image(ImageSpec {
        owner_id: PARTNER_ACCOUNT.to_string(),
        ..windows_spec("partner-private-windows")
    });
    Seeded {
        windows,
        linux,
        shared_windows,
        private_windows,
    }
}

/// Create the one launch template the round-trip rows name, and answer its id.
async fn create_launch_template(router: &MultiServiceRouter) -> String {
    let (status, body) = api(
        router,
        "CreateLaunchTemplate",
        &[
            p("LaunchTemplateName", LAUNCH_TEMPLATE_NAME),
            p("LaunchTemplateData.InstanceType", "m5.large"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "the launch template the rows name must be created, got {status} with body {body}"
    );
    element(&body, "launchTemplateId").unwrap_or_default()
}

/// The launch template a configuration must report, when it reports one.
#[derive(Debug, Clone)]
struct LaunchTemplateExpectation {
    id: String,
    name: String,
    /// The version NUMBER the configuration resolved to, which is what it reports even
    /// where the request named an alias.
    version: String,
}

/// One row of the round-trip table: the parameters an enable carries beyond `ImageId`, and
/// the configuration the service must then report for that image.
struct EnableRow {
    case: &'static str,
    extra: Vec<(String, String)>,
    resource_type: String,
    max_parallel_launches: String,
    target_resource_count: Option<String>,
    launch_template: Option<LaunchTemplateExpectation>,
}

/// The rows the round-trip table is written against. They differ along every member the
/// rule generalises over: whether the resource type is named, whether the maximum is named
/// and whether it sits at its published floor or above it, whether a target snapshot count
/// is named, and which key and which version form the launch template is named by.
fn enable_rows(launch_template_id: &str) -> Vec<EnableRow> {
    let floor = MIN_FAST_LAUNCH_MAX_PARALLEL_LAUNCHES.to_string();
    let expectation = |version: i64| {
        Some(LaunchTemplateExpectation {
            id: launch_template_id.to_string(),
            name: LAUNCH_TEMPLATE_NAME.to_string(),
            version: version.to_string(),
        })
    };
    vec![
        EnableRow {
            case: "every optional member omitted",
            extra: Vec::new(),
            // "Supported values include: snapshot, which is the default value"
            // (`EnableFastLaunchRequest$ResourceType`).
            resource_type: FAST_LAUNCH_RESOURCE_TYPE_SNAPSHOT.to_string(),
            // "Value must be 6 or greater" (`MaxParallelLaunches`).
            max_parallel_launches: floor.clone(),
            target_resource_count: None,
            launch_template: None,
        },
        EnableRow {
            case: "the resource type named, and the maximum at its published floor",
            extra: vec![
                p("ResourceType", FAST_LAUNCH_RESOURCE_TYPE_SNAPSHOT),
                p("MaxParallelLaunches", floor.clone()),
            ],
            resource_type: FAST_LAUNCH_RESOURCE_TYPE_SNAPSHOT.to_string(),
            max_parallel_launches: floor.clone(),
            target_resource_count: None,
            launch_template: None,
        },
        EnableRow {
            case: "a maximum above the floor, and a target count at its floor",
            extra: vec![
                p("MaxParallelLaunches", "17"),
                p(
                    "SnapshotConfiguration.TargetResourceCount",
                    MIN_FAST_LAUNCH_TARGET_RESOURCE_COUNT.to_string(),
                ),
            ],
            resource_type: FAST_LAUNCH_RESOURCE_TYPE_SNAPSHOT.to_string(),
            max_parallel_launches: "17".to_string(),
            target_resource_count: Some(MIN_FAST_LAUNCH_TARGET_RESOURCE_COUNT.to_string()),
            launch_template: None,
        },
        EnableRow {
            case: "the launch template named by id at a numbered version",
            extra: vec![
                p("LaunchTemplate.LaunchTemplateId", launch_template_id),
                p("LaunchTemplate.Version", "1"),
                p("SnapshotConfiguration.TargetResourceCount", "9"),
            ],
            resource_type: FAST_LAUNCH_RESOURCE_TYPE_SNAPSHOT.to_string(),
            max_parallel_launches: floor.clone(),
            target_resource_count: Some("9".to_string()),
            launch_template: expectation(1),
        },
        EnableRow {
            case: "the launch template named by name at $Latest",
            extra: vec![
                p("LaunchTemplate.LaunchTemplateName", LAUNCH_TEMPLATE_NAME),
                p("LaunchTemplate.Version", LAUNCH_TEMPLATE_VERSION_LATEST),
            ],
            resource_type: FAST_LAUNCH_RESOURCE_TYPE_SNAPSHOT.to_string(),
            max_parallel_launches: floor.clone(),
            target_resource_count: None,
            launch_template: expectation(1),
        },
        EnableRow {
            case: "the launch template named by name at $Default",
            extra: vec![
                p("LaunchTemplate.LaunchTemplateName", LAUNCH_TEMPLATE_NAME),
                p("LaunchTemplate.Version", LAUNCH_TEMPLATE_VERSION_DEFAULT),
                p("MaxParallelLaunches", "6"),
            ],
            resource_type: FAST_LAUNCH_RESOURCE_TYPE_SNAPSHOT.to_string(),
            max_parallel_launches: floor,
            target_resource_count: None,
            launch_template: expectation(1),
        },
    ]
}

/// Assert that `body` reports the configuration `row` asks for, for `image_id`, in `state`.
fn assert_reported(case: &str, body: &str, row: &EnableRow, image_id: &str, state: &str) {
    let account = engine::emulator_account_id().to_string();
    for (member, expected) in [
        ("imageId", image_id.to_string()),
        ("resourceType", row.resource_type.clone()),
        ("maxParallelLaunches", row.max_parallel_launches.clone()),
        // "The owner ID for the pre-provisioning resource" (the `owner-id` filter): the
        // account the pre-provisioned snapshots belong to.
        ("ownerId", account),
        ("state", state.to_string()),
    ] {
        let actual = element(body, member).unwrap_or_default();
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {member} {expected:?}, got {actual:?} in body {body}"
        );
    }
    let target = element(body, "targetResourceCount");
    assert_eq!(
        target, row.target_resource_count,
        "case {case:?}: expected targetResourceCount {:?}, got {target:?} in body {body}",
        row.target_resource_count
    );
    match &row.launch_template {
        None => assert!(
            element(body, "launchTemplate").is_none(),
            "case {case:?}: a configuration that names no launch template reports none, \
             got body {body}"
        ),
        Some(expected) => {
            for (member, want) in [
                ("launchTemplateId", &expected.id),
                ("launchTemplateName", &expected.name),
                ("version", &expected.version),
            ] {
                let actual = element(body, member).unwrap_or_default();
                assert_eq!(
                    &actual, want,
                    "case {case:?}: expected {member} {want:?}, got {actual:?} in body {body}"
                );
            }
        }
    }
    let transition_time = element(body, "stateTransitionTime").unwrap_or_default();
    assert!(
        as_timestamp(&transition_time).is_some(),
        "case {case:?}: expected an ISO 8601 stateTransitionTime, got {transition_time:?} \
         in body {body}"
    );
}

/// RULE 1: an enable stores the configuration the request asks for, and the describe
/// reports it — every member the request set, every member it left to the model's
/// documented default, and the members the service owns.
#[tokio::test]
async fn an_enable_stores_the_configuration_and_the_describe_reports_it() {
    let (router, engine) = super::ec2_query::emulator();
    let launch_template_id = create_launch_template(&router).await;
    let rows = enable_rows(&launch_template_id);
    let seeded = seed(&engine, rows.len());

    for (row, image_id) in rows.iter().zip(&seeded.windows) {
        let case = row.case;
        let mut params = vec![p("ImageId", image_id.as_str())];
        params.extend(row.extra.clone());

        let (status, body) = api(&router, "EnableFastLaunch", &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from EnableFastLaunch, got {status} with \
             body {body}"
        );
        // A configuration AWS hands back is `enabling`: the reserved snapshots are not in
        // place yet, so the enable's own answer carries the transient state.
        assert_reported(case, &body, row, image_id, FAST_LAUNCH_STATE_ENABLING);

        let (status, body) = describe(&router, &indexed("ImageId", &[image_id.as_str()])).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeFastLaunchImages, got {status} \
             with body {body}"
        );
        assert_reported(case, &body, row, image_id, FAST_LAUNCH_STATE_ENABLING);
    }

    assert_eq!(
        stored_count(&engine),
        seeded.windows.len(),
        "one configuration is stored per image the enable named"
    );

    // An image the account can see but has not enabled is not one of the "images that meet
    // the requested criteria": naming it narrows the report to nothing rather than failing.
    let (status, body) = describe(&router, &indexed("ImageId", &[seeded.linux.as_str()])).await;
    assert_eq!(
        status, 200,
        "an image with no configuration: expected HTTP 200, got {status} with body {body}"
    );
    assert!(
        elements(&body, "imageId").is_empty(),
        "an image with no configuration is reported as no row, got body {body}"
    );
}

/// RULE 1, continued: a second enable for one image CHANGES the settings of the one
/// configuration it stores rather than storing a second.
#[tokio::test]
async fn a_second_enable_changes_the_settings_of_the_one_configuration() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine, 1);
    let image_id = seeded.windows[0].clone();

    for (case, maximum, count) in [
        ("the first enable", "6", "2"),
        ("the settings changed", "12", "7"),
        ("the settings changed again", "8", "1"),
    ] {
        let (status, body) = api(
            &router,
            "EnableFastLaunch",
            &[
                p("ImageId", image_id.as_str()),
                p("MaxParallelLaunches", maximum),
                p("SnapshotConfiguration.TargetResourceCount", count),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );

        let (_, body) = describe(&router, &[]).await;
        let reported = elements(&body, "maxParallelLaunches");
        assert_eq!(
            reported,
            vec![maximum.to_string()],
            "case {case:?}: expected the one configuration to report {maximum:?}, got \
             {reported:?} in body {body}"
        );
        let reported = elements(&body, "targetResourceCount");
        assert_eq!(
            reported,
            vec![count.to_string()],
            "case {case:?}: expected the one configuration to report {count:?}, got \
             {reported:?} in body {body}"
        );
        assert_eq!(
            stored_count(&engine),
            1,
            "case {case:?}: one image carries one configuration"
        );
    }
}

/// RULE 2: the clock moves a configuration `enabling` -> `enabled`, and finishes the
/// cleanup a disable starts — after which the image is reported no more and can be enabled
/// again. A caller never moves either.
#[tokio::test]
async fn the_clock_moves_a_configuration_and_a_disable_discontinues_it() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine, 1);
    let image_id = seeded.windows[0].clone();
    let named = indexed("ImageId", &[image_id.as_str()]);

    let (status, body) = api(
        &router,
        "EnableFastLaunch",
        &[p("ImageId", image_id.as_str())],
    )
    .await;
    assert_eq!(status, 200, "the enable: got {status} with body {body}");

    // The span is never zero, so a caller reading straight after the enable still sees the
    // transient state.
    let (_, body) = describe(&router, &named).await;
    assert_eq!(
        element(&body, "state").unwrap_or_default(),
        FAST_LAUNCH_STATE_ENABLING,
        "straight after the enable the configuration is still enabling, got body {body}"
    );
    let enabling_at = as_timestamp(&element(&body, "stateTransitionTime").unwrap_or_default())
        .expect("the enabling state carries an ISO 8601 transition time");

    past_the_transition().await;
    let (_, body) = describe(&router, &named).await;
    assert_eq!(
        element(&body, "state").unwrap_or_default(),
        FAST_LAUNCH_STATE_ENABLED,
        "the clock moves the configuration to enabled, got body {body}"
    );
    let enabled_at = as_timestamp(&element(&body, "stateTransitionTime").unwrap_or_default())
        .expect("the enabled state carries an ISO 8601 transition time");
    assert!(
        enabled_at > enabling_at,
        "the state changed when the move fell due: expected a transition time after \
         {enabling_at}, got {enabled_at}"
    );

    // A read reports the move; it does not perform it twice, and two reads report the same
    // configuration.
    let (_, repeat) = describe(&router, &named).await;
    assert_eq!(
        as_timestamp(&element(&repeat, "stateTransitionTime").unwrap_or_default()),
        Some(enabled_at),
        "a second read reports the same transition time, got body {repeat}"
    );

    let (status, body) = api(
        &router,
        "DisableFastLaunch",
        &[p("ImageId", image_id.as_str())],
    )
    .await;
    assert_eq!(status, 200, "the disable: got {status} with body {body}");
    assert_eq!(
        element(&body, "state").unwrap_or_default(),
        FAST_LAUNCH_STATE_DISABLING,
        "the disable answers the transient disabling state, got body {body}"
    );
    let (_, body) = describe(&router, &named).await;
    assert_eq!(
        element(&body, "state").unwrap_or_default(),
        FAST_LAUNCH_STATE_DISABLING,
        "the configuration is still reported while it is being cleaned up, got body {body}"
    );

    // "Amazon EC2 must remove all pre-provisioned snapshots before you can enable Windows
    // fast launch again" (`DisableFastLaunch`).
    let (status, body) = api(
        &router,
        "EnableFastLaunch",
        &[p("ImageId", image_id.as_str())],
    )
    .await;
    assert_eq!(
        status, 400,
        "an enable during the cleanup: expected HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "IncorrectState",
        "an enable during the cleanup: got body {body}"
    );

    past_the_transition().await;
    let (status, body) = describe(&router, &[]).await;
    assert_eq!(status, 200, "the describe after the cleanup: body {body}");
    assert!(
        elements(&body, "imageId").is_empty(),
        "once the cleanup is done the image is no longer fast launch enabled, got body {body}"
    );
    assert_eq!(
        stored_count(&engine),
        0,
        "the cleanup takes the configuration with it"
    );

    let (status, body) = api(
        &router,
        "EnableFastLaunch",
        &[p("ImageId", image_id.as_str())],
    )
    .await;
    assert_eq!(
        status, 200,
        "an enable after the cleanup: got {status} with body {body}"
    );
    assert_eq!(
        element(&body, "state").unwrap_or_default(),
        FAST_LAUNCH_STATE_ENABLING,
        "an enable after the cleanup starts the work again, got body {body}"
    );
}

/// One row of the refusal table: what the case is, the action it calls, the parameters it
/// carries, and the HTTP status and EC2 error code it must be refused with.
type RefusalRow = (&'static str, &'static str, Vec<(String, String)>, u16, &'static str);

/// RULE 3: every value outside what the model documents, and every id the account cannot
/// resolve, is refused with EC2's own code — and a refused request stores nothing.
#[tokio::test]
async fn a_value_the_model_does_not_document_is_refused_and_stores_nothing() {
    let (router, engine) = super::ec2_query::emulator();
    let launch_template_id = create_launch_template(&router).await;
    let seeded = seed(&engine, 1);
    let image_id = seeded.windows[0].clone();
    let below_the_floor = (MIN_FAST_LAUNCH_MAX_PARALLEL_LAUNCHES - 1).to_string();
    let below_the_count_floor = (MIN_FAST_LAUNCH_TARGET_RESOURCE_COUNT - 1).to_string();
    let enable = |extra: Vec<(String, String)>| {
        let mut params = vec![p("ImageId", image_id.as_str())];
        params.extend(extra);
        params
    };

    let rows: Vec<RefusalRow> = vec![
        (
            "a parallel-launch maximum one below the published floor",
            "EnableFastLaunch",
            enable(vec![p("MaxParallelLaunches", below_the_floor.clone())]),
            400,
            "InvalidParameterValue",
        ),
        (
            "a parallel-launch maximum of zero",
            "EnableFastLaunch",
            enable(vec![p("MaxParallelLaunches", "0")]),
            400,
            "InvalidParameterValue",
        ),
        (
            "a negative parallel-launch maximum",
            "EnableFastLaunch",
            enable(vec![p("MaxParallelLaunches", "-1")]),
            400,
            "InvalidParameterValue",
        ),
        (
            "a target snapshot count one below its floor",
            "EnableFastLaunch",
            enable(vec![p(
                "SnapshotConfiguration.TargetResourceCount",
                below_the_count_floor.clone(),
            )]),
            400,
            "InvalidParameterValue",
        ),
        (
            "a resource type the enum does not publish",
            "EnableFastLaunch",
            enable(vec![p("ResourceType", "instance")]),
            400,
            "InvalidParameterValue",
        ),
        (
            "an enable on something that is not an AMI id",
            "EnableFastLaunch",
            vec![p("ImageId", "not-an-ami")],
            400,
            "InvalidAMIID.Malformed",
        ),
        (
            "an enable on a well-formed id no image of the account has",
            "EnableFastLaunch",
            vec![p("ImageId", UNKNOWN_IMAGE_ID)],
            400,
            "InvalidAMIID.NotFound",
        ),
        (
            "an enable on a private Windows AMI of another account",
            "EnableFastLaunch",
            vec![p("ImageId", seeded.private_windows.as_str())],
            400,
            "InvalidAMIID.NotFound",
        ),
        (
            "an enable on an image that is not a Windows AMI",
            "EnableFastLaunch",
            vec![p("ImageId", seeded.linux.as_str())],
            400,
            "InvalidParameterValue",
        ),
        (
            "a launch template named by both of its keys",
            "EnableFastLaunch",
            enable(vec![
                p("LaunchTemplate.LaunchTemplateId", launch_template_id.clone()),
                p("LaunchTemplate.LaunchTemplateName", LAUNCH_TEMPLATE_NAME),
                p("LaunchTemplate.Version", "1"),
            ]),
            400,
            "InvalidParameterCombination",
        ),
        (
            "a launch template named by neither of its keys",
            "EnableFastLaunch",
            enable(vec![p("LaunchTemplate.Version", "1")]),
            400,
            "MissingParameter",
        ),
        (
            "a launch template id no mint of this service ever reached",
            "EnableFastLaunch",
            enable(vec![
                p(
                    "LaunchTemplate.LaunchTemplateId",
                    UNISSUED_LAUNCH_TEMPLATE_ID,
                ),
                p("LaunchTemplate.Version", "1"),
            ]),
            400,
            "InvalidLaunchTemplateId.Malformed",
        ),
        (
            "a launch template version the template does not hold",
            "EnableFastLaunch",
            enable(vec![
                p("LaunchTemplate.LaunchTemplateName", LAUNCH_TEMPLATE_NAME),
                p("LaunchTemplate.Version", "9"),
            ]),
            400,
            "InvalidLaunchTemplateId.VersionNotFound",
        ),
        (
            "a launch template version that is neither a number nor an alias",
            "EnableFastLaunch",
            enable(vec![
                p("LaunchTemplate.LaunchTemplateName", LAUNCH_TEMPLATE_NAME),
                p("LaunchTemplate.Version", "latest"),
            ]),
            400,
            "InvalidParameterValue",
        ),
        (
            "an enable that only asks whether it would have worked",
            "EnableFastLaunch",
            enable(vec![p("DryRun", "true")]),
            412,
            "DryRunOperation",
        ),
        (
            "a disable of an image with no fast launch configuration",
            "DisableFastLaunch",
            vec![p("ImageId", image_id.as_str())],
            400,
            "IncorrectState",
        ),
        (
            "a disable of something that is not an AMI id",
            "DisableFastLaunch",
            vec![p("ImageId", "not-an-ami")],
            400,
            "InvalidAMIID.Malformed",
        ),
        (
            "a disable of a well-formed id no image of the account has",
            "DisableFastLaunch",
            vec![p("ImageId", UNKNOWN_IMAGE_ID)],
            400,
            "InvalidAMIID.NotFound",
        ),
        (
            "a describe with a filter this operation does not document",
            "DescribeFastLaunchImages",
            filters(&[("image-id", &[image_id.as_str()])]),
            400,
            "InvalidParameterValue",
        ),
        (
            "a describe with a NextToken this service did not issue",
            "DescribeFastLaunchImages",
            vec![p("NextToken", "not-a-token-this-service-issued")],
            400,
            "InvalidPaginationToken",
        ),
        (
            "a describe naming something that is not an AMI id",
            "DescribeFastLaunchImages",
            indexed("ImageId", &["not-an-ami"]),
            400,
            "InvalidRequest",
        ),
        (
            "a describe naming a well-formed id no image of the account has",
            "DescribeFastLaunchImages",
            indexed("ImageId", &[UNKNOWN_IMAGE_ID]),
            400,
            "InvalidRequest",
        ),
        (
            "a describe naming a private Windows AMI of another account",
            "DescribeFastLaunchImages",
            indexed("ImageId", &[seeded.private_windows.as_str()]),
            400,
            "InvalidRequest",
        ),
    ];

    for (case, action, params, expected_status, expected_code) in rows {
        let (status, body) = api(&router, action, &params).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected error code {expected_code:?}, got body {body}"
        );
        assert_eq!(
            stored_count(&engine),
            0,
            "case {case:?}: a refused request stores nothing, got body {body}"
        );
    }

    // A describe that cannot resolve an id says WHICH id, so the message is built from the
    // request rather than being one fixed sentence: the same refusal for a different id
    // names that one.
    for image_id in ["not-an-ami", UNKNOWN_IMAGE_ID, seeded.private_windows.as_str()] {
        let (_, body) = describe(&router, &indexed("ImageId", &[image_id])).await;
        let message = unescaped(&element(&body, "Message").unwrap_or_default());
        assert_eq!(
            message,
            format!("Can't describe EC2 Fast Launch images. Image ID: {image_id} is not valid."),
            "the refusal names the id the request carried, got body {body}"
        );
    }

    // The same values AT their published floors are accepted, so the rows above refuse the
    // bound and not the member.
    let (status, body) = api(
        &router,
        "EnableFastLaunch",
        &enable(vec![
            p(
                "MaxParallelLaunches",
                MIN_FAST_LAUNCH_MAX_PARALLEL_LAUNCHES.to_string(),
            ),
            p(
                "SnapshotConfiguration.TargetResourceCount",
                MIN_FAST_LAUNCH_TARGET_RESOURCE_COUNT.to_string(),
            ),
        ]),
    )
    .await;
    assert_eq!(
        status, 200,
        "the published floors are accepted: got {status} with body {body}"
    );

    // "You can only change these settings for Windows AMIs that you own or that have been
    // shared with you": a PUBLIC Windows AMI of another account is shared with this one, so
    // it can carry a configuration — unlike the private one the table refuses above.
    let (status, body) = api(
        &router,
        "EnableFastLaunch",
        &[p("ImageId", seeded.shared_windows.as_str())],
    )
    .await;
    assert_eq!(
        status, 200,
        "a shared Windows AMI: got {status} with body {body}"
    );
    assert_eq!(
        stored_count(&engine),
        2,
        "the two accepted enables are the only configurations stored"
    );
}

/// One row of the selection table: what the case is, the parameters the describe carries,
/// and the ids it must report, in order.
type SelectionRow = (&'static str, Vec<(String, String)>, Vec<String>);

/// RULE 4: `ImageIds`, `Filters` and `MaxResults` narrow and page the report, in `ImageId`
/// order.
#[tokio::test]
async fn ids_filters_and_page_sizes_narrow_the_report() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine, 3);
    let account = engine::emulator_account_id().to_string();
    for image_id in &seeded.windows {
        let (status, body) = api(
            &router,
            "EnableFastLaunch",
            &[p("ImageId", image_id.as_str())],
        )
        .await;
        assert_eq!(status, 200, "the enables: got {status} with body {body}");
    }
    // Every configuration has reached `enabled`, so the rows below are about selection
    // rather than about which state each one happens to be in.
    past_the_transition().await;
    let every = seeded.windows.clone();
    let (first, second, third) = (
        seeded.windows[0].clone(),
        seeded.windows[1].clone(),
        seeded.windows[2].clone(),
    );

    let rows: Vec<SelectionRow> = vec![
        ("no narrowing at all", Vec::new(), every.clone()),
        (
            "one id named",
            indexed("ImageId", &[second.as_str()]),
            vec![second.clone()],
        ),
        (
            "two ids named",
            indexed("ImageId", &[third.as_str(), first.as_str()]),
            vec![first.clone(), third.clone()],
        ),
        (
            "an id of an image with no configuration",
            indexed("ImageId", &[seeded.linux.as_str()]),
            Vec::new(),
        ),
        (
            "the owner-id of the account that enabled them",
            filters(&[("owner-id", &[account.as_str()])]),
            every.clone(),
        ),
        (
            "the owner-id of another account",
            filters(&[("owner-id", &[PARTNER_ACCOUNT])]),
            Vec::new(),
        ),
        (
            "the resource-type every configuration pre-provisions",
            filters(&[("resource-type", &[FAST_LAUNCH_RESOURCE_TYPE_SNAPSHOT])]),
            every.clone(),
        ),
        (
            "a resource-type the enum does not publish",
            filters(&[("resource-type", &["instance"])]),
            Vec::new(),
        ),
        (
            "the state the clock has moved them to",
            filters(&[("state", &[FAST_LAUNCH_STATE_ENABLED])]),
            every.clone(),
        ),
        (
            "a state none of them is in",
            filters(&[("state", &[FAST_LAUNCH_STATE_DISABLING])]),
            Vec::new(),
        ),
        (
            "two values of one filter, joined with an OR",
            filters(&[(
                "state",
                &[FAST_LAUNCH_STATE_DISABLING, FAST_LAUNCH_STATE_ENABLED],
            )]),
            every.clone(),
        ),
        (
            "a filter value carrying EC2's many-character wildcard",
            filters(&[("state", &["enabl*"])]),
            every.clone(),
        ),
        (
            "a filter value carrying EC2's one-character wildcard",
            filters(&[("state", &["enable?"])]),
            every.clone(),
        ),
        (
            "two filters, joined with an AND",
            filters(&[
                ("state", &[FAST_LAUNCH_STATE_ENABLED]),
                ("owner-id", &[PARTNER_ACCOUNT]),
            ]),
            Vec::new(),
        ),
        (
            "an id and a filter that disagree",
            {
                let mut params = indexed("ImageId", &[first.as_str()]);
                params.extend(filters(&[("owner-id", &[PARTNER_ACCOUNT])]));
                params
            },
            Vec::new(),
        ),
        (
            // The modelled `DescribeFastLaunchImagesRequestMaxResults` range is 0..200, so
            // zero is the bottom of it: it names no page size rather than an empty page.
            "the page size at the bottom of its published range",
            vec![p("MaxResults", "0")],
            every.clone(),
        ),
        (
            "the page size at the top of its published range",
            vec![p("MaxResults", "200")],
            every.clone(),
        ),
    ];

    for (case, params, expected) in rows {
        let (status, body) = describe(&router, &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        let reported = elements(&body, "imageId");
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?} in body {body}"
        );
    }

    // One page size above the modelled range is refused before the handler runs, by the
    // boundary validator the model's `@range` drives — so this asserts the status, the code
    // being the query protocols' own for a boundary refusal.
    let (status, body) = describe(&router, &[p("MaxResults", "201")]).await;
    assert_eq!(
        status, 400,
        "one above the published page size: expected HTTP 400, got {status} with body {body}"
    );

    // A page of one walks every configuration in `ImageId` order, and the last page carries
    // no token: "this value is null when there are no more items to return".
    let mut walked: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    for step in 0..every.len() {
        let mut params = vec![p("MaxResults", "1")];
        if let Some(carried) = token.clone() {
            params.push(p("NextToken", carried));
        }
        let (status, body) = describe(&router, &params).await;
        assert_eq!(
            status, 200,
            "page {step}: expected HTTP 200, got {status} with body {body}"
        );
        let page = elements(&body, "imageId");
        assert_eq!(
            page.len(),
            1,
            "page {step}: a page size of one reports one configuration, got {page:?}"
        );
        walked.extend(page);
        token = element(&body, "nextToken");
        let expected_more = step + 1 < every.len();
        assert_eq!(
            token.is_some(),
            expected_more,
            "page {step}: a token is carried only while a configuration follows, got \
             body {body}"
        );
    }
    assert_eq!(
        walked, every,
        "paging one at a time reports every configuration in ImageId order, got {walked:?}"
    );

    // A page size BELOW the number of configurations cuts the first page and names where
    // the next one starts.
    let (status, body) = describe(&router, &[p("MaxResults", "2")]).await;
    assert_eq!(status, 200, "a page of two: got {status} with body {body}");
    assert_eq!(
        elements(&body, "imageId"),
        vec![first.clone(), second.clone()],
        "a page of two reports the first two, got body {body}"
    );
    let token = element(&body, "nextToken").expect("a page of two leaves one configuration");
    let (status, body) = describe(&router, &[p("MaxResults", "2"), p("NextToken", token)]).await;
    assert_eq!(status, 200, "the second page: got {status} with body {body}");
    assert_eq!(
        elements(&body, "imageId"),
        vec![third.clone()],
        "the second page reports the rest, got body {body}"
    );
    assert!(
        element(&body, "nextToken").is_none(),
        "the last page carries no token, got body {body}"
    );
}

/// RULE 2 and RULE 4 together: the `state` filter selects on the state the CLOCK has
/// reached, not on the one a caller last asked for.
#[tokio::test]
async fn the_state_filter_selects_on_the_state_the_clock_has_reached() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine, 2);
    for image_id in &seeded.windows {
        let (status, body) = api(
            &router,
            "EnableFastLaunch",
            &[p("ImageId", image_id.as_str())],
        )
        .await;
        assert_eq!(status, 200, "the enables: got {status} with body {body}");
    }

    let selected = |state: &str| {
        let router = &router;
        let params = filters(&[("state", &[state])]);
        async move {
            let (status, body) = describe(router, &params).await;
            assert_eq!(status, 200, "the state filter: got {status} with body {body}");
            elements(&body, "imageId")
        }
    };

    assert_eq!(
        selected(FAST_LAUNCH_STATE_ENABLING).await,
        seeded.windows,
        "before the span is up both configurations are enabling"
    );
    assert!(
        selected(FAST_LAUNCH_STATE_ENABLED).await.is_empty(),
        "before the span is up neither configuration is enabled"
    );

    past_the_transition().await;
    assert!(
        selected(FAST_LAUNCH_STATE_ENABLING).await.is_empty(),
        "once the span is up neither configuration is enabling any more"
    );
    assert_eq!(
        selected(FAST_LAUNCH_STATE_ENABLED).await,
        seeded.windows,
        "once the span is up both configurations are enabled"
    );

    let (status, body) = api(
        &router,
        "DisableFastLaunch",
        &[p("ImageId", seeded.windows[1].as_str())],
    )
    .await;
    assert_eq!(status, 200, "the disable: got {status} with body {body}");
    assert_eq!(
        selected(FAST_LAUNCH_STATE_DISABLING).await,
        vec![seeded.windows[1].clone()],
        "the disabled configuration is the one being cleaned up"
    );
    assert_eq!(
        selected(FAST_LAUNCH_STATE_ENABLED).await,
        vec![seeded.windows[0].clone()],
        "the other configuration is untouched by the disable"
    );
}
