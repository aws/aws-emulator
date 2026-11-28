// SPDX-License-Identifier: CC0-1.0

//! What `CreateLaunchTemplate` does with an `ImageId` that names an AWS-PUBLISHED public
//! Systems Manager parameter, driven over the wire the service speaks (`ec2Query`, an
//! `application/x-www-form-urlencoded` POST answered with XML — README.md / OPERATIONS.md
//! "Wire protocol") through the crate's shared [`super::ec2_query`] harness.
//!
//! The rule under test: "Alternatively, you can specify a Systems Manager parameter, using
//! one of the following formats. The Systems Manager parameter will resolve to an AMI ID on
//! launch. To reference a public parameter: `resolve:ssm:public-parameter`"
//! (`RequestLaunchTemplateData$ImageId`) — so a `resolve:ssm:` value naming ANY public
//! parameter this account's published image catalog carries is a value the create ACCEPTS:
//! it answers 200 with the launch template it stored, the version keeps the value exactly as
//! submitted (the parameter, never an AMI id), and the parameter is turned into an AMI id
//! only by a launch from that template.
//!
//! The PUBLIC-parameter form is the whole scope here. The `<name>:<version>`,
//! `<name>:<label>` and parameter-ARN forms the same member documents name a parameter stored
//! in or shared with the account rather than a published one;
//! `tests/all/launch_template_ssm_alias_forms.rs` owns the writer's acceptance
//! of those forms and the deferral of every resolution to a launch, so this suite is not the
//! whole cover for `RequestLaunchTemplateData$ImageId`'s alias forms.
//!
//! The rows are the whole published catalog, read out of the store the handler reads rather
//! than written here, so every published name is a case: the Amazon Linux and Windows image
//! aliases, the hierarchies a service publishes its recommended container host under, and
//! the AWS PCS sample AMIs. `launch_template_ssm_alias_catalog` owns what a STORED alias
//! resolves to for a describe that asks for resolution; this suite owns the create's own
//! acceptance of the published namespace and the deferral of the resolution to a launch.

use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, FIRST_LAUNCH_TEMPLATE_VERSION,
    SSM_IMAGE_ALIAS_PREFIX,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, elements, indexed};

/// The Region every call here is signed for. Neither a launch template nor a published
/// parameter carries a Region member, so the Region only selects the ec2Query envelope.
const REGION: &str = "us-east-1";

/// The instance type every template here names, so a launch from it places a real type of
/// the published catalog rather than falling back to a default.
const INSTANCE_TYPE: &str = "t3.micro";

/// The `/aws/service/` hierarchies the published catalog carries an AMI id under.
///
/// `Image$PublicSsmParameterName` is "the name of the public Systems Manager parameter that
/// resolves to this AMI, under the `aws/service/` namespace", and the catalog publishes one
/// hierarchy per family a caller picks an image out of without naming a Region-specific id:
/// the Amazon Linux and Windows alias hierarchies, the ECS-optimized and Bottlerocket
/// hierarchies an ECS container instance is launched from, and the AWS PCS sample AMIs.
/// Asserted against the store's own catalog below, so a namespace dropping out of it is a
/// failure here rather than a refusal a client meets at create time.
const PUBLISHED_NAMESPACES: &[&str] = &[
    "/aws/service/ami-amazon-linux-latest/",
    "/aws/service/ami-windows-latest/",
    "/aws/service/bottlerocket/",
    "/aws/service/ecs/optimized-ami/",
    "/aws/service/pcs/",
];

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

