// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DeleteLaunchTemplateVersions`, over the wire the service actually
//! speaks (`ec2Query`, through the crate's shared [`super::ec2_query`] harness).
//!
//! The rules under test are:
//!
//! 1. THE OUTCOME OF EVERY VERSION THE REQUEST NAMES IS REPORTED, ONE ITEM EACH:
//!    "information about the launch template versions that were successfully deleted"
//!    (`DeleteLaunchTemplateVersionsResult$SuccessfullyDeletedLaunchTemplateVersions`)
//!    beside "information about the launch template versions that could not be deleted"
//!    ($UnsuccessfullyDeletedLaunchTemplateVersions) — so a request naming versions the
//!    template holds and versions it does not succeeds in part: each held version is taken
//!    out of the store, and each version the template does not hold is reported under the
//!    `launchTemplateVersionDoesNotExist` member of `LaunchTemplateErrorCode`
//!    (`smithy_query(query="shape",shape="LaunchTemplateErrorCode")`). A later
//!    `DescribeLaunchTemplateVersions` reports exactly the versions that survived.
//! 2. THE DEFAULT VERSION IS NEVER DELETED: "you can't delete the default version of a
//!    launch template; you must first assign a different version as the default. If the
//!    default version is the only version for the launch template, you must delete the
//!    entire launch template using DeleteLaunchTemplate" — the request is refused whole,
//!    `InvalidParameterValue`, and every version it named is still there afterwards.
//! 3. A DELETED VERSION'S NUMBER IS NEVER MINTED AGAIN — "you can't specify, change, or
//!    replace the numbering of launch template versions" (`CreateLaunchTemplateVersion`) —
//!    while `LaunchTemplate$LatestVersionNumber`, "the version number of the latest version
//!    of the launch template", follows the versions that remain.
//! 4. A REQUEST THAT NAMES NO ONE TEMPLATE, OR NO VERSION NUMBER, DELETES NOTHING: the
//!    template is resolved by exactly one of its two keys, and `Versions` carries version
//!    numbers — up to 200 of them — so a request outside any of those is refused with the
//!    code for the parameter that is wrong and the store is left as it was.
//!
//! Each table row is a distinct input along what the rules generalise over — how many
//! versions are named, in which order, which of them the template holds, whether one is
//! named twice, which version is the default, and which key names the template — so no row
//! is the specification.

use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, error_code, error_message};
use super::wire::{child, child_text, document, set_items};

/// The Region every call here is signed for.
const REGION: &str = "us-west-2";

/// The one launch parameter a template under test carries: "you must specify at least one
/// parameter for the launch template data" (`RequestLaunchTemplateData`), and an instance
/// type is the cheapest one that asks nothing of any other store.
const INSTANCE_TYPE: &str = "t3.micro";

/// The `ResponseError$Code` an item carries for a version the named template does not hold.
const VERSION_DOES_NOT_EXIST_CODE: &str = "launchTemplateVersionDoesNotExist";

/// The `ResponseError$Message` that code carries: fixed text, naming neither the version nor
/// the template.
const VERSION_DOES_NOT_EXIST_MESSAGE: &str = "The launch template version does not exist.";

/// The message a request that names the template's default version is refused with: fixed
/// text, whichever version is the default and whichever template holds it.
const DEFAULT_VERSION_REFUSAL: &str = "The default version cannot be deleted. Either specify \
                                       another version as default or delete the launch \
                                       template.";

/// The most versions one request may name: "you can delete up to 200 launch template
/// versions in a single request" (`DeleteLaunchTemplateVersions`).
const MAX_DELETED_VERSIONS: usize = 200;

