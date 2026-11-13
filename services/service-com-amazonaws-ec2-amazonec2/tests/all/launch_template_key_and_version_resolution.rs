// SPDX-License-Identifier: CC0-1.0

//! How a launch template's KEY and its VERSION resolve across every operation that takes one,
//! and what a version's data reads back as — driven over the wire the service actually speaks
//! (`ec2Query`, through the crate's shared [`super::ec2_query`] harness).
//!
//! THE RULES:
//!
//! 1. A launch template id is a value this service ITSELF issued, so it is SCREENED before it
//!    is looked up: an id no issue of the kind could have produced is not an id of a launch
//!    template at all and is refused `InvalidLaunchTemplateId.Malformed`, while an id it DID
//!    issue whose template the account no longer holds is refused
//!    `InvalidLaunchTemplateId.NotFound`. The two answers mean different things to a caller —
//!    the first says the identifier can never resolve, the second that the resource is gone —
//!    so a delete-then-confirm script settles on the second and never retries the first.
//! 2. A launch that pins a VERSION the template does not hold is refused
//!    `InvalidLaunchTemplateId.VersionNotFound`, naming the version the request asked for and
//!    the id of the template the account holds, and launches nothing; the two aliases resolve
//!    against the template instead.
//! 3. A version's `LaunchTemplateData` reads back as the data its writer submitted: a member
//!    the RESPONSE shape carries and the REQUEST shape does not is reported only where the
//!    store holds an answer for it, so the read → edit → `CreateLaunchTemplateVersion` round
//!    trip a client makes is not refused for a member it never set.
//!
//! The rows differ along what each rule generalises over: rule 1 over the operation that takes
//! the key and over the kind of id (two printed forms, the placeholder, a non-hexadecimal one,
//! a long one far above every value the mint can reach, and an id the mint really handed out);
//! rule 2 over how the version misses (above the highest ever minted, below the first, one
//! that was minted and deleted) and over which key the launch names its template by; rule 3
//! over which metadata options the writer set. No row is the specification.

use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, error_code, error_message};
use super::wire::{child, child_text, document};

/// The Region every call here is signed for. A launch template carries no Region member, so
/// the Region only selects the ec2Query envelope.
const REGION: &str = "us-west-2";

/// The AMI the templates here name, so a launch from one can mint an instance.
const TEMPLATE_AMI: &str = "ami-00000000000000011";

/// One request's form parameters, as the ec2Query harness takes them.
type Params = Vec<(String, String)>;

/// How one operation names the launch template id it is about: operations differ in the
/// member the id arrives on (`LaunchTemplateId`, or `LaunchTemplate.LaunchTemplateId` for a
/// launch), and in the other members they need before they get as far as the key.
type NamesId = fn(&str) -> Params;

