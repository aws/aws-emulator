// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `RunInstances` driven from a LAUNCH TEMPLATE, over the wire the
//! service actually speaks (`ec2Query`, through the crate's shared [`super::ec2_query`]
//! harness).
//!
//! The rules under test are:
//!
//! 1. A LAUNCH THAT NAMES A TEMPLATE RUNS FROM THAT VERSION'S PARAMETERS, overlaid with the
//!    members the request carries itself — "any additional parameters that you specify for the
//!    new instance overwrite the corresponding parameters included in the launch template"
//!    (`RunInstancesRequest$LaunchTemplate`) — so the instance the launch mints reports the
//!    template's values where the request named none and the request's where it did, and a
//!    `DescribeInstances` reads the same instance back.
//! 2. WHICH VERSION is the one the request names, resolved the way every reader of a template
//!    resolves it: a number, `$Latest`, `$Default`, and the DEFAULT version for a request that
//!    names none at all ("Default: The default version of the launch template",
//!    `LaunchTemplateSpecification$Version`).
//! 3. A TEMPLATE THE ACCOUNT DOES NOT HOLD launches nothing: an id this service ITSELF minted
//!    whose template has since been deleted is refused `InvalidLaunchTemplateId.NotFound`,
//!    while an id no mint of it ever reached is not an id of a launch template at all and is
//!    refused `InvalidLaunchTemplateId.Malformed`; a version the template does not hold is
//!    `InvalidLaunchTemplateId.VersionNotFound` and a version that is neither a number nor an
//!    alias is `InvalidParameterValue`. No instance is minted for any of them.
//!
//! Each table row is a distinct input along what the rules generalise over — which member the
//! template carries, which the request carries, which key the template is named by, which
//! version the request asks for — so no row is the specification.

use std::collections::BTreeMap;

use service_com_amazonaws_ec2_amazonec2::state::DEFAULT_INSTANCE_TYPE;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, error_code, error_message};
use super::wire::{child, child_text, document, set_items};

/// The Region every call here is signed for.
const REGION: &str = "us-west-2";

/// The AMI a template names when the row is not about which image is used.
const TEMPLATE_AMI: &str = "ami-00000000000000011";

/// A second AMI, so a row about overriding the template's image cannot be reading one value
/// twice.
const REQUEST_AMI: &str = "ami-00000000000000022";

/// One request's form parameters, as the ec2Query harness takes them.
type Params = Vec<(String, String)>;

/// How a row reads the member it is about off a launched instance.
type Reading = Box<dyn Fn(&Launched) -> Option<String>>;