/// The catalog a `resolve:ssm:` value is resolved against: every public parameter name this
/// account resolves, and the id of the AMI it names. Read out of the store the handlers
/// read, so the table is the service's own catalog rather than a copy of it — and every
/// published hierarchy is represented in it.
fn published_catalog(engine: &engine::InMemoryEngine) -> Vec<(String, String)> {
    let catalog: Vec<(String, String)> = engine
        .state::<Ec2State>()
        .lock()
        .public_ssm_parameter_image_ids(engine::emulator_account_id())
        .into_iter()
        .collect();
    assert!(
        catalog.len() > 1,
        "the published catalog must carry more than one parameter name to span, got {catalog:?}"
    );
    for namespace in PUBLISHED_NAMESPACES {
        assert!(
            catalog
                .iter()
                .any(|(parameter, _)| parameter.starts_with(namespace)),
            "case {namespace:?}: the published catalog must carry a parameter in every \
             hierarchy AWS publishes an AMI id under, got {:?}",
            catalog
                .iter()
                .map(|(parameter, _)| parameter.as_str())
                .collect::<Vec<&str>>()
        );
    }
    catalog
}

/// The `resolve:ssm:` value naming `parameter`.
fn alias(parameter: &str) -> String {
    format!("{SSM_IMAGE_ALIAS_PREFIX}{parameter}")
}

/// The `ImageId` the store holds for the template `launch_template_id`'s first version —
/// the value as the create wrote it, read where a later reader of the template sees it.
fn stored_image_id(engine: &engine::InMemoryEngine, launch_template_id: &str) -> Option<String> {
    engine
        .state::<Ec2State>()
        .lock()
        .launch_templates
        .get(launch_template_id)
        .and_then(|record| record.versions.get(&FIRST_LAUNCH_TEMPLATE_VERSION))
        .and_then(|version| version.data.image_id.clone())
}

/// RULE: every public parameter name the published catalog carries is an `ImageId` the
/// create ACCEPTS — 200, with the launch template it stored — and the version it stores
/// keeps that value exactly as submitted, whichever hierarchy the name belongs to.
#[tokio::test]
async fn every_published_parameter_name_is_accepted_and_stored_as_submitted() {
    let (router, engine) = super::ec2_query::emulator();
    let catalog = published_catalog(&engine);
    let first_version = FIRST_LAUNCH_TEMPLATE_VERSION.to_string();

    for (index, (parameter, _)) in catalog.iter().enumerate() {
        let case = format!("ImageId naming {parameter}");
        let submitted = alias(parameter);
        let name = format!("published.alias.{index}");
        // The tag specification a client that manages its templates carries, which this
        // create is the only taggable resource of.
        let params = vec![
            p("LaunchTemplateName", name.clone()),
            data("ImageId", submitted.clone()),
            data("InstanceType", INSTANCE_TYPE),
            p("TagSpecification.1.ResourceType", "launch-template"),
            p("TagSpecification.1.Tag.1.Key", "owner"),
            p("TagSpecification.1.Tag.1.Value", "platform"),
        ];
        let (status, body) = api(&router, "CreateLaunchTemplate", &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from CreateLaunchTemplate, got {status} with \
             body {body}"
        );
        let launch_template_id = element(&body, "launchTemplateId").unwrap_or_else(|| {
            panic!("case {case:?}: expected a <launchTemplateId>, got {body}")
        });
        assert_eq!(
            element(&body, "launchTemplateName").as_deref(),
            Some(name.as_str()),
            "case {case:?}: expected the name the request gave, got {body}"
        );
        assert_eq!(
            (
                element(&body, "defaultVersionNumber"),
                element(&body, "latestVersionNumber"),
            ),
            (Some(first_version.clone()), Some(first_version.clone())),
            "case {case:?}: the created template's only version is both its default and its \
             latest, got {body}"
        );
        let reported_tags: Vec<(String, String)> = elements(&body, "key")
            .into_iter()
            .zip(elements(&body, "value"))
            .collect();
        assert_eq!(
            reported_tags,
            vec![("owner".to_string(), "platform".to_string())],
            "case {case:?}: expected the tags the request gave, got {reported_tags:?}"
        );

        // The STORE holds the value as submitted: the parameter, not an AMI id resolved
        // while creating the template.
        let held = stored_image_id(&engine, &launch_template_id);
        assert_eq!(
            held.as_deref(),
            Some(submitted.as_str()),
            "case {case:?}: expected the stored version to hold {submitted:?}, got {held:?}"
        );

        // And both readers of the template report it: the template itself, and the version
        // whose `ImageId` a request that asks for no resolution sees.
        let (status, described) = api(
            &router,
            "DescribeLaunchTemplates",
            &[p("LaunchTemplateId.1", launch_template_id.clone())],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeLaunchTemplates, got {status} with \
             body {described}"
        );
        assert_eq!(
            element(&described, "launchTemplateName").as_deref(),
            Some(name.as_str()),
            "case {case:?}: expected the stored template to be reported back, got {described}"
        );
        let (status, versions) = api(
            &router,
            "DescribeLaunchTemplateVersions",
            &[p("LaunchTemplateId", launch_template_id.clone())],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeLaunchTemplateVersions, got \
             {status} with body {versions}"
        );
        assert_eq!(
            element(&versions, "imageId").as_deref(),
            Some(submitted.as_str()),
            "case {case:?}: expected the version to report the submitted {submitted:?}, got \
             {versions}"
        );
    }
}

