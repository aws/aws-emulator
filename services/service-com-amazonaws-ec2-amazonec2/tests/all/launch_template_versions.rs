// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `CreateLaunchTemplateVersion` and the versions
//! `DescribeLaunchTemplateVersions` reports for a template that holds several, driven over
//! the wire the service speaks (`ec2Query`) through the crate's shared [`super::ec2_query`]
//! harness.
//!
//! The rules under test:
//!
//! 1. `CreateLaunchTemplateVersion` STORES ONE VERSION of a template the account already
//!    holds: "launch template versions are numbered in the order in which they are created"
//!    (`CreateLaunchTemplateVersion`), so the number is the template's latest plus one, the
//!    new version becomes the template's `LatestVersionNumber` and the template's DEFAULT
//!    version is left where it is. The launch parameters it reports are the ones it stored,
//!    which is what the describe reads back for that version;
//! 2. `SourceVersion` inherits: "the new version inherits the launch parameters from the
//!    source version … If you specify additional launch parameters for the new version, they
//!    overwrite any corresponding launch parameters inherited from the source version"
//!    (`CreateLaunchTemplateVersionRequest$SourceVersion`);
//! 3. every version a template holds is reported by one describe, NEWEST FIRST, each with
//!    its own launch parameters;
//! 4. a request that cannot be served is refused with EC2's own code and stores nothing,
//!    `DryRun` performs nothing, and one `ClientToken` creates one version.
//!
//! Each table row is a distinct input along what the rules generalise over — how many
//! versions have been created, which launch parameter the new version sets, whether it
//! inherits and from which version (by number or by alias), which member names the template,
//! and which part of a request cannot be served — so no row is the specification.

use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use ec2::state::{ComAmazonawsEc2Amazonec2State as Ec2State, FIRST_LAUNCH_TEMPLATE_VERSION};

use super::ec2_query::{call, element, elements, error_code};
use super::wire::{child, child_text, document};

/// The Region every call here is signed for. A launch template carries no Region member, so
/// the Region only selects the ec2Query envelope.
const REGION: &str = "us-east-1";

/// One row of the refusal table: what it covers, the parameters it sends, and the error code
/// the request must be refused with.
type RefusalRow = (&'static str, Vec<(String, String)>, &'static str);

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

/// Create one launch template holding `data` as its first version, and answer its id.
async fn create_template(
    case: &str,
    router: &MultiServiceRouter,
    name: &str,
    first_version: &[(String, String)],
) -> String {
    let mut params = vec![p("LaunchTemplateName", name)];
    params.extend(first_version.iter().cloned());
    let body = ok(case, router, "CreateLaunchTemplate", &params).await;
    element(&body, "launchTemplateId")
        .unwrap_or_else(|| panic!("case {case:?}: the create must report the id it minted: {body}"))
}

/// One `CreateLaunchTemplateVersion` call.
async fn create_version(
    router: &MultiServiceRouter,
    params: &[(String, String)],
) -> (u16, String) {
    api(router, "CreateLaunchTemplateVersion", params).await
}

/// The version numbers a describe body reports, in the order the body carries them.
fn reported_versions(body: &str) -> Vec<i64> {
    elements(body, "versionNumber")
        .into_iter()
        .map(|number| number.parse::<i64>().unwrap_or_default())
        .collect()
}

/// Every version of `launch_template_id` the describe reports, as
/// `(versionNumber, defaultVersion, imageId, instanceType)` — the members the rows below
/// assert over.
async fn described_versions(
    case: &str,
    router: &MultiServiceRouter,
    launch_template_id: &str,
) -> Vec<(i64, String, Option<String>, Option<String>)> {
    let body = ok(
        case,
        router,
        "DescribeLaunchTemplateVersions",
        &[p("LaunchTemplateId", launch_template_id)],
    )
    .await;
    let parsed = document(case, &body);
    parsed
        .descendants()
        .filter(|node| node.is_element() && child(*node, "versionNumber").is_some())
        .map(|node| {
            let launch_data = child(node, "launchTemplateData");
            (
                child_text(node, "versionNumber")
                    .and_then(|number| number.parse::<i64>().ok())
                    .unwrap_or_default(),
                child_text(node, "defaultVersion").unwrap_or_default(),
                launch_data.and_then(|data| child_text(data, "imageId")),
                launch_data.and_then(|data| child_text(data, "instanceType")),
            )
        })
        .collect()
}