/// One row of the tag table: the case, the request's own tag specification, and the tags the
/// instance must end up with.
type TagRow = (&'static str, Params, BTreeMap<String, String>);

/// One row of the refusal table: the case, the `LaunchTemplate` fields, and the code and
/// message the launch must answer.
type RefusalRow = (String, Params, &'static str, String);

/// A `(key, value)` request parameter.
fn p(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

/// One `LaunchTemplateData.<member>` parameter of a create.
fn data(member: &str, value: impl Into<String>) -> (String, String) {
    p(&format!("LaunchTemplateData.{member}"), value)
}

fn emulator() -> MultiServiceRouter {
    super::ec2_query::emulator().0
}

/// Create a launch template named `name` whose first version carries `version_data`, and
/// answer its id.
async fn create_template(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    version_data: &[(String, String)],
) -> String {
    let mut params = vec![p("LaunchTemplateName", name)];
    params.extend_from_slice(version_data);
    let (status, body) = call(router, REGION, "CreateLaunchTemplate", &params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateLaunchTemplate, \
         got {status} with body {body}"
    );
    let parsed = document(case, &body);
    parsed
        .descendants()
        .filter(|node| node.is_element() && node.tag_name().name() == "launchTemplateId")
        .filter_map(|node| node.text().map(str::to_string))
        .next()
        .unwrap_or_else(|| panic!("case {case:?}: no launch template id in {body}"))
}

/// Add a version carrying `version_data` to the template `launch_template_id`, and answer its
/// number.
async fn create_version(
    router: &MultiServiceRouter,
    case: &str,
    launch_template_id: &str,
    version_data: &[(String, String)],
) -> String {
    let mut params = vec![p("LaunchTemplateId", launch_template_id)];
    params.extend_from_slice(version_data);
    let (status, body) = call(router, REGION, "CreateLaunchTemplateVersion", &params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateLaunchTemplateVersion, \
         got {status} with body {body}"
    );
    let parsed = document(case, &body);
    parsed
        .descendants()
        .filter(|node| node.is_element() && node.tag_name().name() == "versionNumber")
        .filter_map(|node| node.text().map(str::to_string))
        .next()
        .unwrap_or_else(|| panic!("case {case:?}: no version number in {body}"))
}

/// One `RunInstances` call that names `template` fields beside `extra`, as its status and body.
async fn run(
    router: &MultiServiceRouter,
    template: &[(String, String)],
    extra: &[(String, String)],
) -> (u16, String) {
    let mut params = vec![p("MinCount", "1"), p("MaxCount", "1")];
    params.extend_from_slice(template);
    params.extend_from_slice(extra);
    call(router, REGION, "RunInstances", &params).await
}

/// What one launched instance reports: the members a template can decide.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct Launched {
    instance_id: String,
    image_id: Option<String>,
    instance_type: Option<String>,
    key_name: Option<String>,
    subnet_id: Option<String>,
    monitoring: Option<String>,
    availability_zone: Option<String>,
    group_ids: Vec<String>,
    tags: BTreeMap<String, String>,
}

/// The one instance an `<instancesSet>` carries, read off the response of a launch or of a
/// describe.
fn launched(case: &str, body: &str) -> Launched {
    let parsed = document(case, body);
    let item = parsed
        .descendants()
        .find(|node| {
            node.is_element()
                && node.tag_name().name() == "item"
                && child(*node, "instanceId").is_some()
                && child(*node, "instanceState").is_some()
        })
        .unwrap_or_else(|| panic!("case {case:?}: no launched instance in {body}"));
    Launched {
        instance_id: child_text(item, "instanceId").unwrap_or_default(),
        image_id: child_text(item, "imageId"),
        instance_type: child_text(item, "instanceType"),
        key_name: child_text(item, "keyName"),
        subnet_id: child_text(item, "subnetId"),
        monitoring: child(item, "monitoring").and_then(|node| child_text(node, "state")),
        availability_zone: child(item, "placement")
            .and_then(|node| child_text(node, "availabilityZone")),
        group_ids: set_items(item, "groupSet")
            .into_iter()
            .filter_map(|group| child_text(group, "groupId"))
            .collect(),
        tags: set_items(item, "tagSet")
            .into_iter()
            .map(|tag| {
                (
                    child_text(tag, "key").unwrap_or_default(),
                    child_text(tag, "value").unwrap_or_default(),
                )
            })
            .collect(),
    }
}

/// The instance `instance_id` as `DescribeInstances` reports it.
async fn described(router: &MultiServiceRouter, case: &str, instance_id: &str) -> Launched {
    let (status, body) = call(
        router,
        REGION,
        "DescribeInstances",
        &[p("InstanceId.1", instance_id)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: reading {instance_id} back must succeed, got {status} with body {body}"
    );
    launched(case, &body)
}

/// The id of a subnet of this Region's default network, and the id of a security group in the
/// same VPC — the resources a template's own members can name.
async fn default_network(router: &MultiServiceRouter) -> (String, String) {
    let (status, body) = call(router, REGION, "DescribeSubnets", &[]).await;
    assert_eq!(
        status, 200,
        "the default network's subnets must be readable, got {status} with body {body}"
    );
    let parsed = document("the default network", &body);
    let subnet_id = parsed
        .descendants()
        .filter(|node| node.is_element() && node.tag_name().name() == "subnetId")
        .filter_map(|node| node.text().map(str::to_string))
        .next()
        .unwrap_or_else(|| panic!("no subnet in the default network: {body}"));

    let (status, body) = call(router, REGION, "DescribeSecurityGroups", &[]).await;
    assert_eq!(
        status, 200,
        "the default network's security groups must be readable, \
         got {status} with body {body}"
    );
    let parsed = document("the default network", &body);
    let group_id = parsed
        .descendants()
        .filter(|node| node.is_element() && node.tag_name().name() == "groupId")
        .filter_map(|node| node.text().map(str::to_string))
        .next()
        .unwrap_or_else(|| panic!("no security group in the default network: {body}"));
    (subnet_id, group_id)
}

/// RULE 1: the launch runs from the template version's parameters, and a member the request
/// carries itself overrides the template's.
#[tokio::test]
async fn a_launch_runs_from_the_template_overlaid_with_the_request() {
    let router = emulator();
    let (subnet_id, group_id) = default_network(&router).await;

    // One row per member the overlay decides, plus a row for each side winning. `expected`
    // is read off the instance the launch minted AND off the describe that reads it back.
    struct Row {
        case: &'static str,
        /// The template version's own launch parameters.
        template: Params,
        /// What the request carries beside the template reference.
        request: Params,
        /// What the instance must report.
        expect: Reading,
        /// The value that reading must answer.
        value: Option<String>,
    }

    let rows: Vec<Row> = vec![
        Row {
            case: "the image from the template",
            template: vec![data("ImageId", TEMPLATE_AMI)],
            request: Vec::new(),
            expect: Box::new(|instance| instance.image_id.clone()),
            value: Some(TEMPLATE_AMI.to_string()),
        },
        Row {
            case: "the request's image overrides the template's",
            template: vec![data("ImageId", TEMPLATE_AMI)],
            request: vec![p("ImageId", REQUEST_AMI)],
            expect: Box::new(|instance| instance.image_id.clone()),
            value: Some(REQUEST_AMI.to_string()),
        },
        Row {
            case: "the instance type from the template",
            template: vec![data("ImageId", TEMPLATE_AMI), data("InstanceType", "t3.small")],
            request: Vec::new(),
            expect: Box::new(|instance| instance.instance_type.clone()),
            value: Some("t3.small".to_string()),
        },
        Row {
            case: "the request's instance type overrides the template's",
            template: vec![data("ImageId", TEMPLATE_AMI), data("InstanceType", "t3.small")],
            request: vec![p("InstanceType", "t3.large")],
            expect: Box::new(|instance| instance.instance_type.clone()),
            value: Some("t3.large".to_string()),
        },
        Row {
            case: "neither names an instance type, so the launch takes the default",
            template: vec![data("ImageId", TEMPLATE_AMI)],
            request: Vec::new(),
            expect: Box::new(|instance| instance.instance_type.clone()),
            value: Some(DEFAULT_INSTANCE_TYPE.to_string()),
        },
        Row {
            case: "the key pair name from the template",
            template: vec![data("ImageId", TEMPLATE_AMI), data("KeyName", "cluster-key")],
            request: Vec::new(),
            expect: Box::new(|instance| instance.key_name.clone()),
            value: Some("cluster-key".to_string()),
        },
        Row {
            case: "detailed monitoring turned on by the template",
            template: vec![
                data("ImageId", TEMPLATE_AMI),
                data("Monitoring.Enabled", "true"),
            ],
            request: Vec::new(),
            expect: Box::new(|instance| instance.monitoring.clone()),
            value: Some("enabled".to_string()),
        },
        Row {
            case: "the template's placement zone",
            template: vec![
                data("ImageId", TEMPLATE_AMI),
                data("Placement.AvailabilityZone", "us-west-2b"),
            ],
            request: Vec::new(),
            expect: Box::new(|instance| instance.availability_zone.clone()),
            value: Some("us-west-2b".to_string()),
        },
        Row {
            case: "the security group the template names",
            template: vec![
                data("ImageId", TEMPLATE_AMI),
                data("SecurityGroupId.1", group_id.clone()),
            ],
            request: Vec::new(),
            expect: Box::new(|instance| instance.group_ids.first().cloned()),
            value: Some(group_id.clone()),
        },
        Row {
            case: "the subnet of the template's primary network interface",
            template: vec![
                data("ImageId", TEMPLATE_AMI),
                data("NetworkInterface.1.DeviceIndex", "0"),
                data("NetworkInterface.1.SubnetId", subnet_id.clone()),
            ],
            request: Vec::new(),
            expect: Box::new(|instance| instance.subnet_id.clone()),
            value: Some(subnet_id.clone()),
        },
        Row {
            case: "the request's subnet overrides the template interface's",
            template: vec![
                data("ImageId", TEMPLATE_AMI),
                data("NetworkInterface.1.DeviceIndex", "0"),
                data("NetworkInterface.1.SubnetId", "subnet-00000000000000099"),
            ],
            request: vec![p("SubnetId", subnet_id.clone())],
            expect: Box::new(|instance| instance.subnet_id.clone()),
            value: Some(subnet_id.clone()),
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let name = format!("overlay-{index}");
        let launch_template_id =
            create_template(&router, case, &name, &row.template).await;
        let mut template_fields = vec![p("LaunchTemplate.LaunchTemplateId", launch_template_id)];
        template_fields.extend(row.request.clone());
        let (status, body) = run(&router, &template_fields, &[]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from the launch, got {status} with body {body}"
        );
        let minted = launched(case, &body);
        let answered = (row.expect)(&minted);
        assert_eq!(
            answered, row.value,
            "case {case:?}: expected the launch to report {:?}, got {answered:?} \
             from {minted:?}",
            row.value
        );

        // The instance is read back through the describe: what the launch reported is what
        // the store holds, not something composed for the one answer.
        let read_back = described(&router, case, &minted.instance_id).await;
        let answered = (row.expect)(&read_back);
        assert_eq!(
            answered, row.value,
            "case {case:?}: expected the describe to report {:?}, got {answered:?} \
             from {read_back:?}",
            row.value
        );
    }
}

/// RULE 1, continued: the tags a template's `instance` tag specification carries reach the
/// instance the launch mints, and the request's own specification replaces the template's.
#[tokio::test]
async fn the_templates_instance_tags_reach_the_launched_instance() {
    let router = emulator();
    let template_tags: Params = vec![
        data("TagSpecification.1.ResourceType", "instance"),
        data("TagSpecification.1.Tag.1.Key", "Name"),
        data("TagSpecification.1.Tag.1.Value", "from-the-template"),
        data("TagSpecification.1.Tag.2.Key", "Stage"),
        data("TagSpecification.1.Tag.2.Value", "test"),
    ];
    let mut template: Params = vec![data("ImageId", TEMPLATE_AMI)];
    template.extend(template_tags);
    let launch_template_id = create_template(&router, "template tags", "tagged", &template).await;

    // Rows differing along whether the request carries a specification of its own.
    let rows: [TagRow; 2] = [
        (
            "only the template's tags",
            Vec::new(),
            [
                ("Name".to_string(), "from-the-template".to_string()),
                ("Stage".to_string(), "test".to_string()),
            ]
            .into_iter()
            .collect(),
        ),
        (
            "the request's tags replace the template's",
            vec![
                p("TagSpecification.1.ResourceType", "instance"),
                p("TagSpecification.1.Tag.1.Key", "Name"),
                p("TagSpecification.1.Tag.1.Value", "from-the-request"),
            ],
            [("Name".to_string(), "from-the-request".to_string())]
                .into_iter()
                .collect(),
        ),
    ];

    for (case, request, expected) in rows {
        let mut fields = vec![p(
            "LaunchTemplate.LaunchTemplateId",
            launch_template_id.clone(),
        )];
        fields.extend(request);
        let (status, body) = run(&router, &fields, &[]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from the launch, got {status} with body {body}"
        );
        let minted = launched(case, &body);
        assert_eq!(
            minted.tags, expected,
            "case {case:?}: expected the instance to carry {expected:?}, got {minted:?}"
        );
        let read_back = described(&router, case, &minted.instance_id).await;
        assert_eq!(
            read_back.tags, expected,
            "case {case:?}: expected the describe to report {expected:?}, got {read_back:?}"
        );
    }
}

/// RULE 2: which version the launch runs from — a number, either alias, and the template's
/// DEFAULT version for a request that names none.
#[tokio::test]
async fn the_version_the_request_names_is_the_one_the_launch_runs_from() {
    let router = emulator();
    // Two versions, each naming a different image, so which version ran is readable off the
    // instance. Version 1 is the template's default version until something changes it.
    let launch_template_id = create_template(
        &router,
        "versions",
        "versioned",
        &[data("ImageId", TEMPLATE_AMI)],
    )
    .await;
    let second = create_version(
        &router,
        "versions",
        &launch_template_id,
        &[data("ImageId", REQUEST_AMI)],
    )
    .await;
    assert_eq!(
        second, "2",
        "the second version of a template is version 2, got {second:?}"
    );

    // Rows differing along how the request names the version: not at all, by number, and by
    // each of the two aliases the member publishes.
    let rows: [(&str, Option<&str>, &str); 5] = [
        ("no version at all, so the default one", None, TEMPLATE_AMI),
        ("the first version by number", Some("1"), TEMPLATE_AMI),
        ("the second version by number", Some("2"), REQUEST_AMI),
        ("the $Default alias", Some("$Default"), TEMPLATE_AMI),
        ("the $Latest alias", Some("$Latest"), REQUEST_AMI),
    ];

    for (case, version, expected_image) in rows {
        let mut fields = vec![p(
            "LaunchTemplate.LaunchTemplateId",
            launch_template_id.clone(),
        )];
        if let Some(version) = version {
            fields.push(p("LaunchTemplate.Version", version));
        }
        let (status, body) = run(&router, &fields, &[]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from the launch, got {status} with body {body}"
        );
        let minted = launched(case, &body);
        assert_eq!(
            minted.image_id.as_deref(),
            Some(expected_image),
            "case {case:?}: expected the launch to run from {expected_image}, got {minted:?}"
        );
    }

    // The template may also be named by its NAME, which resolves to the same template.
    let (status, body) = run(
        &router,
        &[p("LaunchTemplate.LaunchTemplateName", "versioned")],
        &[],
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"named by its name\": expected HTTP 200, got {status} with body {body}"
    );
    let minted = launched("named by its name", &body);
    assert_eq!(
        minted.image_id.as_deref(),
        Some(TEMPLATE_AMI),
        "case \"named by its name\": expected the default version's image, got {minted:?}"
    );
}

/// RULE 3: a template or a version the account does not hold launches nothing, and the
/// refusal says which of the two it is.
#[tokio::test]
async fn a_template_the_account_does_not_hold_launches_nothing() {
    let router = emulator();
    let launch_template_id = create_template(
        &router,
        "refusals",
        "refused",
        &[data("ImageId", TEMPLATE_AMI)],
    )
    .await;
    // A template created and then deleted: an id this service minted that names no template
    // of the account any more.
    let deleted = create_template(
        &router,
        "refusals",
        "deleted",
        &[data("ImageId", TEMPLATE_AMI)],
    )
    .await;
    let (status, body) = call(
        &router,
        REGION,
        "DeleteLaunchTemplate",
        &[p("LaunchTemplateId", deleted.clone())],
    )
    .await;
    assert_eq!(
        status, 200,
        "deleting a template must succeed, got {status} with body {body}"
    );

    // (case, the LaunchTemplate fields, the code and message the launch must answer)
    let rows: Vec<RefusalRow> = vec![
        (
            "a template that has been deleted".to_string(),
            vec![p("LaunchTemplate.LaunchTemplateId", deleted.clone())],
            "InvalidLaunchTemplateId.NotFound",
            format!("The specified launch template, with template ID {deleted}, does not exist."),
        ),
        (
            "an id no mint of this service ever reached".to_string(),
            vec![p(
                "LaunchTemplate.LaunchTemplateId",
                "lt-0fedcba987654321f",
            )],
            "InvalidLaunchTemplateId.Malformed",
            "Malformed launch template Id: lt-0fedcba987654321f".to_string(),
        ),
        (
            "a name no template of the account carries".to_string(),
            vec![p("LaunchTemplate.LaunchTemplateName", "no.such.template")],
            "InvalidLaunchTemplateName.NotFoundException",
            "At least one of the launch templates specified in the request does not exist."
                .to_string(),
        ),
        (
            "a version the template does not hold".to_string(),
            vec![
                p("LaunchTemplate.LaunchTemplateId", launch_template_id.clone()),
                p("LaunchTemplate.Version", "9"),
            ],
            "InvalidLaunchTemplateId.VersionNotFound",
            format!(
                "Could not find the specified version 9 for the launch template with \
                 ID {launch_template_id}."
            ),
        ),
        (
            "a version that is neither a number nor an alias".to_string(),
            vec![
                p("LaunchTemplate.LaunchTemplateId", launch_template_id.clone()),
                p("LaunchTemplate.Version", "latest"),
            ],
            "InvalidParameterValue",
            "Invalid value 'latest' for version. Valid values are $Latest, $Default, \
             and version numbers"
                .to_string(),
        ),
        (
            "both of the template's keys at once".to_string(),
            vec![
                p("LaunchTemplate.LaunchTemplateId", launch_template_id.clone()),
                p("LaunchTemplate.LaunchTemplateName", "refused"),
            ],
            "InvalidParameterCombination",
            "Either the launch template ID or the launch template name may be specified, \
             but not both"
                .to_string(),
        ),
        (
            "both keys at once, where the id names no template either".to_string(),
            vec![
                p("LaunchTemplate.LaunchTemplateId", "lt-0fedcba987654321f"),
                p("LaunchTemplate.LaunchTemplateName", "no.such.template"),
            ],
            "InvalidParameterCombination",
            "Either the launch template ID or the launch template name may be specified, \
             but not both"
                .to_string(),
        ),
    ];

    for (case, fields, code, message) in rows {
        let before = instance_count(&router).await;
        let (status, body) = run(&router, &fields, &[]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            code,
            "case {case:?}: expected {code}, got body {body}"
        );
        assert_eq!(
            error_message(&body),
            message,
            "case {case:?}: expected the refusal's own wording, got body {body}"
        );
        let after = instance_count(&router).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused launch must mint no instance"
        );
    }
}

/// How many instances the account holds, as `DescribeInstances` reports them.
async fn instance_count(router: &MultiServiceRouter) -> usize {
    let (status, body) = call(router, REGION, "DescribeInstances", &[]).await;
    assert_eq!(
        status, 200,
        "reading the account's instances must succeed, got {status} with body {body}"
    );
    let parsed = document("the account's instances", &body);
    parsed
        .descendants()
        .filter(|node| node.is_element() && node.tag_name().name() == "instanceId")
        .count()
}