/// One row of the id table: the case, the id the row is about, and the code and message every
/// operation refuses that id with.
type IdRow = (String, String, &'static str, String);

/// One row of the metadata-options table: the case, the name the template is created under,
/// and the `MetadataOptions` members its writer sets, as `(member, value)` pairs.
type MetadataRow = (
    &'static str,
    &'static str,
    Vec<(&'static str, &'static str)>,
);

/// A `(key, value)` request parameter.
fn p(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

/// One `LaunchTemplateData.<member>` parameter.
fn data(member: &str, value: impl Into<String>) -> (String, String) {
    p(&format!("LaunchTemplateData.{member}"), value)
}

fn emulator() -> MultiServiceRouter {
    super::ec2_query::emulator().0
}

/// One call signed for [`REGION`].
async fn api(router: &MultiServiceRouter, action: &str, params: &[(String, String)]) -> (u16, String) {
    call(router, REGION, action, params).await
}

/// One call that must succeed, as its body.
async fn ok(
    router: &MultiServiceRouter,
    case: &str,
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

/// The text of the first `<name>` element anywhere in `body`, read through the XML parser.
fn first(case: &str, body: &str, name: &str) -> Option<String> {
    document(case, body)
        .descendants()
        .filter(|node| node.is_element() && node.tag_name().name() == name)
        .filter_map(|node| node.text().map(str::to_string))
        .next()
}

/// Create a launch template named `name` whose first version carries `version_data`, and
/// answer the id the create minted.
async fn create_template(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    version_data: &[(String, String)],
) -> String {
    let mut params = vec![p("LaunchTemplateName", name)];
    params.extend_from_slice(version_data);
    let body = ok(router, case, "CreateLaunchTemplate", &params).await;
    first(case, &body, "launchTemplateId")
        .unwrap_or_else(|| panic!("case {case:?}: the create must report its id: {body}"))
}

/// Add one version carrying `version_data` to `launch_template_id`, and answer its number.
async fn create_version(
    router: &MultiServiceRouter,
    case: &str,
    launch_template_id: &str,
    version_data: &[(String, String)],
) -> String {
    let mut params = vec![p("LaunchTemplateId", launch_template_id)];
    params.extend_from_slice(version_data);
    let body = ok(router, case, "CreateLaunchTemplateVersion", &params).await;
    first(case, &body, "versionNumber")
        .unwrap_or_else(|| panic!("case {case:?}: the create must report its version: {body}"))
}

/// How many instances the account holds, read back through `DescribeInstances`.
async fn instance_count(router: &MultiServiceRouter, case: &str) -> usize {
    let body = ok(router, case, "DescribeInstances", &[]).await;
    document(case, &body)
        .descendants()
        .filter(|node| node.is_element() && node.tag_name().name() == "instanceId")
        .count()
}

/// An id of a launch template this service minted and then took away: a value it really
/// issued that names no template of the account any more.
async fn minted_then_deleted(router: &MultiServiceRouter, case: &str, name: &str) -> String {
    let launch_template_id =
        create_template(router, case, name, &[data("ImageId", TEMPLATE_AMI)]).await;
    let body = ok(
        router,
        case,
        "DeleteLaunchTemplate",
        &[p("LaunchTemplateId", &launch_template_id)],
    )
    .await;
    assert_eq!(
        first(case, &body, "launchTemplateId").as_deref(),
        Some(launch_template_id.as_str()),
        "case {case:?}: the delete must report the template it took away: {body}"
    );
    launch_template_id
}

/// RULE 1: every operation that takes a launch template id screens it before it looks it up —
/// an id no mint of this service reached is unreadable, an id it minted whose template is gone
/// is absent.
#[tokio::test]
async fn an_id_is_screened_against_the_mint_before_it_is_looked_up() {
    let router = emulator();
    // A template that is never deleted, so each refusal below is about the id the row names
    // rather than about an account holding no templates at all.
    let kept = create_template(
        &router,
        "the template that is kept",
        "kept.template",
        &[data("ImageId", TEMPLATE_AMI)],
    )
    .await;
    let deleted = minted_then_deleted(&router, "a template that was taken away", "gone.template").await;

    // (case, the id, the code it is refused with, the message that code carries)
    let ids: Vec<IdRow> = vec![
        (
            "a long-form id far above every value the mint can reach".to_string(),
            "lt-0123456789abcdef0".to_string(),
            "InvalidLaunchTemplateId.Malformed",
            "Malformed launch template Id: lt-0123456789abcdef0".to_string(),
        ),
        (
            "a short-form id, a form this service never mints".to_string(),
            "lt-0fedcba9".to_string(),
            "InvalidLaunchTemplateId.Malformed",
            "Malformed launch template Id: lt-0fedcba9".to_string(),
        ),
        (
            "the all-zero placeholder id, which a mint counting from one cannot produce"
                .to_string(),
            "lt-00000000000000000".to_string(),
            "InvalidLaunchTemplateId.Malformed",
            "Malformed launch template Id: lt-00000000000000000".to_string(),
        ),
        (
            "an id that is not hexadecimal".to_string(),
            "lt-zzzzzzzzzzzzzzzzz".to_string(),
            "InvalidLaunchTemplateId.Malformed",
            "Malformed launch template Id: lt-zzzzzzzzzzzzzzzzz".to_string(),
        ),
        (
            "an id carrying another resource kind's prefix".to_string(),
            "sg-00000000000000001".to_string(),
            "InvalidLaunchTemplateId.Malformed",
            "Malformed launch template Id: sg-00000000000000001".to_string(),
        ),
        (
            "an id this service really issued, whose template has since been deleted".to_string(),
            deleted.clone(),
            "InvalidLaunchTemplateId.NotFound",
            format!("The specified launch template, with template ID {deleted}, does not exist."),
        ),
    ];

    // (the operation, how it names the id, and the other members it needs to get that far)
    let operations: Vec<(&str, NamesId)> = vec![
        ("DescribeLaunchTemplateVersions", |id| {
            vec![p("LaunchTemplateId", id)]
        }),
        ("DeleteLaunchTemplate", |id| {
            vec![p("LaunchTemplateId", id)]
        }),
        ("DeleteLaunchTemplateVersions", |id| {
            vec![
                p("LaunchTemplateId", id),
                p("LaunchTemplateVersion.1", "1"),
            ]
        }),
        ("CreateLaunchTemplateVersion", |id| {
            vec![p("LaunchTemplateId", id), data("InstanceType", "t3.micro")]
        }),
        ("ModifyLaunchTemplate", |id| {
            vec![p("LaunchTemplateId", id), p("SetDefaultVersion", "1")]
        }),
        ("RunInstances", |id| {
            vec![
                p("MinCount", "1"),
                p("MaxCount", "1"),
                p("LaunchTemplate.LaunchTemplateId", id),
            ]
        }),
    ];

    for (action, names_id) in operations {
        for (row, id, expected_code, expected_message) in &ids {
            let case = format!("{action}: {row}");
            let (status, body) = api(&router, action, &names_id(id)).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} with body {body}"
            );
            assert_eq!(
                &error_code(&body),
                expected_code,
                "case {case:?}: expected {expected_code}, got body {body}"
            );
            assert_eq!(
                &error_message(&body),
                expected_message,
                "case {case:?}: expected EC2's own sentence for that refusal, got body {body}"
            );
        }
    }

    // Every refusal only REPORTED: the template that was never deleted still resolves, by
    // either key, and nothing was launched for the refused launches.
    let body = ok(
        &router,
        "the template that is kept",
        "DescribeLaunchTemplateVersions",
        &[p("LaunchTemplateId", &kept)],
    )
    .await;
    assert_eq!(
        first("the template that is kept", &body, "launchTemplateId").as_deref(),
        Some(kept.as_str()),
        "the kept template must still resolve by its id, got {body}"
    );
    assert_eq!(
        instance_count(&router, "the template that is kept").await,
        0,
        "a refused launch must mint no instance"
    );
}

/// RULE 2: a launch pins a version the template holds, or it is refused and launches nothing.
#[tokio::test]
async fn a_launch_resolves_the_version_it_pins_against_the_template() {
    let router = emulator();
    let case = "version resolution";
    let name = "version.resolution";
    let launch_template_id = create_template(
        &router,
        case,
        name,
        &[data("ImageId", TEMPLATE_AMI), data("InstanceType", "t3.micro")],
    )
    .await;
    for expected in ["2", "3"] {
        let minted = create_version(
            &router,
            case,
            &launch_template_id,
            &[data("ImageId", TEMPLATE_AMI), data("InstanceType", "t3.small")],
        )
        .await;
        assert_eq!(
            minted, expected,
            "case {case:?}: versions are numbered in the order they are created, got {minted}"
        );
    }
    // Version 2 is taken away: a number the template once held and does not any more, which
    // the mint never hands out again.
    let body = ok(
        &router,
        case,
        "DeleteLaunchTemplateVersions",
        &[
            p("LaunchTemplateId", &launch_template_id),
            p("LaunchTemplateVersion.1", "2"),
        ],
    )
    .await;
    assert_eq!(
        first(case, &body, "versionNumber").as_deref(),
        Some("2"),
        "case {case:?}: the delete must report the version it took away: {body}"
    );

    // (case, the version the launch pins, the key it names the template by)
    let rows: Vec<(&str, &str, bool)> = vec![
        ("a number above every version ever minted", "4", true),
        ("a number far above every version ever minted", "99", true),
        ("a number below the first version", "0", true),
        ("a negative number", "-1", true),
        ("a version that was minted and then deleted", "2", true),
        (
            "a version that does not exist, the template named by its name",
            "4",
            false,
        ),
    ];

    for (row, version, by_id) in rows {
        let case = format!("refused version {version:?}: {row}");
        let before = instance_count(&router, &case).await;
        let mut params = vec![p("MinCount", "1"), p("MaxCount", "1")];
        if by_id {
            params.push(p("LaunchTemplate.LaunchTemplateId", &launch_template_id));
        } else {
            params.push(p("LaunchTemplate.LaunchTemplateName", name));
        }
        params.push(p("LaunchTemplate.Version", version));
        let (status, body) = api(&router, "RunInstances", &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidLaunchTemplateId.VersionNotFound",
            "case {case:?}: expected the version-not-found code, got body {body}"
        );
        // The template's ID is named even where the request named the template by its NAME:
        // the id comes from the template the account holds, the version from the request.
        let expected = format!(
            "Could not find the specified version {version} for the launch template with \
             ID {launch_template_id}."
        );
        let message = error_message(&body);
        assert_eq!(
            message, expected,
            "case {case:?}: expected EC2's own sentence, got {message:?} in {body}"
        );
        let after = instance_count(&router, &case).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused launch must mint no instance, went from {before} to \
             {after}"
        );
    }

    // The versions the template DOES hold still launch, by number and by either alias: the
    // latest is 3, the default is still the first version.
    for (row, version, expected_type) in [
        ("the latest version by its alias", "$Latest", "t3.small"),
        ("the default version by its alias", "$Default", "t3.micro"),
        ("a version the template holds, by number", "1", "t3.micro"),
    ] {
        let case = format!("launched from {version:?}: {row}");
        let body = ok(
            &router,
            &case,
            "RunInstances",
            &[
                p("MinCount", "1"),
                p("MaxCount", "1"),
                p("LaunchTemplate.LaunchTemplateId", &launch_template_id),
                p("LaunchTemplate.Version", version),
            ],
        )
        .await;
        let reported = document(&case, &body)
            .descendants()
            .find(|node| node.is_element() && node.tag_name().name() == "instancesSet")
            .and_then(|set| {
                set.children()
                    .find(|node| node.is_element() && node.tag_name().name() == "item")
            })
            .and_then(|item| child_text(item, "instanceType"))
            .unwrap_or_else(|| panic!("case {case:?}: the launch must report its instance: {body}"));
        assert_eq!(
            reported, expected_type,
            "case {case:?}: expected the version's own instance type, got {reported:?} in {body}"
        );
    }
}