/// The `(defaultVersionNumber, latestVersionNumber)` `DescribeLaunchTemplates` reports for
/// `launch_template_id` — the counters the create moves, read back through the template's own
/// reader rather than off the create's result.
async fn template_counters(
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

/// RULES 1 and 3: each create stores the NEXT version of the template, moves its latest
/// version number and leaves its default alone, and one describe reports every version it
/// holds — newest first, each with the launch parameters it was created with.
///
/// The rows differ along how many versions the template already holds and which launch
/// parameter the new version sets.
#[tokio::test]
async fn each_create_stores_the_next_version_and_the_describe_reports_them_newest_first() {
    let (router, _engine) = super::ec2_query::emulator();
    let case = "versions created one after another";
    let launch_template_id = create_template(
        case,
        &router,
        "versions.sequence",
        &[
            data("ImageId", "ami-00000000000000001"),
            data("InstanceType", "t3.micro"),
        ],
    )
    .await;

    // One row per version created after the first: the instance type it sets, and the image
    // id it sets (none on the last row, which therefore stores no image at all, inheriting
    // nothing without a `SourceVersion`).
    let rows: Vec<(&str, Option<&str>, &str)> = vec![
        ("the second version", Some("ami-00000000000000002"), "t3.small"),
        ("the third version", Some("ami-00000000000000003"), "m5.large"),
        ("the fourth version, with no image id", None, "c5.xlarge"),
    ];

    let mut expected: Vec<(i64, String, Option<String>, Option<String>)> = vec![(
        FIRST_LAUNCH_TEMPLATE_VERSION,
        "true".to_string(),
        Some("ami-00000000000000001".to_string()),
        Some("t3.micro".to_string()),
    )];

    for (index, (case, image_id, instance_type)) in rows.iter().enumerate() {
        let number = FIRST_LAUNCH_TEMPLATE_VERSION + i64::try_from(index).unwrap_or_default() + 1;
        let mut params = vec![
            p("LaunchTemplateId", &launch_template_id),
            data("InstanceType", instance_type),
        ];
        if let Some(image_id) = image_id {
            params.push(data("ImageId", image_id));
        }
        let body = ok(case, &router, "CreateLaunchTemplateVersion", &params).await;

        // The create reports the version it stored: its number, that it is not the default
        // one, and the launch parameters it was given.
        assert_eq!(
            element(&body, "versionNumber"),
            Some(number.to_string()),
            "case {case:?}: expected the create to report version {number}, got {body}"
        );
        assert_eq!(
            element(&body, "defaultVersion"),
            Some("false".to_string()),
            "case {case:?}: a new version is not the default one, got {body}"
        );
        assert_eq!(
            element(&body, "instanceType"),
            Some((*instance_type).to_string()),
            "case {case:?}: expected the instance type the create carried, got {body}"
        );
        assert_eq!(
            element(&body, "imageId"),
            image_id.map(str::to_string),
            "case {case:?}: expected the image id the create carried, got {body}"
        );
        assert_eq!(
            element(&body, "launchTemplateId"),
            Some(launch_template_id.clone()),
            "case {case:?}: expected the version to name its template, got {body}"
        );
        // "If true, the resource is managed by a service provider"
        // (`OperatorResponse$Managed`): nothing manages a template a caller created, which
        // is a fact about it and not something unknown, so the member is reported false.
        assert_eq!(
            element(&body, "managed"),
            Some("false".to_string()),
            "case {case:?}: expected the version's operator to report managed false, \
             got {body}"
        );

        // The template's latest version moved to the one just created; its default did not.
        let counters = template_counters(case, &router, &launch_template_id).await;
        assert_eq!(
            counters,
            (
                Some(FIRST_LAUNCH_TEMPLATE_VERSION.to_string()),
                Some(number.to_string())
            ),
            "case {case:?}: expected the latest version to move to {number} and the default \
             to stay at {FIRST_LAUNCH_TEMPLATE_VERSION}, got {counters:?}"
        );

        // Every version held so far is reported, newest first.
        expected.insert(
            0,
            (
                number,
                "false".to_string(),
                image_id.map(str::to_string),
                Some((*instance_type).to_string()),
            ),
        );
        let reported = described_versions(case, &router, &launch_template_id).await;
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the versions {expected:?}, newest first, got {reported:?}"
        );
    }
}

