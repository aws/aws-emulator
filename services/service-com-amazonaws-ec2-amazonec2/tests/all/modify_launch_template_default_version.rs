// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `ModifyLaunchTemplate`, driven over the wire the service speaks
//! (`ec2Query`) through the crate's shared [`super::ec2_query`] harness.
//!
//! The rules under test:
//!
//! 1. `ModifyLaunchTemplate` MOVES THE TEMPLATE'S DEFAULT VERSION to the version the request
//!    names: "you can specify which version of the launch template to set as the default
//!    version" (`ModifyLaunchTemplate`), and nothing else about the template changes — its
//!    id, its name and its `LatestVersionNumber` stay where they were. The template it
//!    reports is the STORED one, so `DescribeLaunchTemplates` reports the same
//!    `DefaultVersionNumber` afterwards and `DescribeLaunchTemplateVersions` marks exactly
//!    that version as the default one;
//! 2. the version a request names is a number or one of the aliases the launch template API
//!    documents, "$Latest, $Default, and numbers"
//!    (`DescribeLaunchTemplateVersionsRequest$Versions`), resolved against the template that
//!    holds them;
//! 3. the template's `Operator` is the ANSWER to whether a service provider manages it:
//!    "describes whether the resource is managed by a service provider and, if so, describes
//!    the service provider that manages it" (`OperatorResponse`), so a template nobody
//!    manages reports `managed` false and names no principal, and a managed one keeps the
//!    principal it was created with across a modify;
//! 4. a request that cannot be served is refused with EC2's own code and moves nothing, and
//!    a `DryRun` of one that would have succeeded performs nothing.
//!
//! Each table row is a distinct input along what the rules generalise over — which member
//! names the template, whether the version is named by number or by alias, which version of
//! the ones held it names (the first, one in the middle, the latest, the one already
//! default), and which part of a request cannot be served — so no row is the specification.

use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use ec2::state::FIRST_LAUNCH_TEMPLATE_VERSION;

use super::ec2_query::{call, element, error_code};
use super::wire::{child, child_text, document, reported_operator};

/// The Region every call here is signed for. A launch template carries no Region member, so
/// the Region only selects the ec2Query envelope.
const REGION: &str = "us-east-1";

/// How many versions the template under test holds: enough that the first, a middle one and
/// the latest are three different versions.
const VERSIONS_HELD: i64 = 4;

/// One row of the refusal table: what it covers, the parameters it sends, the HTTP status
/// and the error code the request must be refused with.
type RefusalRow = (&'static str, Vec<(String, String)>, u16, &'static str);

/// One ec2Query call through the shared harness.
async fn api(
    router: &MultiServiceRouter,
    action: &str,
    params: &[(String, String)],
) -> (u16, String) {
    call(router, REGION, action, params).await
}

/// A `(key, value)` request parameter.
fn p(key: &str, value: &str) -> (String, String) {
    (key.to_string(), value.to_string())
}

/// One `LaunchTemplateData.<member>` parameter.
fn data(member: &str, value: &str) -> (String, String) {
    p(&format!("LaunchTemplateData.{member}"), value)
}

/// One call that has to succeed, as the body it answered.
async fn ok(
    case: &str,
    router: &MultiServiceRouter,
    action: &str,
    params: &[(String, String)],
) -> String {
    let (status, body) = api(router, action, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {action}, got {status} with body {body}"
    );
    body
}

/// A launch template named `name` holding [`VERSIONS_HELD`] versions, as its id.
async fn template_with_versions(case: &str, router: &MultiServiceRouter, name: &str) -> String {
    let body = ok(
        case,
        router,
        "CreateLaunchTemplate",
        &[
            p("LaunchTemplateName", name),
            data("InstanceType", "t3.micro"),
        ],
    )
    .await;
    let launch_template_id = element(&body, "launchTemplateId")
        .unwrap_or_else(|| panic!("case {case:?}: the create must report the id it minted: {body}"));
    for number in (FIRST_LAUNCH_TEMPLATE_VERSION + 1)..=VERSIONS_HELD {
        let body = ok(
            case,
            router,
            "CreateLaunchTemplateVersion",
            &[
                p("LaunchTemplateId", &launch_template_id),
                data("InstanceType", "t3.small"),
            ],
        )
        .await;
        assert_eq!(
            element(&body, "versionNumber"),
            Some(number.to_string()),
            "case {case:?}: expected the create to store version {number}, got {body}"
        );
    }
    launch_template_id
}

/// The `(defaultVersionNumber, latestVersionNumber)` `DescribeLaunchTemplates` reports for
/// `launch_template_id` — read back through the template's own reader rather than off the
/// modify's own result.
async fn described_counters(
    case: &str,
    router: &MultiServiceRouter,
    launch_template_id: &str,
) -> (Option<String>, Option<String>) {
    let body = ok(
        case,
        router,
        "DescribeLaunchTemplates",
        &[p("LaunchTemplateId.1", launch_template_id)],
    )
    .await;
    (
        element(&body, "defaultVersionNumber"),
        element(&body, "latestVersionNumber"),
    )
}