/// RULE 3: a version's data reads back as the data its writer submitted — the metadata options
/// the request set, and no member the request shape cannot carry.
#[tokio::test]
async fn a_version_reports_the_metadata_options_its_writer_set_and_no_others() {
    let router = emulator();

    // (case, the `MetadataOptions` members the writer sets, as (member, value) pairs)
    let rows: Vec<MetadataRow> = vec![
        (
            "one member, the token requirement",
            "metadata.tokens",
            vec![("HttpTokens", "required")],
        ),
        (
            "the endpoint and the hop limit",
            "metadata.endpoint",
            vec![("HttpEndpoint", "enabled"), ("HttpPutResponseHopLimit", "3")],
        ),
        (
            "the tags member alone",
            "metadata.tags",
            vec![("InstanceMetadataTags", "enabled")],
        ),
        (
            "every member the request shape carries",
            "metadata.every",
            vec![
                ("HttpTokens", "optional"),
                ("HttpPutResponseHopLimit", "64"),
                ("HttpEndpoint", "disabled"),
                ("HttpProtocolIpv6", "enabled"),
                ("InstanceMetadataTags", "disabled"),
            ],
        ),
    ];

    for (case, name, options) in rows {
        let mut version_data = vec![data("ImageId", TEMPLATE_AMI)];
        for (member, value) in &options {
            version_data.push(data(&format!("MetadataOptions.{member}"), *value));
        }
        let launch_template_id = create_template(&router, case, name, &version_data).await;

        // The read, and the response the writer of a DERIVED version gets, report the one
        // projection of the stored data: both are asserted, so neither can grow a member of
        // its own.
        let described = ok(
            &router,
            case,
            "DescribeLaunchTemplateVersions",
            &[p("LaunchTemplateId", &launch_template_id)],
        )
        .await;
        let derived = ok(
            &router,
            case,
            "CreateLaunchTemplateVersion",
            &[
                p("LaunchTemplateId", &launch_template_id),
                p("SourceVersion", "1"),
                data("InstanceType", "t3.small"),
            ],
        )
        .await;

        for (read, body) in [("the read", &described), ("the derived version", &derived)] {
            let case = format!("{case}: {read}");
            let parsed = document(&case, body);
            let reported = parsed
                .descendants()
                .find(|node| node.is_element() && node.tag_name().name() == "metadataOptions")
                .unwrap_or_else(|| {
                    panic!("case {case:?}: the version must report its metadata options: {body}")
                });
            // Every member the writer set, read back off the version.
            for (member, value) in &options {
                let wire = wire_name(member);
                let answered = child_text(reported, &wire);
                assert_eq!(
                    answered.as_deref(),
                    Some(*value),
                    "case {case:?}: expected metadataOptions.{wire} {value:?}, got \
                     {answered:?} in {body}"
                );
            }
            // "The state of the metadata option changes" is a fact of APPLYING options to a
            // running instance, and the request shape carries no member for it, so a stored
            // version says nothing about it: a client that hands this data back to
            // `CreateLaunchTemplateVersion` would be refused for a member it never set.
            assert!(
                child(reported, "state").is_none(),
                "case {case:?}: the version's metadata options must carry no state member, \
                 got {body}"
            );
            // Nothing else was invented either: the members reported are exactly the ones
            // the writer set.
            let answered: Vec<String> = reported
                .children()
                .filter(|node| node.is_element())
                .map(|node| node.tag_name().name().to_string())
                .collect();
            let mut expected: Vec<String> =
                options.iter().map(|(member, _)| wire_name(member)).collect();
            expected.sort();
            let mut sorted = answered.clone();
            sorted.sort();
            assert_eq!(
                sorted, expected,
                "case {case:?}: expected exactly the members the writer set, got \
                 {answered:?} in {body}"
            );
        }
    }
}

/// The wire name of one `LaunchTemplateInstanceMetadataOptions` member: ec2Query writes a
/// member under its own name with the first letter lowercased
/// (`smithy_query(query="shape",shape="LaunchTemplateInstanceMetadataOptions")` declaring no
/// `xmlName` of its own for any of them).
fn wire_name(member: &str) -> String {
    let mut characters = member.chars();
    match characters.next() {
        None => String::new(),
        Some(first) => first.to_ascii_lowercase().to_string() + characters.as_str(),
    }
}