/// A `(key, value)` request parameter.
fn p(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

/// `LaunchTemplateVersion.N` form fields, as ec2Query flattens the `Versions` list.
fn versions(named: &[&str]) -> Vec<(String, String)> {
    named
        .iter()
        .enumerate()
        .map(|(index, version)| p(&format!("LaunchTemplateVersion.{}", index + 1), *version))
        .collect()
}

fn emulator() -> MultiServiceRouter {
    super::ec2_query::emulator().0
}

/// One launch template named `name`, with `held` versions in all (its first version plus
/// `held - 1` more), as its id.
async fn template_with_versions(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    held: usize,
) -> String {
    let (status, body) = call(
        router,
        REGION,
        "CreateLaunchTemplate",
        &[
            p("LaunchTemplateName", name),
            p("LaunchTemplateData.InstanceType", INSTANCE_TYPE),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateLaunchTemplate, got {status} with \
         body {body}"
    );
    let parsed = document(case, &body);
    let launch_template_id = parsed
        .descendants()
        .filter(|node| node.is_element() && node.tag_name().name() == "launchTemplateId")
        .filter_map(|node| node.text().map(str::to_string))
        .next()
        .unwrap_or_else(|| panic!("case {case:?}: no launch template id in {body}"));
    for _ in 1..held {
        add_version(router, case, &launch_template_id).await;
    }
    launch_template_id
}

/// One more version of `launch_template_id`, as the version number it was given.
async fn add_version(router: &MultiServiceRouter, case: &str, launch_template_id: &str) -> i64 {
    let (status, body) = call(
        router,
        REGION,
        "CreateLaunchTemplateVersion",
        &[
            p("LaunchTemplateId", launch_template_id),
            p("LaunchTemplateData.InstanceType", INSTANCE_TYPE),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateLaunchTemplateVersion, got {status} \
         with body {body}"
    );
    let parsed = document(case, &body);
    parsed
        .descendants()
        .filter(|node| node.is_element() && node.tag_name().name() == "versionNumber")
        .filter_map(|node| node.text().and_then(|text| text.parse::<i64>().ok()))
        .next()
        .unwrap_or_else(|| panic!("case {case:?}: no version number in {body}"))
}

/// Make `version` the default version of `launch_template_id`.
async fn set_default_version(
    router: &MultiServiceRouter,
    case: &str,
    launch_template_id: &str,
    version: i64,
) {
    let (status, body) = call(
        router,
        REGION,
        "ModifyLaunchTemplate",
        &[
            p("LaunchTemplateId", launch_template_id),
            p("SetDefaultVersion", version.to_string()),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from ModifyLaunchTemplate, got {status} with \
         body {body}"
    );
}

/// One `DeleteLaunchTemplateVersions` call, as its status and body.
async fn delete_versions(
    router: &MultiServiceRouter,
    params: &[(String, String)],
) -> (u16, String) {
    call(router, REGION, "DeleteLaunchTemplateVersions", params).await
}

/// One reported item: the template's id and name, and the version number.
type Reported = (String, String, i64);

/// The `successfullyDeletedLaunchTemplateVersionSet` items of a response, in the order it
/// reported them.
fn deleted(case: &str, body: &str) -> Vec<Reported> {
    reported(case, body, "successfullyDeletedLaunchTemplateVersionSet")
        .into_iter()
        .map(|(item, _)| item)
        .collect()
}

/// The `unsuccessfullyDeletedLaunchTemplateVersionSet` items of a response, each with the
/// `responseError` code and message it carries, in the order it reported them.
#[allow(clippy::type_complexity)]
fn undeleted(case: &str, body: &str) -> Vec<(Reported, (String, String))> {
    reported(case, body, "unsuccessfullyDeletedLaunchTemplateVersionSet")
        .into_iter()
        .map(|(item, error)| {
            (
                item,
                error.unwrap_or_else(|| {
                    panic!("case {case:?}: an unsuccessful item must carry a responseError: {body}")
                }),
            )
        })
        .collect()
}

/// Every item of the response envelope's `set`, with the `responseError` code and message of
/// the ones that carry one.
#[allow(clippy::type_complexity)]
fn reported(case: &str, body: &str, set: &str) -> Vec<(Reported, Option<(String, String)>)> {
    let parsed = document(case, body);
    set_items(parsed.root_element(), set)
        .into_iter()
        .map(|item| {
            let version = child_text(item, "versionNumber")
                .and_then(|text| text.parse::<i64>().ok())
                .unwrap_or_else(|| {
                    panic!("case {case:?}: an item of {set} must carry a versionNumber: {body}")
                });
            let error = child(item, "responseError").map(|error| {
                (
                    child_text(error, "code").unwrap_or_default(),
                    child_text(error, "message").unwrap_or_default(),
                )
            });
            (
                (
                    child_text(item, "launchTemplateId").unwrap_or_default(),
                    child_text(item, "launchTemplateName").unwrap_or_default(),
                    version,
                ),
                error,
            )
        })
        .collect()
}

/// The version numbers `DescribeLaunchTemplateVersions` reports for `launch_template_id`,
/// smallest first.
async fn surviving_versions(
    router: &MultiServiceRouter,
    case: &str,
    launch_template_id: &str,
) -> Vec<i64> {
    let (status, body) = call(
        router,
        REGION,
        "DescribeLaunchTemplateVersions",
        &[p("LaunchTemplateId", launch_template_id)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeLaunchTemplateVersions, got {status} \
         with body {body}"
    );
    let parsed = document(case, &body);
    let mut numbers: Vec<i64> = parsed
        .descendants()
        .filter(|node| node.is_element() && node.tag_name().name() == "versionNumber")
        .filter_map(|node| node.text().and_then(|text| text.parse::<i64>().ok()))
        .collect();
    numbers.sort_unstable();
    numbers
}

/// The `defaultVersionNumber` and `latestVersionNumber` `DescribeLaunchTemplates` reports for
/// `launch_template_id`.
async fn template_counters(
    router: &MultiServiceRouter,
    case: &str,
    launch_template_id: &str,
) -> (i64, i64) {
    let (status, body) = call(
        router,
        REGION,
        "DescribeLaunchTemplates",
        &[p("LaunchTemplateId.1", launch_template_id)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeLaunchTemplates, got {status} with \
         body {body}"
    );
    let parsed = document(case, &body);
    let number = |name: &str| {
        parsed
            .descendants()
            .filter(|node| node.is_element() && node.tag_name().name() == name)
            .filter_map(|node| node.text().and_then(|text| text.parse::<i64>().ok()))
            .next()
            .unwrap_or_else(|| panic!("case {case:?}: no <{name}> in {body}"))
    };
    (number("defaultVersionNumber"), number("latestVersionNumber"))
}

/// Rule 1: every version the request names is reported, deleted or not, and the versions that
/// survive are exactly the ones no reported deletion took.
#[tokio::test]
async fn every_named_version_is_reported_and_the_survivors_follow() {
    // (case, versions the template holds, versions named, deleted, reported absent, survivors)
    let table = vec![
        ("one held version", 3, vec!["2"], vec![2], vec![], vec![1, 3]),
        (
            "a held version and one that never existed",
            3,
            vec!["3", "99"],
            vec![3],
            vec![99],
            vec![1, 2],
        ),
        (
            "only a version that never existed",
            2,
            vec!["42"],
            vec![],
            vec![42],
            vec![1, 2],
        ),
        (
            "two held versions, the newest named first",
            3,
            vec!["3", "2"],
            vec![3, 2],
            vec![],
            vec![1],
        ),
        (
            "the same number twice names one version",
            2,
            vec!["2", "2"],
            vec![2],
            vec![],
            vec![1],
        ),
        (
            "an absent version named before a held one",
            3,
            vec!["99", "2"],
            vec![2],
            vec![99],
            vec![1, 3],
        ),
        (
            "every version but the default",
            3,
            vec!["2", "3"],
            vec![2, 3],
            vec![],
            vec![1],
        ),
        (
            "a number below the first version",
            2,
            vec!["0"],
            vec![],
            vec![0],
            vec![1, 2],
        ),
        (
            "a negative number",
            2,
            vec!["-1"],
            vec![],
            vec![-1],
            vec![1, 2],
        ),
    ];

    let router = emulator();
    for (index, (case, held, named, expect_deleted, expect_absent, expect_survivors)) in
        table.iter().enumerate()
    {
        let name = format!("ltv-outcomes-{index}");
        let launch_template_id = template_with_versions(&router, case, &name, *held).await;
        let mut params = vec![p("LaunchTemplateId", launch_template_id.clone())];
        params.extend(versions(named));
        let (status, body) = delete_versions(&router, &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );

        let reported_deleted = deleted(case, &body);
        let expected_deleted: Vec<Reported> = expect_deleted
            .iter()
            .map(|version| (launch_template_id.clone(), name.clone(), *version))
            .collect();
        assert_eq!(
            reported_deleted, expected_deleted,
            "case {case:?}: expected the successful set {expected_deleted:?}, got \
             {reported_deleted:?} in {body}"
        );

        let reported_absent = undeleted(case, &body);
        let expected_absent: Vec<(Reported, (String, String))> = expect_absent
            .iter()
            .map(|version| {
                (
                    (launch_template_id.clone(), name.clone(), *version),
                    (
                        VERSION_DOES_NOT_EXIST_CODE.to_string(),
                        VERSION_DOES_NOT_EXIST_MESSAGE.to_string(),
                    ),
                )
            })
            .collect();
        assert_eq!(
            reported_absent, expected_absent,
            "case {case:?}: expected the unsuccessful set {expected_absent:?}, got \
             {reported_absent:?} in {body}"
        );

        let survivors = surviving_versions(&router, case, &launch_template_id).await;
        assert_eq!(
            &survivors, expect_survivors,
            "case {case:?}: expected the template to hold {expect_survivors:?} afterwards, \
             got {survivors:?}"
        );
    }
}

/// Rule 2: a request that names the default version is refused whole, and every version it
/// named survives.
#[tokio::test]
async fn the_default_version_is_never_deleted() {
    // (case, versions the template holds, which version is the default, versions named)
    let table = [
        ("the default is the first version", 2, 1, vec!["1"]),
        ("the default was moved to the latest", 2, 2, vec!["2"]),
        (
            "the default is named beside a version that would delete",
            3,
            2,
            vec!["3", "2"],
        ),
        (
            "the default is named beside one that never existed",
            2,
            1,
            vec!["99", "1"],
        ),
        (
            "the default is the only version the template holds",
            1,
            1,
            vec!["1"],
        ),
    ];

    let router = emulator();
    for (index, (case, held, default, named)) in table.iter().enumerate() {
        let name = format!("ltv-default-{index}");
        let launch_template_id = template_with_versions(&router, case, &name, *held).await;
        if *default != 1 {
            set_default_version(&router, case, &launch_template_id, *default).await;
        }
        let before = surviving_versions(&router, case, &launch_template_id).await;

        let mut params = vec![p("LaunchTemplateId", launch_template_id.clone())];
        params.extend(versions(named));
        let (status, body) = delete_versions(&router, &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValue",
            "case {case:?}: expected InvalidParameterValue, got {body}"
        );
        assert_eq!(
            error_message(&body),
            DEFAULT_VERSION_REFUSAL,
            "case {case:?}: expected the fixed refusal {DEFAULT_VERSION_REFUSAL:?}, got \
             {:?}",
            error_message(&body)
        );

        let after = surviving_versions(&router, case, &launch_template_id).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused request must delete nothing; expected {before:?}, got \
             {after:?}"
        );
        let (reported_default, _) = template_counters(&router, case, &launch_template_id).await;
        assert_eq!(
            reported_default, *default,
            "case {case:?}: expected version {default} to stay the default, got \
             {reported_default}"
        );
    }
}

/// Rule 3: the latest version follows the versions that remain, and a number a deleted
/// version carried is never minted again.
#[tokio::test]
async fn a_deleted_version_number_is_never_minted_again() {
    let case = "delete the latest version, then add one";
    let router = emulator();
    let name = "ltv-numbering";
    let launch_template_id = template_with_versions(&router, case, name, 3).await;

    let (status, body) = delete_versions(
        &router,
        &[
            p("LaunchTemplateId", launch_template_id.clone()),
            p("LaunchTemplateVersion.1", "3"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}"
    );

    // "The version number of the latest version of the launch template"
    // (`LaunchTemplate$LatestVersionNumber`) is the latest version it STILL holds.
    let (default_version, latest_version) =
        template_counters(&router, case, &launch_template_id).await;
    assert_eq!(
        (default_version, latest_version),
        (1, 2),
        "case {case:?}: expected the template to report default 1 and latest 2 after its \
         version 3 was deleted, got default {default_version} and latest {latest_version}"
    );

    let minted = add_version(&router, case, &launch_template_id).await;
    assert_eq!(
        minted, 4,
        "case {case:?}: expected the next version to be numbered 4 rather than reusing the \
         deleted 3, got {minted}"
    );
    let survivors = surviving_versions(&router, case, &launch_template_id).await;
    assert_eq!(
        survivors,
        vec![1, 2, 4],
        "case {case:?}: expected the template to hold versions 1, 2 and 4, got {survivors:?}"
    );
    let (_, latest_version) = template_counters(&router, case, &launch_template_id).await;
    assert_eq!(
        latest_version, 4,
        "case {case:?}: expected the latest version to be 4, got {latest_version}"
    );
}

/// Rule 1 again, by the other key: a request may name the template by name, and the items it
/// reports name it by both keys either way.
#[tokio::test]
async fn either_key_names_the_template() {
    let case = "the template named by name";
    let router = emulator();
    let name = "ltv-by-name";
    let launch_template_id = template_with_versions(&router, case, name, 2).await;

    let (status, body) = delete_versions(
        &router,
        &[
            p("LaunchTemplateName", name),
            p("LaunchTemplateVersion.1", "2"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}"
    );
    assert_eq!(
        deleted(case, &body),
        vec![(launch_template_id.clone(), name.to_string(), 2)],
        "case {case:?}: expected the deleted item to name the template by both keys, got \
         {body}"
    );
    let survivors = surviving_versions(&router, case, &launch_template_id).await;
    assert_eq!(
        survivors,
        vec![1],
        "case {case:?}: expected only version 1 to survive, got {survivors:?}"
    );
}

/// Rule 4: a request that names no one template, or something that is no version number,
/// deletes nothing — and says which parameter is wrong.
#[tokio::test]
async fn a_request_outside_the_parameters_deletes_nothing() {
    let router = emulator();
    let case = "the template the refusals are measured against";
    let name = "ltv-refusals";
    let launch_template_id = template_with_versions(&router, case, name, 2).await;
    let before = surviving_versions(&router, case, &launch_template_id).await;

    // A second template, deleted whole, so one row can name a launch template id the
    // account really minted and no longer holds.
    let deleted_template_id =
        template_with_versions(&router, case, "ltv-refusals-deleted", 2).await;
    let (status, body) = call(
        &router,
        REGION,
        "DeleteLaunchTemplate",
        &[p("LaunchTemplateId", deleted_template_id.clone())],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DeleteLaunchTemplate, got {status} with \
         body {body}"
    );

    // (case, parameters, expected status, expected code, expected message). A row whose
    // expected code is empty asserts the status alone: the bound row below is ANSWERED
    // rather than refused, and a request carrying no `Versions` member at all is refused by
    // the boundary validator — the member is `@required`, so that refusal is the framework's
    // and never reaches this operation's own reading of the list.
    let table = vec![
        (
            "neither the id nor the name".to_string(),
            versions(&["2"]),
            400,
            "MissingParameter",
            "The request must contain either the parameter LaunchTemplateId or the \
             parameter LaunchTemplateName"
                .to_string(),
        ),
        (
            "both the id and the name".to_string(),
            [
                vec![
                    p("LaunchTemplateId", launch_template_id.clone()),
                    p("LaunchTemplateName", name),
                ],
                versions(&["2"]),
            ]
            .concat(),
            400,
            "InvalidParameterCombination",
            "Either the launch template ID or the launch template name may be specified, \
             but not both"
                .to_string(),
        ),
        (
            "an id that is no launch template id".to_string(),
            [vec![p("LaunchTemplateId", "lt-nothex")], versions(&["2"])].concat(),
            400,
            "InvalidLaunchTemplateId.Malformed",
            "Malformed launch template Id: lt-nothex".to_string(),
        ),
        (
            "a well-formed id no mint of this service ever reached".to_string(),
            [
                vec![p("LaunchTemplateId", "lt-0123456789abcdef0")],
                versions(&["2"]),
            ]
            .concat(),
            400,
            "InvalidLaunchTemplateId.Malformed",
            "Malformed launch template Id: lt-0123456789abcdef0".to_string(),
        ),
        (
            "the id of a template that has since been deleted".to_string(),
            [
                vec![p("LaunchTemplateId", deleted_template_id.clone())],
                versions(&["2"]),
            ]
            .concat(),
            400,
            "InvalidLaunchTemplateId.NotFound",
            format!(
                "The specified launch template, with template ID {deleted_template_id}, \
                 does not exist."
            ),
        ),
        (
            "a name no template carries".to_string(),
            [
                vec![p("LaunchTemplateName", "ltv-no-such-template")],
                versions(&["2"]),
            ]
            .concat(),
            400,
            "InvalidLaunchTemplateName.NotFoundException",
            "At least one of the launch templates specified in the request does not exist."
                .to_string(),
        ),
        (
            "no version at all".to_string(),
            vec![p("LaunchTemplateId", launch_template_id.clone())],
            400,
            "",
            String::new(),
        ),
        (
            "the $Latest alias".to_string(),
            [
                vec![p("LaunchTemplateId", launch_template_id.clone())],
                versions(&["$Latest"]),
            ]
            .concat(),
            400,
            "InvalidParameterValue",
            "Invalid value '$Latest' for LaunchTemplateVersion. Valid values are version \
             numbers"
                .to_string(),
        ),
        (
            "the $Default alias".to_string(),
            [
                vec![p("LaunchTemplateId", launch_template_id.clone())],
                versions(&["$Default"]),
            ]
            .concat(),
            400,
            "InvalidParameterValue",
            "Invalid value '$Default' for LaunchTemplateVersion. Valid values are version \
             numbers"
                .to_string(),
        ),
        (
            "a word that is no version".to_string(),
            [
                vec![p("LaunchTemplateId", launch_template_id.clone())],
                versions(&["newest"]),
            ]
            .concat(),
            400,
            "InvalidParameterValue",
            "Invalid value 'newest' for version. Valid values are $Latest, $Default, and \
             version numbers"
                .to_string(),
        ),
        (
            format!("{MAX_DELETED_VERSIONS} versions, the most a request may name"),
            [
                vec![p("LaunchTemplateId", launch_template_id.clone())],
                (1..=MAX_DELETED_VERSIONS)
                    .map(|index| {
                        p(
                            &format!("LaunchTemplateVersion.{index}"),
                            // Past the versions this template holds, so the request is
                            // ANSWERED rather than deleting anything: the bound itself is
                            // what this row is about.
                            (index + 1_000).to_string(),
                        )
                    })
                    .collect(),
            ]
            .concat(),
            200,
            "",
            String::new(),
        ),
        (
            format!("{} versions, one past the bound", MAX_DELETED_VERSIONS + 1),
            [
                vec![p("LaunchTemplateId", launch_template_id.clone())],
                (1..=MAX_DELETED_VERSIONS + 1)
                    .map(|index| {
                        p(
                            &format!("LaunchTemplateVersion.{index}"),
                            (index + 1_000).to_string(),
                        )
                    })
                    .collect(),
            ]
            .concat(),
            400,
            "InvalidParameterValue",
            format!(
                "Value ({}) for parameter LaunchTemplateVersion is invalid. The maximum \
                 number of launch template versions that may be deleted in one request is \
                 {MAX_DELETED_VERSIONS}",
                MAX_DELETED_VERSIONS + 1
            ),
        ),
    ];

    for (case, params, expected_status, code, message) in &table {
        let (status, body) = delete_versions(&router, params).await;
        assert_eq!(
            status, *expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        // A row with no expected code asserts the status alone: the row at the published
        // bound is ANSWERED rather than refused, and the row that carries no `Versions`
        // member is refused before this operation reads anything.
        if !code.is_empty() {
            assert_eq!(
                error_code(&body),
                *code,
                "case {case:?}: expected the code {code:?}, got {body}"
            );
            assert_eq!(
                &error_message(&body),
                message,
                "case {case:?}: expected the message {message:?}, got {:?}",
                error_message(&body)
            );
        }
        let after = surviving_versions(&router, case, &launch_template_id).await;
        assert_eq!(
            after, before,
            "case {case:?}: the request must delete nothing; expected {before:?}, got {after:?}"
        );
    }
}

/// Rule 4 again, for the permission check: "checks whether you have the required permissions
/// for the action, WITHOUT ACTUALLY MAKING THE REQUEST … the error response is
/// DryRunOperation" (`DryRun`), so a dry run of a delete that would have succeeded deletes
/// nothing, and one that would have been refused is refused with ITS reason instead.
#[tokio::test]
async fn a_dry_run_deletes_nothing() {
    let router = emulator();
    let case = "a dry run of a delete that would have succeeded";
    let name = "ltv-dry-run";
    let launch_template_id = template_with_versions(&router, case, name, 2).await;

    let (status, body) = delete_versions(
        &router,
        &[
            p("LaunchTemplateId", launch_template_id.clone()),
            p("LaunchTemplateVersion.1", "2"),
            p("DryRun", "true"),
        ],
    )
    .await;
    assert_eq!(
        status, 412,
        "case {case:?}: expected HTTP 412, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "DryRunOperation",
        "case {case:?}: expected DryRunOperation, got {body}"
    );
    let survivors = surviving_versions(&router, case, &launch_template_id).await;
    assert_eq!(
        survivors,
        vec![1, 2],
        "case {case:?}: expected both versions to survive the dry run, got {survivors:?}"
    );

    let case = "a dry run of a delete that names the default version";
    let (status, body) = delete_versions(
        &router,
        &[
            p("LaunchTemplateId", launch_template_id.clone()),
            p("LaunchTemplateVersion.1", "1"),
            p("DryRun", "true"),
        ],
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidParameterValue",
        "case {case:?}: expected the default-version refusal rather than the dry-run \
         answer, got {body}"
    );
}