/// RULE, second half: the parameter such a template holds is resolved to an AMI id by the
/// LAUNCH — an instance launched from the template runs from the image that name publishes
/// and reports that id, never the parameter the template stored.
#[tokio::test]
async fn a_launch_from_such_a_template_resolves_the_parameter_to_the_ami_it_publishes() {
    let (router, engine) = super::ec2_query::emulator();
    let catalog = published_catalog(&engine);

    for (index, (parameter, image_id)) in catalog.iter().enumerate() {
        let case = format!("{parameter} -> {image_id}");
        let submitted = alias(parameter);
        let name = format!("published.launch.{index}");
        // An instance runs the machine code its AMI holds, so the version names a type whose
        // architecture is that of the AMI the parameter resolves to — read off the AMI rather
        // than written here.
        let instance_type = super::ec2_query::launch_type_for_image(&router, REGION, &case, image_id)
            .await
            .first()
            .map(|(_, instance_type)| instance_type.clone())
            .unwrap_or_else(|| INSTANCE_TYPE.to_string());
        let (status, body) = api(
            &router,
            "CreateLaunchTemplate",
            &[
                p("LaunchTemplateName", name),
                data("ImageId", submitted.clone()),
                data("InstanceType", instance_type),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from CreateLaunchTemplate, got {status} with \
             body {body}"
        );
        let launch_template_id = element(&body, "launchTemplateId").unwrap_or_else(|| {
            panic!("case {case:?}: expected a <launchTemplateId>, got {body}")
        });

        let (status, launched) = api(
            &router,
            "RunInstances",
            &[
                p("LaunchTemplate.LaunchTemplateId", launch_template_id),
                p("MinCount", "1"),
                p("MaxCount", "1"),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from RunInstances, got {status} with body \
             {launched}"
        );
        let reported = element(&launched, "imageId")
            .unwrap_or_else(|| panic!("case {case:?}: expected an <imageId>, got {launched}"));
        assert_eq!(
            &reported, image_id,
            "case {case:?}: expected the launch to run from the published AMI {image_id}, got \
             {reported:?}"
        );
        assert_ne!(
            reported, submitted,
            "case {case:?}: the launch must resolve the parameter, not report it"
        );
        let instance_id = element(&launched, "instanceId")
            .unwrap_or_else(|| panic!("case {case:?}: expected an <instanceId>, got {launched}"));

        // Read back where a later caller sees it: the instance the launch minted carries the
        // resolved id too.
        let (status, described) = api(
            &router,
            "DescribeInstances",
            &indexed("InstanceId", &[instance_id.as_str()]),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeInstances, got {status} with body \
             {described}"
        );
        let read_back = element(&described, "imageId")
            .unwrap_or_else(|| panic!("case {case:?}: expected an <imageId>, got {described}"));
        assert_eq!(
            &read_back, image_id,
            "case {case:?}: expected DescribeInstances to report {image_id}, got {read_back:?}"
        );
    }
}