/// The version numbers `DescribeLaunchTemplateVersions` marks as the template's DEFAULT
/// version — the per-version side of the same fact the template's `DefaultVersionNumber`
/// reports, so a correct modify leaves exactly one entry here.
async fn versions_flagged_default(
    case: &str,
    router: &MultiServiceRouter,
    launch_template_id: &str,
) -> Vec<i64> {
    let body = ok(
        case,
        router,
        "DescribeLaunchTemplateVersions",
        &[p("LaunchTemplateId", launch_template_id)],
    )
    .await;
    let parsed = document(case, &body);
    let mut flagged: Vec<i64> = parsed
        .descendants()
        .filter(|node| node.is_element() && child(*node, "versionNumber").is_some())
        .filter(|node| child_text(*node, "defaultVersion").as_deref() == Some("true"))
        .filter_map(|node| {
            child_text(node, "versionNumber").and_then(|number| number.parse::<i64>().ok())
        })
        .collect();
    flagged.sort_unstable();
    flagged
}

/// RULES 1 and 2: each modify moves the template's default version to the version it names —
/// by number or by alias — leaves everything else about the template where it was, and is
/// what both readers of the template report afterwards.
///
/// The rows differ along which member names the template, and which of the versions held the
/// request names and how: the latest by number, the latest by alias, the first by number, the
/// one already default by alias, a middle one, and that same one again.
#[tokio::test]
async fn a_modify_moves_the_default_version_to_the_version_it_names() {
    let (router, _engine) = super::ec2_query::emulator();
    let name = "modify.default.version";
    let case = "a template holding four versions";
    let launch_template_id = template_with_versions(case, &router, name).await;

    // The template starts out with its first version as the default one, which is what the
    // create stored: a later version is not the default until something makes it so.
    let counters = described_counters(case, &router, &launch_template_id).await;
    assert_eq!(
        counters,
        (
            Some(FIRST_LAUNCH_TEMPLATE_VERSION.to_string()),
            Some(VERSIONS_HELD.to_string())
        ),
        "case {case:?}: expected the template to default to version \
         {FIRST_LAUNCH_TEMPLATE_VERSION} with {VERSIONS_HELD} as its latest, got {counters:?}"
    );

    // Rows: how the template is named, the `SetDefaultVersion` the request carries, and the
    // version number the template must then default to.
    let rows: Vec<(&str, bool, &str, i64)> = vec![
        ("the latest version by number, named by id", true, "4", 4),
        ("the version already default, by the $Default alias", true, "$Default", 4),
        ("the first version by number, named by name", false, "1", 1),
        ("the latest version by the $Latest alias, named by name", false, "$Latest", 4),
        ("a version in the middle by number, named by id", true, "2", 2),
        ("that same version again, which is already default", true, "2", 2),
    ];

    for (case, by_id, set_default_version, expected) in rows {
        let params = vec![
            if by_id {
                p("LaunchTemplateId", &launch_template_id)
            } else {
                p("LaunchTemplateName", name)
            },
            p("SetDefaultVersion", set_default_version),
        ];
        let body = ok(case, &router, "ModifyLaunchTemplate", &params).await;

        // The modify reports the template as it now stands: the default version it was asked
        // for, and the id, the name and the latest version it already had.
        let reported = (
            element(&body, "launchTemplateId"),
            element(&body, "launchTemplateName"),
            element(&body, "defaultVersionNumber"),
            element(&body, "latestVersionNumber"),
        );
        let wanted = (
            Some(launch_template_id.clone()),
            Some(name.to_string()),
            Some(expected.to_string()),
            Some(VERSIONS_HELD.to_string()),
        );
        assert_eq!(
            reported, wanted,
            "case {case:?}: expected (id, name, defaultVersionNumber, latestVersionNumber) \
             {wanted:?}, got {reported:?} from body {body}"
        );

        // The template the readers report is the same one: the counters it carries, and the
        // one version flagged as the default on the version side.
        let counters = described_counters(case, &router, &launch_template_id).await;
        assert_eq!(
            counters,
            (
                Some(expected.to_string()),
                Some(VERSIONS_HELD.to_string())
            ),
            "case {case:?}: expected the stored template to default to version {expected} \
             with {VERSIONS_HELD} as its latest, got {counters:?}"
        );
        let flagged = versions_flagged_default(case, &router, &launch_template_id).await;
        assert_eq!(
            flagged,
            vec![expected],
            "case {case:?}: expected version {expected} alone to report defaultVersion true, \
             got {flagged:?}"
        );

        // RULE 3: nothing manages a template this account created for itself, which is an
        // ANSWER about it rather than something unknown, so the operator reports it.
        let operator = reported_operator(case, &body);
        assert_eq!(
            operator,
            (Some("false".to_string()), None),
            "case {case:?}: expected the operator of an unmanaged template to report \
             (managed, principal) (Some(\"false\"), None), got {operator:?} from body {body}"
        );
    }

    // RULE 3's other half: a template a service provider manages reports that provider, and
    // a modify of it reports the same operator the create did.
    let principal = "autoscaling.amazonaws.com";
    let case = "a template a service provider manages";
    let body = ok(
        case,
        &router,
        "CreateLaunchTemplate",
        &[
            p("LaunchTemplateName", "modify.default.managed"),
            p("Operator.Principal", principal),
            data("InstanceType", "t3.micro"),
        ],
    )
    .await;
    let managed_id = element(&body, "launchTemplateId")
        .unwrap_or_else(|| panic!("case {case:?}: the create must report the id it minted: {body}"));
    let body = ok(
        case,
        &router,
        "ModifyLaunchTemplate",
        &[
            p("LaunchTemplateId", &managed_id),
            p("SetDefaultVersion", "$Latest"),
        ],
    )
    .await;
    let operator = reported_operator(case, &body);
    assert_eq!(
        operator,
        (Some("true".to_string()), Some(principal.to_string())),
        "case {case:?}: expected the operator to report (managed, principal) \
         (Some(\"true\"), Some({principal:?})), got {operator:?} from body {body}"
    );
}

