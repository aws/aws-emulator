// SPDX-License-Identifier: CC0-1.0

//! WHICH `ImageId` VALUES A LAUNCH TEMPLATE WRITER TAKES when the value names a Systems
//! Manager parameter instead of an AMI, driven over the wire the service speaks (`ec2Query`,
//! an `application/x-www-form-urlencoded` POST answered with XML — README.md / OPERATIONS.md
//! "Wire protocol") through the crate's shared [`super::ec2_query`] harness.
//!
//! The rule under test: a WRITE of a launch template version RESOLVES such a value against
//! the account's image catalog, the same catalog a launch from the stored version resolves it
//! against — so a value naming a parameter that resolves to an AMI is accepted and STORED AS
//! SUBMITTED, unresolved (the parameter "will resolve to an AMI ID on launch",
//! `RequestLaunchTemplateData$ImageId`, so every reader of the version still reports the
//! parameter), while a value naming a parameter this account resolves no AMI for is refused
//! where it is written, as the invalid ALIAS it is — `SsmInvalidParameter`, whose fixed
//! sentence names the parameters that resolved to nothing — and nothing is stored. Both
//! writers — `CreateLaunchTemplate` and `CreateLaunchTemplateVersion` — apply it, so the two
//! cannot disagree about which values a stored version may carry.
//!
//! The rows are the alias forms that member documents — a public parameter, a parameter name
//! of the account's own, that name with a version number or with a label, and the parameter
//! ARN of a parameter shared from another account in each of those three shapes
//! (`smithy_query(query="doc",shape="RequestLaunchTemplateData",member="ImageId")`) — each
//! with the outcome the rule gives it, beside a name in the published namespace that this
//! account's catalog carries no image under. Each row is a distinct input along what the rule
//! generalises over (which form the value takes, and whether anything resolves it), so no row
//! is the specification; the published name every row is compared against comes out of the
//! store the handlers read rather than being written here.
//!
//! `launch_template_published_ssm_alias` owns the whole published catalog as rows of the
//! create's acceptance, and `launch_from_ssm_image_alias` owns what a LAUNCH does with each
//! value; this suite owns the forms a writer takes and what it does with each of them.

use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, FIRST_LAUNCH_TEMPLATE_VERSION,
    SSM_IMAGE_ALIAS_PREFIX,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, elements, error_code, error_message};

/// The Region every call here is signed for. A launch template carries no Region member, so
/// the Region only selects the ec2Query envelope — and it is the Region of the parameter ARN
/// the shared-parameter rows carry.
const REGION: &str = "us-east-1";

/// The instance type every template here names, beside the `ImageId` under test.
const INSTANCE_TYPE: &str = "t3.micro";

/// A parameter name of the caller's OWN namespace: "to reference a parameter stored in the
/// same account: `resolve:ssm:parameter-name`". Nothing publishes it, so the account resolves
/// no AMI for it.
const OWN_PARAMETER: &str = "/team/golden/image-id";

/// A name in the PUBLISHED namespace (`Image$PublicSsmParameterName`: "under the
/// `aws/service/` namespace") that the catalog carries no image under.
const UNPUBLISHED_PARAMETER: &str = "/aws/service/ami-amazon-linux-latest/no-such-build";

/// The version number a `parameter-name:version-number` row pins.
const PARAMETER_VERSION: &str = "3";

/// The label a `parameter-name:label` row names.
const PARAMETER_LABEL: &str = "released";

/// The class EC2 refuses an `ImageId` alias it can resolve no AMI for with, and the fixed text
/// of that refusal — written out here as the service states them
/// (`ec2::state`'s `SSM_INVALID_PARAMETER` / `INVALID_SSM_ALIASES_PREFIX`), so this suite
/// checks AWS's own wording rather than the constants the crate carries.
const INVALID_ALIAS_CODE: &str = "SsmInvalidParameter";
const INVALID_ALIASES_PREFIX: &str = "The following aliases are invalid: ";

/// One ec2Query call through the shared harness.
async fn api(
    router: &MultiServiceRouter,
    action: &str,
    params: &[(String, String)],
) -> (u16, String) {
    super::ec2_query::call(router, REGION, action, params).await
}