/// RULE 2: `SourceVersion` inherits the source's launch parameters, and a parameter the
/// request specifies overwrites the inherited one.
///
/// The rows differ along which version is the source (by number, by `$Latest`, by
/// `$Default`), which parameter the request overwrites and which it leaves to be inherited.
#[tokio::test]
async fn a_source_version_is_inherited_and_specified_parameters_overwrite_it() {
    let (router, _engine) = super::ec2_query::emulator();
    let case = "a template to inherit from";
    let launch_template_id = create_template(
        case,
        &router,
        "versions.inherit",
        &[
            data("ImageId", "ami-00000000000000001"),
            data("InstanceType", "t3.micro"),
            data("KernelId", "aki-00000000000000001"),
        ],
    )
    .await;
    // Version 2 carries launch parameters of its own, so a row sourcing `$Latest` inherits
    // something different from one sourcing version 1.
    let body = ok(
        "the second version",
        &router,
        "CreateLaunchTemplateVersion",
        &[
            p("LaunchTemplateId", &launch_template_id),
            data("ImageId", "ami-00000000000000002"),
            data("InstanceType", "t3.small"),
        ],
    )
    .await;
    assert_eq!(
        element(&body, "versionNumber"),
        Some("2".to_string()),
        "the second version must be numbered 2, got {body}"
    );

    // Rows: the source named, the parameters the new version specifies, and the
    // `(imageId, instanceType, kernelId)` the stored version must report.
    type InheritRow = (
        &'static str,
        &'static str,
        Vec<(String, String)>,
        (Option<&'static str>, Option<&'static str>, Option<&'static str>),
    );
    let rows: Vec<InheritRow> = vec![
        (
            // FIRST, because every row below creates another version and so moves what
            // `$Latest` names: here it is still version 2, which carries no kernel.
            "the $Latest alias, which carries no kernel to inherit",
            "$Latest",
            vec![data("InstanceType", "m5.large")],
            (Some("ami-00000000000000002"), Some("m5.large"), None),
        ),
        (
            "version 1 by number, nothing overwritten",
            "1",
            vec![data("EbsOptimized", "true")],
            (
                Some("ami-00000000000000001"),
                Some("t3.micro"),
                Some("aki-00000000000000001"),
            ),
        ),
        (
            "version 1 by number, the image id overwritten",
            "1",
            vec![data("ImageId", "ami-00000000000000009")],
            (
                Some("ami-00000000000000009"),
                Some("t3.micro"),
                Some("aki-00000000000000001"),
            ),
        ),
        (
            "version 2 by number, which carries no kernel",
            "2",
            vec![data("InstanceType", "r5.xlarge")],
            (Some("ami-00000000000000002"), Some("r5.xlarge"), None),
        ),
        (
            "the $Default alias, which is still version 1",
            "$Default",
            vec![data("InstanceType", "c5.large")],
            (
                Some("ami-00000000000000001"),
                Some("c5.large"),
                Some("aki-00000000000000001"),
            ),
        ),
    ];

    for (case, source_version, specified, expected) in rows {
        let mut params = vec![
            p("LaunchTemplateId", &launch_template_id),
            p("SourceVersion", source_version),
        ];
        params.extend(specified);
        let body = ok(case, &router, "CreateLaunchTemplateVersion", &params).await;
        let reported = (
            element(&body, "imageId"),
            element(&body, "instanceType"),
            element(&body, "kernelId"),
        );
        let expected = (
            expected.0.map(str::to_string),
            expected.1.map(str::to_string),
            expected.2.map(str::to_string),
        );
        assert_eq!(
            reported, expected,
            "case {case:?}: expected (imageId, instanceType, kernelId) {expected:?}, \
             got {reported:?} from body {body}"
        );
    }

    // A version created WITHOUT a source inherits nothing, however many versions precede it.
    let body = ok(
        "no source version",
        &router,
        "CreateLaunchTemplateVersion",
        &[
            p("LaunchTemplateId", &launch_template_id),
            data("InstanceType", "r5.large"),
        ],
    )
    .await;
    assert_eq!(
        (element(&body, "imageId"), element(&body, "kernelId")),
        (None, None),
        "case \"no source version\": a version that names no source inherits nothing, \
         got {body}"
    );
}

/// RULE 1's other half, and RULE 4: the template a request names has to resolve, a version
/// that cannot be served is refused with EC2's own code and stores nothing, a `DryRun`
/// performs nothing, and one `ClientToken` creates one version.
#[tokio::test]
async fn a_create_that_cannot_be_served_is_refused_and_stores_nothing() {
    let (router, engine) = super::ec2_query::emulator();
    let case = "a template to refuse against";
    let launch_template_id = create_template(
        case,
        &router,
        "versions.refusals",
        &[data("ImageId", "ami-00000000000000001")],
    )
    .await;

    let held = |engine: &engine::InMemoryEngine| -> usize {
        engine
            .state::<Ec2State>()
            .lock()
            .launch_templates
            .get(&launch_template_id)
            .map_or(0, |record| record.versions.len())
    };
    assert_eq!(
        held(&engine),
        1,
        "the template starts with the one version its create stored"
    );

    // Rows: WHICH part of the request cannot be served, not the value of one of them.
    let rows: Vec<RefusalRow> = vec![
        (
            "neither the id nor the name",
            vec![data("ImageId", "ami-00000000000000002")],
            "MissingParameter",
        ),
        (
            "both the id and the name",
            vec![
                p("LaunchTemplateId", &launch_template_id),
                p("LaunchTemplateName", "versions.refusals"),
                data("ImageId", "ami-00000000000000002"),
            ],
            "InvalidParameterCombination",
        ),
        (
            "a value outside the launch template id form",
            vec![
                p("LaunchTemplateId", "not-an-id"),
                data("ImageId", "ami-00000000000000002"),
            ],
            "InvalidLaunchTemplateId.Malformed",
        ),
        (
            "a well-formed id no mint of this service ever reached",
            vec![
                p("LaunchTemplateId", "lt-0ffffffffffffffff"),
                data("ImageId", "ami-00000000000000002"),
            ],
            "InvalidLaunchTemplateId.Malformed",
        ),
        (
            "a name no template of the account carries",
            vec![
                p("LaunchTemplateName", "versions.absent"),
                data("ImageId", "ami-00000000000000002"),
            ],
            "InvalidLaunchTemplateName.NotFoundException",
        ),
        (
            "a source version the template does not hold",
            vec![
                p("LaunchTemplateId", &launch_template_id),
                p("SourceVersion", "9"),
                data("ImageId", "ami-00000000000000002"),
            ],
            "InvalidLaunchTemplateId.VersionNotFound",
        ),
        (
            "a source version that is neither a number nor an alias",
            vec![
                p("LaunchTemplateId", &launch_template_id),
                p("SourceVersion", "$Newest"),
                data("ImageId", "ami-00000000000000002"),
            ],
            "InvalidParameterValue",
        ),
        (
            // `LaunchTemplateData` is `required` in the model, so the request walk refuses a
            // request that carries none of it BEFORE this handler runs: the status and the
            // boundary's own code are what the operation promises for it.
            "no launch template data at all",
            vec![p("LaunchTemplateId", &launch_template_id)],
            "MalformedInput",
        ),
        (
            "an ImageId naming a Systems Manager parameter no image publishes",
            vec![
                p("LaunchTemplateId", &launch_template_id),
                data("ImageId", "resolve:ssm:/aws/service/no/such/parameter"),
            ],
            "SsmInvalidParameter",
        ),
    ];

    for (case, params, expected_code) in rows {
        let (status, body) = create_version(&router, &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, expected_code,
            "case {case:?}: expected the error code {expected_code:?}, got {code:?} in \
             body {body}"
        );
        assert_eq!(
            held(&engine),
            1,
            "case {case:?}: a refused create stores no version"
        );
    }

    // A dry run of a request that WOULD have succeeded reports that outcome and stores
    // nothing.
    let (status, body) = create_version(
        &router,
        &[
            p("LaunchTemplateId", &launch_template_id),
            p("DryRun", "true"),
            data("ImageId", "ami-00000000000000002"),
        ],
    )
    .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (412, "DryRunOperation"),
        "a dry run must report the outcome it asked about, got {status} with body {body}"
    );
    assert_eq!(
        held(&engine),
        1,
        "a dry run stores no version"
    );

    // One ClientToken creates one version: the repeat answers with the version the first
    // call created rather than a second one.
    let token = vec![
        p("LaunchTemplateId", &launch_template_id),
        p("ClientToken", "one-token"),
        data("ImageId", "ami-00000000000000002"),
    ];
    let first = ok("a token's first call", &router, "CreateLaunchTemplateVersion", &token).await;
    let repeat = ok("a token's repeat", &router, "CreateLaunchTemplateVersion", &token).await;
    assert_eq!(
        element(&first, "versionNumber"),
        element(&repeat, "versionNumber"),
        "a repeated ClientToken must answer with the version it created, got {first} \
         then {repeat}"
    );
    assert_eq!(
        held(&engine),
        2,
        "a repeated ClientToken stores one version, not two"
    );
    let reported = reported_versions(
        &ok(
            "after the token's repeat",
            &router,
            "DescribeLaunchTemplateVersions",
            &[p("LaunchTemplateId", &launch_template_id)],
        )
        .await,
    );
    assert_eq!(
        reported,
        vec![2, 1],
        "after the token's repeat the template holds two versions, newest first, \
         got {reported:?}"
    );

    // The token is scoped to the template it was used against: the SAME token naming another
    // template is a request about that template, not a replay of this one, so it creates that
    // template's own next version rather than answering with this template's.
    let other = create_template(
        "another template",
        &router,
        "versions.other",
        &[data("ImageId", "ami-00000000000000003")],
    )
    .await;
    let body = ok(
        "the same token against another template",
        &router,
        "CreateLaunchTemplateVersion",
        &[
            p("LaunchTemplateId", &other),
            p("ClientToken", "one-token"),
            data("ImageId", "ami-00000000000000004"),
        ],
    )
    .await;
    assert_eq!(
        (
            element(&body, "launchTemplateId"),
            element(&body, "versionNumber"),
            element(&body, "imageId"),
        ),
        (
            Some(other.clone()),
            Some("2".to_string()),
            Some("ami-00000000000000004".to_string()),
        ),
        "a token reused against another template must create that template's next version, \
         got {body}"
    );
}