/// RULE 4: a request that cannot be served is refused with EC2's own code and moves nothing,
/// and a `DryRun` of one that would have succeeded performs nothing.
///
/// The rows differ along WHICH part of the request cannot be served, not along the value of
/// one of them.
#[tokio::test]
async fn a_modify_that_cannot_be_served_is_refused_and_moves_nothing() {
    let (router, _engine) = super::ec2_query::emulator();
    let name = "modify.default.refusals";
    let case = "a template to refuse against";
    let launch_template_id = template_with_versions(case, &router, name).await;

    // Something to move away from, so a refusal that moved the default version would show.
    let settled = 3;
    ok(
        "settle the default version",
        &router,
        "ModifyLaunchTemplate",
        &[
            p("LaunchTemplateId", &launch_template_id),
            p("SetDefaultVersion", &settled.to_string()),
        ],
    )
    .await;

    let rows: Vec<RefusalRow> = vec![
        (
            "neither the id nor the name",
            vec![p("SetDefaultVersion", "1")],
            400,
            "MissingParameter",
        ),
        (
            "both the id and the name",
            vec![
                p("LaunchTemplateId", &launch_template_id),
                p("LaunchTemplateName", name),
                p("SetDefaultVersion", "1"),
            ],
            400,
            "InvalidParameterCombination",
        ),
        (
            "a value outside the launch template id form",
            vec![p("LaunchTemplateId", "not-an-id"), p("SetDefaultVersion", "1")],
            400,
            "InvalidLaunchTemplateId.Malformed",
        ),
        (
            "a well-formed id no mint of this service ever reached",
            vec![
                p("LaunchTemplateId", "lt-0ffffffffffffffff"),
                p("SetDefaultVersion", "1"),
            ],
            400,
            "InvalidLaunchTemplateId.Malformed",
        ),
        (
            "a name no template of the account carries",
            vec![
                p("LaunchTemplateName", "modify.default.absent"),
                p("SetDefaultVersion", "1"),
            ],
            400,
            "InvalidLaunchTemplateName.NotFoundException",
        ),
        (
            "no version to set as the default at all",
            vec![p("LaunchTemplateId", &launch_template_id)],
            400,
            "MissingParameter",
        ),
        (
            "a version number the template does not hold",
            vec![
                p("LaunchTemplateId", &launch_template_id),
                p("SetDefaultVersion", "9"),
            ],
            400,
            "InvalidLaunchTemplateId.VersionNotFound",
        ),
        (
            "a version number below the first one",
            vec![
                p("LaunchTemplateId", &launch_template_id),
                p("SetDefaultVersion", "0"),
            ],
            400,
            "InvalidLaunchTemplateId.VersionNotFound",
        ),
        (
            "a version that is neither a number nor an alias",
            vec![
                p("LaunchTemplateId", &launch_template_id),
                p("SetDefaultVersion", "$Newest"),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            "a client token longer than the documented maximum",
            vec![
                p("LaunchTemplateId", &launch_template_id),
                p("SetDefaultVersion", "1"),
                p("ClientToken", &"t".repeat(129)),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            // The flag asks whether the request would have succeeded; this one would have,
            // so it is reported as such and performs nothing.
            "a dry run of a request that would have succeeded",
            vec![
                p("LaunchTemplateId", &launch_template_id),
                p("SetDefaultVersion", "1"),
                p("DryRun", "true"),
            ],
            412,
            "DryRunOperation",
        ),
    ];

    for (case, params, expected_status, expected_code) in rows {
        let (status, body) = api(&router, "ModifyLaunchTemplate", &params).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, expected_code,
            "case {case:?}: expected the error code {expected_code:?}, got {code:?} in \
             body {body}"
        );
        let counters = described_counters(case, &router, &launch_template_id).await;
        assert_eq!(
            counters,
            (
                Some(settled.to_string()),
                Some(VERSIONS_HELD.to_string())
            ),
            "case {case:?}: a refused modify leaves the default version at {settled}, \
             got {counters:?}"
        );
        let flagged = versions_flagged_default(case, &router, &launch_template_id).await;
        assert_eq!(
            flagged,
            vec![settled],
            "case {case:?}: a refused modify leaves version {settled} alone flagged as the \
             default one, got {flagged:?}"
        );
    }
}