/// A `(key, value)` request parameter.
fn p(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

/// One `LaunchTemplateData.<member>` parameter.
fn data(member: &str, value: impl Into<String>) -> (String, String) {
    p(&format!("LaunchTemplateData.{member}"), value)
}

/// The `resolve:ssm:` value naming `parameter`.
fn alias(parameter: &str) -> String {
    format!("{SSM_IMAGE_ALIAS_PREFIX}{parameter}")
}

/// The ARN of a Systems Manager parameter, as the one canonical ARN builder of this
/// workspace writes it ([`engine::emulator_arn`]).
fn parameter_arn(parameter: &str) -> String {
    engine::emulator_arn("ssm", REGION, &format!("parameter{parameter}"))
}

/// The first public parameter name this account's published catalog resolves — read out of
/// the store the handlers read, so the row is the service's own catalog rather than a copy of
/// it — and the id of the AMI it names.
fn published_parameter(engine: &engine::InMemoryEngine) -> (String, String) {
    engine
        .state::<Ec2State>()
        .lock()
        .public_ssm_parameter_image_ids(engine::emulator_account_id())
        .into_iter()
        .next()
        .expect("the published image catalog must carry a parameter name")
}

/// One row: what it names, the `ImageId` the request carries, and whether the account
/// resolves an AMI for the parameter it names — which is what decides the outcome.
struct Form {
    case: String,
    submitted: String,
    resolved: bool,
}

/// Every alias form a launch template writer takes, with the outcome the rule gives it.
///
/// `published` is the one name read out of the store, so the published-parameter row cannot
/// drift from the catalog.
fn alias_forms(published: &str) -> Vec<Form> {
    let arn = parameter_arn(OWN_PARAMETER);
    let row = |case: &str, submitted: String, resolved: bool| Form {
        case: case.to_string(),
        submitted,
        resolved,
    };
    vec![
        row(
            "a public parameter the catalog resolves",
            alias(published),
            true,
        ),
        row(
            "a public parameter name the catalog resolves nothing for",
            alias(UNPUBLISHED_PARAMETER),
            false,
        ),
        row(
            "a parameter name of the account's own",
            alias(OWN_PARAMETER),
            false,
        ),
        row(
            "that name with a version number",
            alias(&format!("{OWN_PARAMETER}:{PARAMETER_VERSION}")),
            false,
        ),
        row(
            "that name with a label",
            alias(&format!("{OWN_PARAMETER}:{PARAMETER_LABEL}")),
            false,
        ),
        row(
            "the ARN of a parameter shared from another account",
            alias(&arn),
            false,
        ),
        row(
            "that ARN with a version number",
            alias(&format!("{arn}:{PARAMETER_VERSION}")),
            false,
        ),
        row(
            "that ARN with a label",
            alias(&format!("{arn}:{PARAMETER_LABEL}")),
            false,
        ),
    ]
}

/// The `ImageId` the STORE holds for `version` of the template `launch_template_id` — the
/// value as the writer wrote it, read where every later reader of the template sees it.
fn stored_image_id(
    engine: &engine::InMemoryEngine,
    launch_template_id: &str,
    version: i64,
) -> Option<String> {
    engine
        .state::<Ec2State>()
        .lock()
        .launch_templates
        .get(launch_template_id)
        .and_then(|record| record.versions.get(&version))
        .and_then(|stored| stored.data.image_id.clone())
}

/// How many launch templates the account holds, read out of the store: a refused write must
/// leave this where it was.
fn templates_held(engine: &engine::InMemoryEngine) -> usize {
    engine.state::<Ec2State>().lock().launch_templates.len()
}

/// The `imageId` `DescribeLaunchTemplateVersions` reports for every version of a template,
/// with no `ResolveAlias`: "if false, and if a Systems Manager parameter is specified for
/// `ImageId`, the parameter is displayed in the response for `imageId`. Default: false".
async fn described_image_ids(
    case: &str,
    router: &MultiServiceRouter,
    launch_template_id: &str,
) -> Vec<String> {
    let (status, body) = api(
        router,
        "DescribeLaunchTemplateVersions",
        &[p("LaunchTemplateId", launch_template_id)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeLaunchTemplateVersions, got {status} \
         with body {body}"
    );
    elements(&body, "imageId")
}

/// Assert a refused write: the class EC2 answers for an alias it can resolve no AMI for, the
/// sentence naming the PARAMETER the value named, and nothing stored.
fn assert_refused(case: &str, submitted: &str, status: u16, body: &str) {
    assert_eq!(
        (status, error_code(body)),
        (400, INVALID_ALIAS_CODE.to_string()),
        "case {case:?} ({submitted}): expected {INVALID_ALIAS_CODE}, got {status} with body \
         {body}"
    );
    let parameter = submitted
        .strip_prefix(SSM_IMAGE_ALIAS_PREFIX)
        .unwrap_or(submitted);
    let expected = format!("{INVALID_ALIASES_PREFIX}{parameter}");
    assert_eq!(
        error_message(body),
        expected,
        "case {case:?}: expected {expected:?}, got {:?}",
        error_message(body)
    );
}

/// RULE: `CreateLaunchTemplate` takes an `ImageId` in a documented `resolve:ssm:` form when
/// the account resolves an AMI for the parameter it names — 200 with the launch template it
/// stored, whose only version is both its default and its latest, keeping the value exactly
/// as submitted — and refuses one it resolves nothing for, storing no template.
#[tokio::test]
async fn a_create_takes_the_forms_it_resolves_and_refuses_what_it_resolves_nothing_for() {
    let (router, engine) = super::ec2_query::emulator();
    let (published, _) = published_parameter(&engine);
    let first_version = FIRST_LAUNCH_TEMPLATE_VERSION.to_string();

    for (index, form) in alias_forms(&published).iter().enumerate() {
        let Form {
            case,
            submitted,
            resolved,
        } = form;
        let name = format!("alias.form.{index}");
        let held_before = templates_held(&engine);
        let (status, body) = api(
            &router,
            "CreateLaunchTemplate",
            &[
                p("LaunchTemplateName", name.clone()),
                data("ImageId", submitted.clone()),
                data("InstanceType", INSTANCE_TYPE),
            ],
        )
        .await;
        if !resolved {
            assert_refused(case, submitted, status, &body);
            assert_eq!(
                templates_held(&engine),
                held_before,
                "case {case:?}: a refused create must store no template"
            );
            continue;
        }
        assert_eq!(
            status, 200,
            "case {case:?} ({submitted}): expected HTTP 200 from CreateLaunchTemplate, got \
             {status} with body {body}"
        );
        let launch_template_id = element(&body, "launchTemplateId").unwrap_or_else(|| {
            panic!("case {case:?}: expected a <launchTemplateId>, got {body}")
        });
        assert_eq!(
            (
                element(&body, "launchTemplateName"),
                element(&body, "defaultVersionNumber"),
                element(&body, "latestVersionNumber"),
            ),
            (
                Some(name.clone()),
                Some(first_version.clone()),
                Some(first_version.clone()),
            ),
            "case {case:?}: expected the stored template's name and its one version, got {body}"
        );

        let held = stored_image_id(&engine, &launch_template_id, FIRST_LAUNCH_TEMPLATE_VERSION);
        assert_eq!(
            held.as_deref(),
            Some(submitted.as_str()),
            "case {case:?}: expected the stored version to hold {submitted:?} unresolved, got \
             {held:?}"
        );
        let reported = described_image_ids(case, &router, &launch_template_id).await;
        assert_eq!(
            reported,
            vec![submitted.clone()],
            "case {case:?}: expected the version to report {submitted:?}, got {reported:?}"
        );
    }
}

/// The same rule for the OTHER writer: `CreateLaunchTemplateVersion` stores a value it
/// resolves exactly as submitted and refuses one it resolves nothing for, leaving the
/// template's versions as they were — so the two writers cannot disagree about which value a
/// stored version may carry.
#[tokio::test]
async fn a_version_create_takes_the_same_forms_the_create_takes() {
    let (router, engine) = super::ec2_query::emulator();
    let (published, _) = published_parameter(&engine);

    // One template to hold the versions, created from a literal AMI id so the forms under
    // test are what the versions add.
    let (status, body) = api(
        &router,
        "CreateLaunchTemplate",
        &[
            p("LaunchTemplateName", "alias.form.versions"),
            data("ImageId", "ami-0123456789abcdef0"),
            data("InstanceType", INSTANCE_TYPE),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "the template holding the versions must be created, got {status} with body {body}"
    );
    let launch_template_id = element(&body, "launchTemplateId")
        .unwrap_or_else(|| panic!("no <launchTemplateId> in {body}"));

    let forms = alias_forms(&published);
    // The versions are "numbered in the order in which they are created"
    // (`CreateLaunchTemplateVersion`), and a refused create numbers none, so the next number
    // is tracked across the rows rather than derived from the row's position.
    let mut next_number = FIRST_LAUNCH_TEMPLATE_VERSION + 1;
    let mut stored_values: Vec<String> = Vec::new();
    for form in &forms {
        let Form {
            case,
            submitted,
            resolved,
        } = form;
        let (status, body) = api(
            &router,
            "CreateLaunchTemplateVersion",
            &[
                p("LaunchTemplateId", launch_template_id.clone()),
                data("ImageId", submitted.clone()),
                data("InstanceType", INSTANCE_TYPE),
            ],
        )
        .await;
        if !resolved {
            assert_refused(case, submitted, status, &body);
            continue;
        }
        assert_eq!(
            status, 200,
            "case {case:?} ({submitted}): expected HTTP 200 from \
             CreateLaunchTemplateVersion, got {status} with body {body}"
        );
        assert_eq!(
            element(&body, "versionNumber"),
            Some(next_number.to_string()),
            "case {case:?}: expected version {next_number}, got {body}"
        );
        assert_eq!(
            element(&body, "imageId").as_deref(),
            Some(submitted.as_str()),
            "case {case:?}: expected the created version to report {submitted:?}, got {body}"
        );
        let held = stored_image_id(&engine, &launch_template_id, next_number);
        assert_eq!(
            held.as_deref(),
            Some(submitted.as_str()),
            "case {case:?}: expected the stored version to hold {submitted:?} unresolved, got \
             {held:?}"
        );
        stored_values.push(submitted.clone());
        next_number += 1;
    }

    // Every version the rows created reads back with its own value, none of them resolved —
    // and no version carries a value a row was refused for.
    let reported = described_image_ids("every version", &router, &launch_template_id).await;
    for submitted in &stored_values {
        assert!(
            reported.contains(submitted),
            "expected {submitted:?} among the versions described, got {reported:?}"
        );
    }
    for form in forms.iter().filter(|form| !form.resolved) {
        assert!(
            !reported.contains(&form.submitted),
            "case {:?}: a refused version must not be stored, got {reported:?}",
            form.case
        );
    }
}

/// RULE, second half: the parameter a STORED version carries is read by the launch as well —
/// a launch from a template whose value the catalog resolves runs from that AMI, and the
/// template itself stays exactly as it was stored.
#[tokio::test]
async fn the_launch_resolves_the_parameter_a_stored_version_carries() {
    let (router, engine) = super::ec2_query::emulator();
    let (published, published_image) = published_parameter(&engine);
    let submitted = alias(&published);
    // An instance runs the machine code its AMI holds, so the version names a type whose
    // architecture is that of the AMI the parameter resolves to.
    let instance_type = super::ec2_query::launch_type_for_image(
        &router,
        REGION,
        "the stored version's launch",
        &published_image,
    )
    .await
    .first()
    .map(|(_, instance_type)| instance_type.clone())
    .unwrap_or_else(|| INSTANCE_TYPE.to_string());

    let (status, body) = api(
        &router,
        "CreateLaunchTemplate",
        &[
            p("LaunchTemplateName", "alias.launch.0"),
            data("ImageId", submitted.clone()),
            data("InstanceType", instance_type),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "the create must be served, got {status} with body {body}"
    );
    let launch_template_id = element(&body, "launchTemplateId")
        .unwrap_or_else(|| panic!("expected a <launchTemplateId>, got {body}"));

    let (status, launched) = api(
        &router,
        "RunInstances",
        &[
            p("LaunchTemplate.LaunchTemplateId", launch_template_id.clone()),
            p("MinCount", "1"),
            p("MaxCount", "1"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "expected HTTP 200 from RunInstances, got {status} with body {launched}"
    );
    assert_eq!(
        element(&launched, "imageId").as_deref(),
        Some(published_image.as_str()),
        "expected the launch to run from {published_image}, got {launched}"
    );

    // The template is untouched by the launch: the value it stored is still the one
    // submitted, read back through the describe.
    let reported = described_image_ids("after the launch", &router, &launch_template_id).await;
    assert_eq!(
        reported,
        vec![submitted.clone()],
        "expected the stored version to still report {submitted:?}, got {reported:?}"
    );
}
