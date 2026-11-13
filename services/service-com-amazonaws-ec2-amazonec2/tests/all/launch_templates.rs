// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the EC2 launch template slice: `CreateLaunchTemplate` and
//! `DeleteLaunchTemplate`, driven over the wire the service actually speaks — `ec2Query`,
//! an `application/x-www-form-urlencoded` POST answered with XML (README.md /
//! OPERATIONS.md "Wire protocol"), through the crate's shared [`super::ec2_query`]
//! harness.
//!
//! The rule under test is that a launch template is PERSISTED STATE: a create stores one
//! and answers with what it stored, and a delete removes exactly the template the request
//! names — with every version of it — and answers with that template as it was, so the id
//! and the name it carried resolve to nothing afterwards. Each table row is a distinct
//! input along what that generalises over: the name's length and characters, how many tags
//! the create carries, whether it carries a version description or an operator, which
//! launch parameters its data holds, and whether the delete names the template by id or by
//! name.

use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, FIRST_LAUNCH_TEMPLATE_VERSION,
    LaunchTemplateRecord, MAX_LAUNCH_TEMPLATE_NAME_CHARS, MIN_LAUNCH_TEMPLATE_NAME_CHARS,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, elements, error_code};

/// The Region every call here is signed for. A launch template carries no Region member,
/// so the Region only selects the ec2Query envelope.
const REGION: &str = "us-east-1";

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

/// One row of the delete-resolution table: what the case is, the parameters the delete
/// carries, and the error code it must be refused with.
type DeleteCase = (&'static str, Vec<(String, String)>, &'static str);

/// The stored record for `launch_template_id`, as the store holds it.
fn stored(engine: &engine::InMemoryEngine, launch_template_id: &str) -> Option<LaunchTemplateRecord> {
    engine
        .state::<Ec2State>()
        .lock()
        .launch_templates
        .get(launch_template_id)
        .cloned()
}

/// How many launch templates the store holds.
fn stored_count(engine: &engine::InMemoryEngine) -> usize {
    engine.state::<Ec2State>().lock().launch_templates.len()
}

/// Whether `id` is the id form this service mints: `lt-` and 17 lowercase hex digits.
fn is_minted_id(id: &str) -> bool {
    id.strip_prefix("lt-").is_some_and(|suffix| {
        suffix.len() == 17 && suffix.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}

/// Which of the two keys a delete names the template by — the API takes "either the launch
/// template ID or the launch template name, but not both".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DeleteBy {
    Id,
    Name,
}

/// One row of the create/delete table: a template to create, and how to delete it.
struct Row {
    case: &'static str,
    name: String,
    version_description: Option<String>,
    /// `LaunchTemplateData.*` parameters: the launch parameters the version holds.
    data: Vec<(String, String)>,
    tags: Vec<(String, String)>,
    operator_principal: Option<String>,
    delete_by: DeleteBy,
}

/// The create parameters for one row.
fn create_params(row: &Row) -> Vec<(String, String)> {
    let mut params = vec![p("LaunchTemplateName", row.name.clone())];
    params.extend(row.data.iter().cloned());
    if let Some(description) = &row.version_description {
        params.push(p("VersionDescription", description.clone()));
    }
    if !row.tags.is_empty() {
        params.push(p("TagSpecification.1.ResourceType", "launch-template"));
        for (index, (key, value)) in row.tags.iter().enumerate() {
            params.push(p(
                &format!("TagSpecification.1.Tag.{}.Key", index + 1),
                key.clone(),
            ));
            params.push(p(
                &format!("TagSpecification.1.Tag.{}.Value", index + 1),
                value.clone(),
            ));
        }
    }
    if let Some(principal) = &row.operator_principal {
        params.push(p("Operator.Principal", principal.clone()));
    }
    params
}

/// One `LaunchTemplateData.<member>` parameter.
fn data(member: &str, value: &str) -> (String, String) {
    p(&format!("LaunchTemplateData.{member}"), value)
}

/// The rows the create/delete table is written against. They differ along every attribute
/// the rule generalises over: the name's length and character classes, the number of tags,
/// whether a version description or an operator is given, how many and which launch
/// parameters the data carries, and which key the delete names the template by.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "shortest name, one launch parameter, no tags, deleted by id",
            name: "abc".to_string(),
            version_description: None,
            data: vec![data("ImageId", "ami-0abcdef1234567890")],
            tags: Vec::new(),
            operator_principal: None,
            delete_by: DeleteBy::Id,
        },
        Row {
            case: "longest name, deleted by name",
            name: "n".repeat(MAX_LAUNCH_TEMPLATE_NAME_CHARS),
            version_description: Some("the first version".to_string()),
            data: vec![data("InstanceType", "t3.micro")],
            tags: Vec::new(),
            operator_principal: None,
            delete_by: DeleteBy::Name,
        },
        Row {
            case: "every character class the name pattern admits, three tags",
            name: "Web(1).tier-a/b_C2".to_string(),
            version_description: None,
            data: vec![
                data("ImageId", "ami-00000000000000001"),
                data("InstanceType", "t3.small"),
                data("UserData", "IyEvYmluL2Jhc2gKZWNobyBoaQo="),
            ],
            tags: vec![
                ("Name".to_string(), "web".to_string()),
                ("Owner".to_string(), "TeamA".to_string()),
                ("Stage".to_string(), String::new()),
            ],
            operator_principal: None,
            delete_by: DeleteBy::Name,
        },
        Row {
            case: "lists and nested members in the data, one tag, deleted by id",
            name: "batch.workers".to_string(),
            version_description: Some(String::new()),
            data: vec![
                data("ImageId", "ami-00000000000000002"),
                data("SecurityGroupId.1", "sg-00000000000000001"),
                data("SecurityGroupId.2", "sg-00000000000000002"),
                data("Monitoring.Enabled", "true"),
                data("EbsOptimized", "true"),
            ],
            tags: vec![("Name".to_string(), "batch".to_string())],
            operator_principal: None,
            delete_by: DeleteBy::Id,
        },
        Row {
            case: "a managing service provider, longest version description",
            name: "managed_template".to_string(),
            version_description: Some("d".repeat(255)),
            data: vec![data("ImageId", "ami-00000000000000003")],
            tags: Vec::new(),
            operator_principal: Some("autoscaling.amazonaws.com".to_string()),
            delete_by: DeleteBy::Id,
        },
    ]
}

/// A create stores the template and reports what it stored; the delete that follows
/// removes it with its versions and reports the same template, after which neither its id
/// nor its name resolves.
#[tokio::test]
async fn a_created_launch_template_is_stored_reported_and_then_deleted() {
    let (router, engine) = super::ec2_query::emulator();
    let created_by = engine::emulator_arn("iam", "", "root");

    for row in rows() {
        let case = row.case;
        let (status, body) = api(&router, "CreateLaunchTemplate", &create_params(&row)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from CreateLaunchTemplate, got {status} with body {body}"
        );

        let launch_template_id = element(&body, "launchTemplateId").unwrap_or_default();
        assert!(
            is_minted_id(&launch_template_id),
            "case {case:?}: expected an 'lt-' id of 17 hex digits, got {launch_template_id:?}"
        );
        let reported_name = element(&body, "launchTemplateName").unwrap_or_default();
        assert_eq!(
            reported_name, row.name,
            "case {case:?}: expected the name the request gave, got {reported_name:?}"
        );
        assert_eq!(
            element(&body, "createdBy").unwrap_or_default(),
            created_by,
            "case {case:?}: expected the creating principal {created_by:?}, got body {body}"
        );
        assert_eq!(
            element(&body, "defaultVersionNumber").unwrap_or_default(),
            FIRST_LAUNCH_TEMPLATE_VERSION.to_string(),
            "case {case:?}: the only version is the default one, got body {body}"
        );
        assert_eq!(
            element(&body, "latestVersionNumber").unwrap_or_default(),
            FIRST_LAUNCH_TEMPLATE_VERSION.to_string(),
            "case {case:?}: the only version is the latest one, got body {body}"
        );
        let create_time = element(&body, "createTime").unwrap_or_default();
        assert!(
            ::aws_smithy_types::DateTime::from_str(
                &create_time,
                ::aws_smithy_types::date_time::Format::DateTime,
            )
            .is_ok(),
            "case {case:?}: expected an ISO 8601 createTime, got {create_time:?}"
        );
        let reported_tags: Vec<(String, String)> = elements(&body, "key")
            .into_iter()
            .zip(elements(&body, "value"))
            .collect();
        let expected_tags: Vec<(String, String)> = row.tags.clone();
        assert_eq!(
            reported_tags, expected_tags,
            "case {case:?}: expected the tags the request gave, got {reported_tags:?}"
        );
        if let Some(principal) = &row.operator_principal {
            assert_eq!(
                element(&body, "principal").as_deref(),
                Some(principal.as_str()),
                "case {case:?}: expected the managing principal in the operator, got body {body}"
            );
            assert_eq!(
                element(&body, "managed").unwrap_or_default(),
                "true",
                "case {case:?}: a template a provider manages is reported managed, got body {body}"
            );
        } else {
            assert!(
                element(&body, "principal").is_none(),
                "case {case:?}: no operator was asked for, so none may be reported: {body}"
            );
        }

        // The STORE, not the response, is what the create has to have changed: one record
        // holding exactly the one version, with the launch parameters the request carried.
        let record = stored(&engine, &launch_template_id)
            .unwrap_or_else(|| panic!("case {case:?}: the created template must be stored"));
        assert_eq!(
            record.versions.len(),
            1,
            "case {case:?}: a create stores exactly one version, got {:?}",
            record.versions.keys().collect::<Vec<_>>()
        );
        let version = record
            .versions
            .get(&FIRST_LAUNCH_TEMPLATE_VERSION)
            .unwrap_or_else(|| panic!("case {case:?}: version 1 must be stored"));
        assert_eq!(
            version.version_description, row.version_description,
            "case {case:?}: expected the version description the request gave, got {:?}",
            version.version_description
        );
        for (member, value) in &row.data {
            let expected = match member.as_str() {
                "LaunchTemplateData.ImageId" => version.data.image_id.clone(),
                "LaunchTemplateData.InstanceType" => version.data.instance_type.clone(),
                "LaunchTemplateData.UserData" => version.data.user_data.clone(),
                "LaunchTemplateData.EbsOptimized" => {
                    version.data.ebs_optimized.map(|set| set.to_string())
                }
                "LaunchTemplateData.Monitoring.Enabled" => version
                    .data
                    .monitoring
                    .as_ref()
                    .and_then(|monitoring| monitoring.enabled)
                    .map(|set| set.to_string()),
                "LaunchTemplateData.SecurityGroupId.1" => version
                    .data
                    .security_group_ids
                    .as_ref()
                    .and_then(|ids| ids.first().cloned()),
                "LaunchTemplateData.SecurityGroupId.2" => version
                    .data
                    .security_group_ids
                    .as_ref()
                    .and_then(|ids| ids.get(1).cloned()),
                other => panic!("case {case:?}: the table has no reader for {other}"),
            };
            assert_eq!(
                expected.as_deref(),
                Some(value.as_str()),
                "case {case:?}: expected the stored version to carry {member}={value}, \
                 got {expected:?}"
            );
        }

        // Deleting by either key removes that one template and answers with it as it was.
        let delete_params = match row.delete_by {
            DeleteBy::Id => vec![p("LaunchTemplateId", launch_template_id.clone())],
            DeleteBy::Name => vec![p("LaunchTemplateName", row.name.clone())],
        };
        let (status, deleted) = api(&router, "DeleteLaunchTemplate", &delete_params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DeleteLaunchTemplate, got {status} with body {deleted}"
        );
        assert_eq!(
            element(&deleted, "launchTemplateId").unwrap_or_default(),
            launch_template_id,
            "case {case:?}: the delete must report the template it removed, got {deleted}"
        );
        assert_eq!(
            element(&deleted, "launchTemplateName").unwrap_or_default(),
            row.name,
            "case {case:?}: the delete must report the template it removed, got {deleted}"
        );
        assert_eq!(
            element(&deleted, "createTime").unwrap_or_default(),
            create_time,
            "case {case:?}: the create time of a template never changes, got {deleted}"
        );

        assert!(
            stored(&engine, &launch_template_id).is_none(),
            "case {case:?}: the deleted template, and its versions, must be gone from the store"
        );
        assert_eq!(
            stored_count(&engine),
            0,
            "case {case:?}: the delete must remove exactly the one template"
        );

        // Neither key resolves any more, and each is refused with the code for the key the
        // request used.
        let (status, again) = api(&router, "DeleteLaunchTemplate", &delete_params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a second delete names nothing, got {status} with body {again}"
        );
        let expected_code = match row.delete_by {
            DeleteBy::Id => "InvalidLaunchTemplateId.NotFound",
            DeleteBy::Name => "InvalidLaunchTemplateName.NotFoundException",
        };
        assert_eq!(
            error_code(&again),
            expected_code,
            "case {case:?}: expected {expected_code}, got body {again}"
        );
    }
}

/// A name is accepted exactly when the modelled `LaunchTemplateName` can carry it: 3 to
/// 128 characters of `^[a-zA-Z0-9\(\)\.\-/_]+$`. The rows are both published length bounds,
/// one past each, every character class the pattern admits, and characters it does not.
#[tokio::test]
async fn a_name_is_accepted_exactly_when_the_modelled_name_can_carry_it() {
    let (router, engine) = super::ec2_query::emulator();

    let cases: Vec<(&str, String, bool)> = vec![
        (
            "the published minimum length",
            "a".repeat(MIN_LAUNCH_TEMPLATE_NAME_CHARS),
            true,
        ),
        (
            "one character below the minimum",
            "a".repeat(MIN_LAUNCH_TEMPLATE_NAME_CHARS - 1),
            false,
        ),
        (
            "the published maximum length",
            "b".repeat(MAX_LAUNCH_TEMPLATE_NAME_CHARS),
            true,
        ),
        (
            "one character above the maximum",
            "c".repeat(MAX_LAUNCH_TEMPLATE_NAME_CHARS + 1),
            false,
        ),
        ("digits only", "123".to_string(), true),
        ("mixed case letters", "AbCdEf".to_string(), true),
        (
            "every punctuation the pattern admits",
            "x()./-_y".to_string(),
            true,
        ),
        ("a space", "a b".to_string(), false),
        ("an asterisk", "a*b".to_string(), false),
        ("a plus sign", "a+b".to_string(), false),
        ("an at sign", "a@b".to_string(), false),
        ("a colon", "a:b".to_string(), false),
        ("a backslash", "a\\b".to_string(), false),
        ("a non-ASCII letter", "naïve".to_string(), false),
        ("empty", String::new(), false),
    ];

    let mut accepted = 0;
    for (case, name, is_accepted) in cases {
        let params = vec![
            p("LaunchTemplateName", name.clone()),
            data("ImageId", "ami-0abcdef1234567890"),
        ];
        let (status, body) = api(&router, "CreateLaunchTemplate", &params).await;
        if is_accepted {
            accepted += 1;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 for name {name:?}, got {status} with body {body}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400 for name {name:?}, got {status} with body {body}"
            );
        }
        assert_eq!(
            stored_count(&engine),
            accepted,
            "case {case:?}: only an accepted name may store a template"
        );
    }
}

/// A create that carries no launch parameter at all is refused: "you must specify at least
/// one parameter for the launch template data".
#[tokio::test]
async fn a_create_with_no_launch_parameter_is_refused() {
    let (router, engine) = super::ec2_query::emulator();

    let (status, body) = api(
        &router,
        "CreateLaunchTemplate",
        &[p("LaunchTemplateName", "empty-data")],
    )
    .await;
    assert_eq!(
        status, 400,
        "empty launch template data: expected HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        stored_count(&engine),
        0,
        "empty launch template data: nothing may be stored, got body {body}"
    );
}

/// A name names one template: a second create under a name the account already holds is
/// refused, and the name becomes free again once the template is deleted.
#[tokio::test]
async fn a_name_names_one_template_until_it_is_deleted() {
    let (router, engine) = super::ec2_query::emulator();

    let first = vec![
        p("LaunchTemplateName", "shared-name"),
        data("ImageId", "ami-00000000000000001"),
    ];
    let (status, body) = api(&router, "CreateLaunchTemplate", &first).await;
    assert_eq!(status, 200, "first create: got {status} with body {body}");
    let first_id = element(&body, "launchTemplateId").unwrap_or_default();

    // A different request under the same name — different data, so this is not an
    // idempotent repeat.
    let second = vec![
        p("LaunchTemplateName", "shared-name"),
        data("InstanceType", "t3.large"),
    ];
    let (status, body) = api(&router, "CreateLaunchTemplate", &second).await;
    assert_eq!(
        status, 400,
        "second create under one name: got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidLaunchTemplateName.AlreadyExistsException",
        "second create under one name: got body {body}"
    );
    assert_eq!(
        stored_count(&engine),
        1,
        "a refused create stores nothing: got body {body}"
    );

    let (status, body) = api(
        &router,
        "DeleteLaunchTemplate",
        &[p("LaunchTemplateName", "shared-name")],
    )
    .await;
    assert_eq!(status, 200, "delete: got {status} with body {body}");

    let (status, body) = api(&router, "CreateLaunchTemplate", &second).await;
    assert_eq!(
        status, 200,
        "create after the delete: got {status} with body {body}"
    );
    let third_id = element(&body, "launchTemplateId").unwrap_or_default();
    assert_ne!(
        third_id, first_id,
        "a reused name gets a new template, not the deleted one's id"
    );
}

/// A delete names the template by id or by name, never both and never neither, and an id
/// is checked for shape before it is looked up.
#[tokio::test]
async fn a_delete_resolves_one_key_and_refuses_the_rest() {
    let (router, engine) = super::ec2_query::emulator();

    let (status, body) = api(
        &router,
        "CreateLaunchTemplate",
        &[
            p("LaunchTemplateName", "resolver"),
            data("ImageId", "ami-00000000000000001"),
        ],
    )
    .await;
    assert_eq!(status, 200, "setup create: got {status} with body {body}");
    let known_id = element(&body, "launchTemplateId").unwrap_or_default();

    // Rows: neither key, both keys, a malformed id (not hex, no prefix, the wrong length),
    // a well-formed id of each published id length that names nothing, and an unknown name.
    let cases: Vec<DeleteCase> = vec![
        (
            "neither key",
            Vec::new(),
            "MissingParameter",
        ),
        (
            "both keys",
            vec![
                p("LaunchTemplateId", known_id.clone()),
                p("LaunchTemplateName", "resolver"),
            ],
            "InvalidParameterCombination",
        ),
        (
            "an id that is not hexadecimal",
            vec![p("LaunchTemplateId", "lt-zzzzzzzzzzzzzzzzz")],
            "InvalidLaunchTemplateId.Malformed",
        ),
        (
            "an id without the resource prefix",
            vec![p("LaunchTemplateId", "00000000000000001")],
            "InvalidLaunchTemplateId.Malformed",
        ),
        (
            "an id of a length no EC2 id has",
            vec![p("LaunchTemplateId", "lt-0123456")],
            "InvalidLaunchTemplateId.Malformed",
        ),
        (
            "an id of another resource kind",
            vec![p("LaunchTemplateId", "sg-00000000000000001")],
            "InvalidLaunchTemplateId.Malformed",
        ),
        (
            "a well-formed long id no mint of this service ever reached",
            vec![p("LaunchTemplateId", "lt-0fedcba987654321f")],
            "InvalidLaunchTemplateId.Malformed",
        ),
        (
            "a well-formed short id no mint of this service ever reached",
            vec![p("LaunchTemplateId", "lt-0fedcba9")],
            "InvalidLaunchTemplateId.Malformed",
        ),
        (
            "a name nothing was created under",
            vec![p("LaunchTemplateName", "no-such-template")],
            "InvalidLaunchTemplateName.NotFoundException",
        ),
    ];

    for (case, params, expected_code) in cases {
        let (status, body) = api(&router, "DeleteLaunchTemplate", &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected {expected_code}, got body {body}"
        );
        assert!(
            stored(&engine, &known_id).is_some(),
            "case {case:?}: a refused delete must leave the stored template alone"
        );
    }
}

/// `DryRun` reports the outcome a permitted caller would have got — `DryRunOperation`,
/// HTTP 412 — and changes nothing, on both the create and the delete.
#[tokio::test]
async fn dry_run_reports_the_permitted_outcome_without_writing() {
    let (router, engine) = super::ec2_query::emulator();

    let create = vec![
        p("LaunchTemplateName", "dry-run"),
        data("ImageId", "ami-00000000000000001"),
    ];
    let mut dry_create = create.clone();
    dry_create.push(p("DryRun", "true"));
    let (status, body) = api(&router, "CreateLaunchTemplate", &dry_create).await;
    assert_eq!(
        status, 412,
        "dry-run create: expected HTTP 412, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "DryRunOperation",
        "dry-run create: got body {body}"
    );
    assert_eq!(
        stored_count(&engine),
        0,
        "dry-run create: nothing may be stored"
    );

    let (status, body) = api(&router, "CreateLaunchTemplate", &create).await;
    assert_eq!(status, 200, "create: got {status} with body {body}");
    let launch_template_id = element(&body, "launchTemplateId").unwrap_or_default();

    let (status, body) = api(
        &router,
        "DeleteLaunchTemplate",
        &[
            p("LaunchTemplateId", launch_template_id.clone()),
            p("DryRun", "true"),
        ],
    )
    .await;
    assert_eq!(
        status, 412,
        "dry-run delete: expected HTTP 412, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "DryRunOperation",
        "dry-run delete: got body {body}"
    );
    assert!(
        stored(&engine, &launch_template_id).is_some(),
        "dry-run delete: the template must still be stored"
    );

    // A dry run that would have been REFUSED is refused with that reason instead.
    let (status, body) = api(
        &router,
        "DeleteLaunchTemplate",
        &[
            p("LaunchTemplateId", "lt-0fedcba987654321f"),
            p("DryRun", "true"),
        ],
    )
    .await;
    assert_eq!(
        status, 400,
        "dry-run delete of an unreadable template id: got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidLaunchTemplateId.Malformed",
        "dry-run delete of an unreadable template id: got body {body}"
    );

    let (status, body) = api(
        &router,
        "DeleteLaunchTemplate",
        &[p("LaunchTemplateId", launch_template_id.clone())],
    )
    .await;
    assert_eq!(status, 200, "delete: got {status} with body {body}");
    assert_eq!(
        stored_count(&engine),
        0,
        "delete: the template must be gone"
    );
}

/// A `ClientToken` makes a repeated create answer with the one template it created, and a
/// token whose template was deleted creates a new one rather than naming the deleted id.
#[tokio::test]
async fn a_client_token_makes_a_repeated_create_answer_with_the_one_template() {
    let (router, engine) = super::ec2_query::emulator();

    let params = vec![
        p("LaunchTemplateName", "idempotent"),
        data("ImageId", "ami-00000000000000001"),
        p("ClientToken", "token-0001"),
    ];
    let (status, first) = api(&router, "CreateLaunchTemplate", &params).await;
    assert_eq!(status, 200, "first create: got {status} with body {first}");
    let first_id = element(&first, "launchTemplateId").unwrap_or_default();

    let (status, repeat) = api(&router, "CreateLaunchTemplate", &params).await;
    assert_eq!(status, 200, "repeat: got {status} with body {repeat}");
    assert_eq!(
        element(&repeat, "launchTemplateId").unwrap_or_default(),
        first_id,
        "a repeat under one token answers with the template it created, got {repeat}"
    );
    assert_eq!(
        stored_count(&engine),
        1,
        "a repeat under one token creates no second template"
    );

    // A different token under the same name is a different request, and the name is taken.
    let mut other_token = params.clone();
    other_token.retain(|(key, _)| key != "ClientToken");
    other_token.push(p("ClientToken", "token-0002"));
    let (status, body) = api(&router, "CreateLaunchTemplate", &other_token).await;
    assert_eq!(
        status, 400,
        "another token, same name: got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidLaunchTemplateName.AlreadyExistsException",
        "another token, same name: got body {body}"
    );

    let (status, body) = api(
        &router,
        "DeleteLaunchTemplate",
        &[p("LaunchTemplateId", first_id.clone())],
    )
    .await;
    assert_eq!(status, 200, "delete: got {status} with body {body}");

    // The token no longer names anything, so the same request creates a new template.
    let (status, body) = api(&router, "CreateLaunchTemplate", &params).await;
    assert_eq!(
        status, 200,
        "create after the delete: got {status} with body {body}"
    );
    let new_id = element(&body, "launchTemplateId").unwrap_or_default();
    assert_ne!(
        new_id, first_id,
        "a token whose template was deleted must not answer with the deleted id"
    );
    assert!(
        stored(&engine, &new_id).is_some(),
        "create after the delete: the new template must be stored"
    );
}

/// A tag specification must name the one resource type this create tags —
/// `launch-template` — and the tags it carries land on the template.
#[tokio::test]
async fn a_tag_specification_must_name_the_launch_template_resource_type() {
    let (router, engine) = super::ec2_query::emulator();

    // Rows: the resource types `RunInstances` tags, the type of another resource of this
    // service, a type that is not a resource type at all, and no type at all.
    let refused = ["instance", "volume", "network-interface", "vpc", "", "fleet"];
    for (index, resource_type) in refused.iter().enumerate() {
        let mut params = vec![
            p("LaunchTemplateName", format!("tagged-{index}")),
            data("ImageId", "ami-00000000000000001"),
            p("TagSpecification.1.Tag.1.Key", "Name"),
            p("TagSpecification.1.Tag.1.Value", "web"),
        ];
        if !resource_type.is_empty() {
            params.push(p("TagSpecification.1.ResourceType", *resource_type));
        }
        let (status, body) = api(&router, "CreateLaunchTemplate", &params).await;
        assert_eq!(
            status, 400,
            "case {resource_type:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValue",
            "case {resource_type:?}: got body {body}"
        );
        assert_eq!(
            stored_count(&engine),
            0,
            "case {resource_type:?}: a refused create stores nothing"
        );
    }

    let params = vec![
        p("LaunchTemplateName", "tagged"),
        data("ImageId", "ami-00000000000000001"),
        p("TagSpecification.1.ResourceType", "launch-template"),
        p("TagSpecification.1.Tag.1.Key", "Name"),
        p("TagSpecification.1.Tag.1.Value", "web"),
    ];
    let (status, body) = api(&router, "CreateLaunchTemplate", &params).await;
    assert_eq!(
        status, 200,
        "case \"launch-template\": expected HTTP 200, got {status} with body {body}"
    );
    let launch_template_id = element(&body, "launchTemplateId").unwrap_or_default();
    let record = stored(&engine, &launch_template_id)
        .expect("the tagged template must be stored");
    let tags = record.template.tags.clone().unwrap_or_default();
    assert_eq!(
        tags.len(),
        1,
        "case \"launch-template\": expected the one tag the request gave, got {tags:?}"
    );
    assert_eq!(
        (tags[0].key.as_deref(), tags[0].value.as_deref()),
        (Some("Name"), Some("web")),
        "case \"launch-template\": expected Name=web on the stored template, got {tags:?}"
    );
}

mod describe_launch_template_versions {
    //! End-to-end cover for `DescribeLaunchTemplateVersions`, driven over the wire the service
    //! actually speaks — `ec2Query`, an `application/x-www-form-urlencoded` POST answered with
    //! XML (README.md / OPERATIONS.md "Wire protocol"), through the crate's shared
    //! [`crate::ec2_query`] harness.
    //!
    //! The rule under test is that the operation REPORTS PERSISTED STATE: every version it
    //! answers with is a version the store holds, under the template that holds it, and the
    //! request only selects which of them are reported — by template (id or name), by version
    //! (number or the `$Latest`/`$Default` alias), by version range, by the documented filters
    //! and by page. Each table row is a distinct input along what that generalises over.
    //!
    //! `CreateLaunchTemplateVersion` creates the versions after the first, so the suites below
    //! that need a template with MANY versions seed the extra ones into the store that
    //! [`ec2::state::ComAmazonawsEc2Amazonec2State::launch_templates`] holds — the same record
    //! both template writers write and this operation reads — which is also how a version with
    //! a chosen create time or a since-withdrawn image alias is put there.

    use std::time::{Duration, SystemTime};

    use ec2::ops::describe_launch_template_versions::SSM_IMAGE_ALIAS_PREFIX;
    use ec2::ops::types::{
        LaunchTemplateIamInstanceProfileSpecificationRequest,
        LaunchTemplateInstanceMetadataOptionsRequest,
        LaunchTemplateInstanceNetworkInterfaceSpecificationRequest,
        LaunchTemplateLicenseConfigurationRequest, LaunchTemplatePlacementRequest, OperatorResponse,
        RequestLaunchTemplateData,
    };
    use ec2::state::{
        ComAmazonawsEc2Amazonec2State as Ec2State, LaunchTemplateVersionRecord, MAX_FILTER_VALUE_BYTES,
        MAX_FILTER_VALUES, MAX_FILTERS, MAX_RESOURCE_IDS,
    };
    use service_com_amazonaws_ec2_amazonec2 as ec2;
    use smithy_server_runtime::MultiServiceRouter;

    use crate::ec2_query::{element, elements, error_code, filters, indexed};

    /// The Region every call here is signed for. A launch template version carries no Region
    /// member, so the Region only selects the ec2Query envelope.
    const REGION: &str = "us-east-1";

    /// The `(key, value)` form fields of one request.
    type Params = Vec<(String, String)>;

    /// One row of the round-trip table: the case, the template's name, the create's launch
    /// parameters, and the `(element, value)` pairs the describe must report.
    type RoundTripRow = (
        &'static str,
        &'static str,
        Params,
        Vec<(&'static str, &'static str)>,
    );

    /// One row of a selection table: the case, the selecting parameters, and the version
    /// numbers the describe must report, in order.
    type SelectionRow = (&'static str, Params, Vec<i64>);

    /// One row of a refusal table: the case, the parameters, and the error code the request must
    /// be refused with.
    type RefusalRow = (&'static str, Params, &'static str);

    /// One row of the account-wide table: the case, the version aliases named, and the
    /// `(template id, version number)` pairs the describe must report, in order.
    type AccountRow = (&'static str, Vec<&'static str>, Vec<(String, i64)>);

    /// One row of the filter table: the filter's name, its values, and the version numbers it
    /// must select.
    type FilterRow<'a> = (&'a str, Vec<&'a str>, Vec<i64>);

    /// One row of the several-filters table: the case, the filters, and the version numbers they
    /// must select together.
    type ConjunctionRow = (
        &'static str,
        Vec<(&'static str, &'static [&'static str])>,
        Vec<i64>,
    );

    /// One version to seed into a stored template: its number, how many seconds ago it was
    /// created, and the launch parameters it holds.
    type SeededVersion = (i64, u64, RequestLaunchTemplateData);

    /// One ec2Query call through the shared harness.
    async fn api(
        router: &MultiServiceRouter,
        action: &str,
        params: &[(String, String)],
    ) -> (u16, String) {
        crate::ec2_query::call(router, REGION, action, params).await
    }

    /// A `(key, value)` request parameter.
    fn p(key: &str, value: impl Into<String>) -> (String, String) {
        (key.to_string(), value.into())
    }

    /// One `LaunchTemplateData.<member>` parameter of a create.
    fn data(member: &str, value: &str) -> (String, String) {
        p(&format!("LaunchTemplateData.{member}"), value)
    }

    /// Create one launch template and answer its id. The create carries at least one launch
    /// parameter, which `RequestLaunchTemplateData` requires.
    async fn create_template(
        router: &MultiServiceRouter,
        name: &str,
        extra: &[(String, String)],
    ) -> String {
        let mut params = vec![p("LaunchTemplateName", name)];
        params.extend(extra.iter().cloned());
        let (status, body) = api(router, "CreateLaunchTemplate", &params).await;
        assert_eq!(
            status, 200,
            "creating {name:?}: expected HTTP 200, got {status} with body {body}"
        );
        element(&body, "launchTemplateId").unwrap_or_else(|| {
            panic!("creating {name:?}: the create must report the id it minted: {body}")
        })
    }

    /// The version numbers reported by a describe body, in the order the body carries them.
    fn reported_versions(body: &str) -> Vec<i64> {
        elements(body, "versionNumber")
            .into_iter()
            .map(|number| number.parse::<i64>().unwrap_or_default())
            .collect()
    }

    /// Seed `numbers` as further versions of the stored template `launch_template_id`, each
    /// with the launch parameters `data_for` gives it and a create time `age_for` seconds in the
    /// past, and record `default` as the template's default version.
    ///
    /// Writes the same `LaunchTemplateRecord` `CreateLaunchTemplate` writes, so the operation
    /// under test reads one store and not a test-only copy.
    fn seed_versions(
        engine: &engine::InMemoryEngine,
        launch_template_id: &str,
        versions: &[SeededVersion],
        default: i64,
    ) {
        let state = engine.state::<Ec2State>();
        let mut store = state.lock();
        let record = store
            .launch_templates
            .get_mut(launch_template_id)
            .unwrap_or_else(|| panic!("the template {launch_template_id} must be stored"));
        for (number, age_seconds, data) in versions {
            record.versions.insert(
                *number,
                LaunchTemplateVersionRecord {
                    version_number: *number,
                    version_description: Some(format!("version {number}")),
                    create_time: SystemTime::now() - Duration::from_secs(*age_seconds),
                    created_by: ::engine::emulator_arn("iam", "", "root"),
                    default_version: *number == default,
                    data: data.clone(),
                },
            );
        }
        let latest = record.versions.keys().copied().max().unwrap_or(default);
        record.template.default_version_number = Some(default);
        record.template.latest_version_number = Some(latest);
    }

    /// A created template with versions 1 to 5, version 3 recorded as the default: the shape
    /// every selection row below is written against.
    async fn template_with_five_versions(
        router: &MultiServiceRouter,
        engine: &engine::InMemoryEngine,
        name: &str,
    ) -> String {
        let launch_template_id =
            create_template(router, name, &[data("ImageId", "ami-00000000000000001")]).await;
        let seeded: Vec<SeededVersion> = (2..=5)
            .map(|number| {
                (
                    number,
                    u64::try_from(number).unwrap_or_default() * 60,
                    RequestLaunchTemplateData {
                        image_id: Some(format!("ami-{number:017}")),
                        ..RequestLaunchTemplateData::default()
                    },
                )
            })
            .collect();
        seed_versions(engine, &launch_template_id, &seeded, 3);
        launch_template_id
    }

    /// A create stores one version and a describe reports exactly that version — by the
    /// template's id and by its name alike — with the launch parameters the create carried.
    /// Describing twice reports the same thing, a read changing nothing a later caller sees.
    #[tokio::test]
    async fn a_created_template_round_trips_through_describe_launch_template_versions() {
        let (router, engine) = crate::ec2_query::emulator();
        let created_by = ::engine::emulator_arn("iam", "", "root");

        // Rows differ along the launch parameters the one version holds: how many there are,
        // which members they are, and whether they are scalars, nested members or lists.
        let rows: Vec<RoundTripRow> = vec![
            (
                "one scalar launch parameter",
                "roundtrip.one",
                vec![data("ImageId", "ami-0abcdef1234567890")],
                vec![("imageId", "ami-0abcdef1234567890")],
            ),
            (
                "two scalars",
                "roundtrip.two",
                vec![
                    data("ImageId", "ami-00000000000000009"),
                    data("InstanceType", "t3.micro"),
                ],
                vec![
                    ("imageId", "ami-00000000000000009"),
                    ("instanceType", "t3.micro"),
                ],
            ),
            (
                "a nested member and a boolean",
                "roundtrip.nested",
                vec![
                    data("Monitoring.Enabled", "true"),
                    data("EbsOptimized", "true"),
                    data("KernelId", "aki-00000000000000001"),
                ],
                vec![
                    ("ebsOptimized", "true"),
                    ("kernelId", "aki-00000000000000001"),
                ],
            ),
            (
                "a list member and metadata options",
                "roundtrip.lists",
                vec![
                    data("SecurityGroupId.1", "sg-00000000000000001"),
                    data("SecurityGroupId.2", "sg-00000000000000002"),
                    data("MetadataOptions.HttpTokens", "required"),
                ],
                vec![("httpTokens", "required")],
            ),
        ];

        for (case, name, create_data, expected) in rows {
            let launch_template_id = create_template(&router, name, &create_data).await;

            for (key, value) in [
                ("LaunchTemplateId", launch_template_id.clone()),
                ("LaunchTemplateName", name.to_string()),
            ] {
                let (status, body) = api(
                    &router,
                    "DescribeLaunchTemplateVersions",
                    &[p(key, value.clone())],
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {case:?} by {key}: expected HTTP 200, got {status} with body {body}"
                );
                assert_eq!(
                    reported_versions(&body),
                    vec![1],
                    "case {case:?} by {key}: a created template holds exactly version 1, \
                     got body {body}"
                );
                assert_eq!(
                    element(&body, "launchTemplateId").as_deref(),
                    Some(launch_template_id.as_str()),
                    "case {case:?} by {key}: expected the template's own id, got body {body}"
                );
                assert_eq!(
                    element(&body, "launchTemplateName").as_deref(),
                    Some(name),
                    "case {case:?} by {key}: expected the template's own name, got body {body}"
                );
                assert_eq!(
                    element(&body, "defaultVersion").as_deref(),
                    Some("true"),
                    "case {case:?} by {key}: the only version is the default one, \
                     got body {body}"
                );
                assert_eq!(
                    element(&body, "createdBy").as_deref(),
                    Some(created_by.as_str()),
                    "case {case:?} by {key}: expected the creating principal, got body {body}"
                );
                let create_time = element(&body, "createTime").unwrap_or_default();
                assert!(
                    ::aws_smithy_types::DateTime::from_str(
                        &create_time,
                        ::aws_smithy_types::date_time::Format::DateTime,
                    )
                    .is_ok(),
                    "case {case:?} by {key}: expected an ISO 8601 createTime, \
                     got {create_time:?}"
                );
                for (reported, expected_value) in &expected {
                    assert_eq!(
                        element(&body, reported).as_deref(),
                        Some(*expected_value),
                        "case {case:?} by {key}: expected {reported}={expected_value}, \
                         got body {body}"
                    );
                }
            }

            // A read must not change what a later caller sees: the reported version set of two
            // describes is the same (the `requestId` of the envelope is per call).
            let describe = [p("LaunchTemplateId", launch_template_id.clone())];
            let (_, first) = api(&router, "DescribeLaunchTemplateVersions", &describe).await;
            let (_, again) = api(&router, "DescribeLaunchTemplateVersions", &describe).await;
            assert_eq!(
                element(&first, "launchTemplateVersionSet"),
                element(&again, "launchTemplateVersionSet"),
                "case {case:?}: two describes of one template must report the same versions"
            );
            let stored = engine
                .state::<Ec2State>()
                .lock()
                .launch_templates
                .get(&launch_template_id)
                .map(|record| record.versions.len());
            assert_eq!(
                stored,
                Some(1),
                "case {case:?}: a describe must not add or remove a stored version"
            );
        }
    }

    /// Which versions a request selects: every version of the template by default, the numbers
    /// and aliases `Versions` names, and the range `MinVersion`/`MaxVersion` bounds. The rows
    /// span each of those, their combination, and the values that name no version at all.
    #[tokio::test]
    async fn versions_are_selected_by_number_by_alias_and_by_range() {
        let (router, engine) = crate::ec2_query::emulator();
        let launch_template_id =
            template_with_five_versions(&router, &engine, "selection.five").await;

        // `Versions` reaches the wire under its xmlName, `LaunchTemplateVersion.N`.
        let versions = |values: &[&str]| indexed("LaunchTemplateVersion", values);

        let rows: Vec<SelectionRow> = vec![
            ("no version named: every version", Vec::new(), vec![5, 4, 3, 2, 1]),
            ("the $Latest alias", versions(&["$Latest"]), vec![5]),
            ("the $Default alias", versions(&["$Default"]), vec![3]),
            (
                "both aliases in one request",
                versions(&["$Latest", "$Default"]),
                vec![5, 3],
            ),
            ("one number", versions(&["2"]), vec![2]),
            (
                "numbers out of order are reported newest first",
                versions(&["4", "2"]),
                vec![4, 2],
            ),
            (
                "a number repeated names one version",
                versions(&["2", "2"]),
                vec![2],
            ),
            (
                "a number and an alias",
                versions(&["1", "$Latest"]),
                vec![5, 1],
            ),
            // "The version number AFTER WHICH to describe launch template versions"
            // (`MinVersion`) against "the version number UP TO WHICH to describe launch
            // template versions" (`MaxVersion`): the lower edge is exclusive and the upper
            // edge inclusive, so a bound pair names the half-open range after Min through Max.
            ("MinVersion alone", vec![p("MinVersion", "3")], vec![5, 4]),
            ("MaxVersion alone", vec![p("MaxVersion", "2")], vec![2, 1]),
            (
                "both bounds",
                vec![p("MinVersion", "2"), p("MaxVersion", "4")],
                vec![4, 3],
            ),
            (
                "a bound past the versions held",
                vec![p("MinVersion", "6")],
                Vec::new(),
            ),
            (
                "the first version is reported only when MinVersion is below it",
                vec![p("MinVersion", "0"), p("MaxVersion", "5")],
                vec![5, 4, 3, 2, 1],
            ),
            (
                "MinVersion names the last version left out",
                vec![p("MinVersion", "1"), p("MaxVersion", "5")],
                vec![5, 4, 3, 2],
            ),
            (
                "MaxVersion names the last version reported",
                vec![p("MaxVersion", "5")],
                vec![5, 4, 3, 2, 1],
            ),
            (
                "named versions outside the bound are left out",
                {
                    let mut params = versions(&["1", "5"]);
                    params.push(p("MinVersion", "2"));
                    params
                },
                vec![5],
            ),
        ];

        for (case, selection, expected) in rows {
            let mut params = vec![p("LaunchTemplateId", launch_template_id.clone())];
            params.extend(selection);
            let (status, body) = api(&router, "DescribeLaunchTemplateVersions", &params).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with body {body}"
            );
            let reported = reported_versions(&body);
            assert_eq!(
                reported, expected,
                "case {case:?}: expected versions {expected:?}, got {reported:?}"
            );
        }

        // A version value that names nothing: a number the template does not hold is a
        // version-not-found, and a word that is neither alias nor number is a bad value.
        let refusals: Vec<RefusalRow> = vec![
            (
                "a number no version carries",
                versions(&["9"]),
                "InvalidLaunchTemplateId.VersionNotFound",
            ),
            (
                "zero, which no version is numbered",
                versions(&["0"]),
                "InvalidLaunchTemplateId.VersionNotFound",
            ),
            (
                "a word that is not an alias",
                versions(&["$Newest"]),
                "InvalidParameterValue",
            ),
            (
                "a lower-case alias, aliases being case-sensitive",
                versions(&["$latest"]),
                "InvalidParameterValue",
            ),
            (
                "a MinVersion that is not a number",
                vec![p("MinVersion", "oldest")],
                "InvalidParameterValue",
            ),
            (
                "a MaxVersion that is not a number",
                vec![p("MaxVersion", "$Latest")],
                "InvalidParameterValue",
            ),
        ];

        for (case, selection, expected) in refusals {
            let mut params = vec![p("LaunchTemplateId", launch_template_id.clone())];
            params.extend(selection);
            let (status, body) = api(&router, "DescribeLaunchTemplateVersions", &params).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} with body {body}"
            );
            assert_eq!(
                error_code(&body),
                expected,
                "case {case:?}: expected {expected}, got body {body}"
            );
        }
    }

    /// A dry run reports the outcome of the permission check and describes nothing: "If you
    /// have the required permissions, the error response is DryRunOperation"
    /// (`DescribeLaunchTemplateVersionsRequest$DryRun`). Every caller of this emulator is
    /// permitted, so every row — whichever selection it carries — is answered the same way.
    #[tokio::test]
    async fn a_dry_run_reports_the_permission_check_and_describes_nothing() {
        let (router, engine) = crate::ec2_query::emulator();
        let launch_template_id = template_with_five_versions(&router, &engine, "dry.run.five").await;
        // A template this service MINTED and then took away: an id it really issued, so the
        // dry run is answered before the lookup that would have found nothing — where an id
        // no mint of it ever reached is screened out ahead of the flag, that value naming
        // nothing for a permission check to be about.
        let gone = create_template(
            &router,
            "dry.run.gone",
            &[data("ImageId", "ami-00000000000000001")],
        )
        .await;
        let (status, body) = api(
            &router,
            "DeleteLaunchTemplate",
            &[p("LaunchTemplateId", gone.clone())],
        )
        .await;
        assert_eq!(
            status, 200,
            "the template the dry run names must have been taken away, got {status} with \
             body {body}"
        );

        // Rows differ along WHAT the dry run would have described: one template, one version
        // of it, an account-wide alias, and a template that does not exist at all.
        let rows: Vec<(&str, Params)> = vec![
            (
                "a template by id",
                vec![p("LaunchTemplateId", launch_template_id.clone())],
            ),
            (
                "one version of a template",
                {
                    let mut params = vec![p("LaunchTemplateId", launch_template_id.clone())];
                    params.extend(indexed("LaunchTemplateVersion", &["$Latest"]));
                    params
                },
            ),
            (
                "an account-wide describe",
                indexed("LaunchTemplateVersion", &["$Default"]),
            ),
            (
                "a template the account no longer carries",
                vec![p("LaunchTemplateId", gone.clone())],
            ),
        ];

        for (case, selection) in rows {
            let mut params = vec![p("DryRun", "true")];
            params.extend(selection);
            let (status, body) = api(&router, "DescribeLaunchTemplateVersions", &params).await;
            assert_eq!(
                status, 412,
                "case {case:?}: expected HTTP 412, got {status} with body {body}"
            );
            assert_eq!(
                error_code(&body),
                "DryRunOperation",
                "case {case:?}: expected DryRunOperation, got body {body}"
            );
            assert!(
                reported_versions(&body).is_empty(),
                "case {case:?}: a dry run must describe nothing, got body {body}"
            );
        }

        // A dry run of a request that is NOT well formed is answered with that request's own
        // refusal: the permission check it asks about is never reached, so the malformed input
        // is what the caller is told about. Rows differ along which part of the request is
        // malformed.
        let malformed: Vec<RefusalRow> = vec![
            (
                "a dry run naming both an id and a name",
                vec![
                    p("LaunchTemplateId", launch_template_id.clone()),
                    p("LaunchTemplateName", "dry.run.five"),
                ],
                "InvalidParameterCombination",
            ),
            (
                "a dry run with a MinVersion that is not a number",
                vec![
                    p("LaunchTemplateId", launch_template_id.clone()),
                    p("MinVersion", "oldest"),
                ],
                "InvalidParameterValue",
            ),
            (
                "a dry run with an undocumented filter",
                {
                    let mut params = vec![p("LaunchTemplateId", launch_template_id.clone())];
                    params.extend(filters(&[("no-such-filter", &["x"])]));
                    params
                },
                "InvalidParameterValue",
            ),
            (
                "a dry run naming no template and no version",
                Vec::new(),
                "MissingParameter",
            ),
            (
                "a dry run with a malformed id",
                vec![p("LaunchTemplateId", "not-an-id")],
                "InvalidLaunchTemplateId.Malformed",
            ),
        ];

        for (case, selection, expected) in malformed {
            let mut params = vec![p("DryRun", "true")];
            params.extend(selection);
            let (status, body) = api(&router, "DescribeLaunchTemplateVersions", &params).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} with body {body}"
            );
            assert_eq!(
                error_code(&body),
                expected,
                "case {case:?}: expected {expected}, got body {body}"
            );
        }

        // A dry run changed nothing: the versions are still all there afterwards.
        let (status, body) = api(
            &router,
            "DescribeLaunchTemplateVersions",
            &[p("LaunchTemplateId", launch_template_id)],
        )
        .await;
        assert_eq!(
            status, 200,
            "after the dry runs: expected HTTP 200, got {status} with body {body}"
        );
        let reported = reported_versions(&body);
        assert_eq!(
            reported,
            vec![5, 4, 3, 2, 1],
            "after the dry runs: expected the five versions, got {reported:?}"
        );
    }

    /// A template an Amazon Web Services service manages and hides is reported only when the
    /// request asks for managed resources: "If this parameter is set to true, the output
    /// includes resources that are managed by Amazon Web Services services, even if managed
    /// resource visibility is set to hidden"
    /// (`DescribeLaunchTemplateVersionsRequest$IncludeManagedResources`), hidden being
    /// `OperatorResponse$HiddenByDefault`, "whether the resource is hidden from public view".
    #[tokio::test]
    async fn a_managed_template_is_reported_only_when_managed_resources_are_asked_for() {
        let (router, engine) = crate::ec2_query::emulator();
        let visible =
            create_template(&router, "operator.visible", &[data("ImageId", "ami-00000000000000001")])
                .await;
        let hidden =
            create_template(&router, "operator.hidden", &[data("ImageId", "ami-00000000000000002")])
                .await;

        // Rows of the operator table: what the template's operator says about it, and whether
        // a request that does NOT ask for managed resources sees it.
        let operators: Vec<(&str, &str, Option<OperatorResponse>, bool)> = vec![
            ("no operator at all", visible.as_str(), None, true),
            (
                "managed but not hidden",
                visible.as_str(),
                Some(OperatorResponse {
                    managed: Some(true),
                    principal: Some(::engine::emulator_arn("iam", "", "role/aws-service-role")),
                    hidden_by_default: Some(false),
                }),
                true,
            ),
            (
                "managed and hidden",
                hidden.as_str(),
                Some(OperatorResponse {
                    managed: Some(true),
                    principal: Some(::engine::emulator_arn("iam", "", "role/aws-service-role")),
                    hidden_by_default: Some(true),
                }),
                false,
            ),
        ];

        for (case, launch_template_id, operator, visible_by_default) in operators {
            {
                let state = engine.state::<Ec2State>();
                let mut store = state.lock();
                let record = store
                    .launch_templates
                    .get_mut(launch_template_id)
                    .unwrap_or_else(|| panic!("case {case:?}: the template must be stored"));
                record.template.operator = operator.clone();
            }

            for asked in [None, Some("false"), Some("true")] {
                let mut params = vec![p("LaunchTemplateId", launch_template_id.to_string())];
                if let Some(value) = asked {
                    params.push(p("IncludeManagedResources", value));
                }
                let asks_for_managed = asked == Some("true");
                let (status, body) = api(&router, "DescribeLaunchTemplateVersions", &params).await;
                assert_eq!(
                    status, 200,
                    "case {case:?} with IncludeManagedResources={asked:?}: expected HTTP 200, \
                     got {status} with body {body}"
                );
                let reported = reported_versions(&body);
                let expected: Vec<i64> = if visible_by_default || asks_for_managed {
                    vec![1]
                } else {
                    Vec::new()
                };
                assert_eq!(
                    reported, expected,
                    "case {case:?} with IncludeManagedResources={asked:?}: expected versions \
                     {expected:?}, got {reported:?} from body {body}"
                );
            }
        }
    }

    /// The template a request names has to resolve: a malformed id, an id no template carries
    /// and a name no template carries are each refused with their own code, and naming both an
    /// id and a name is refused as a parameter combination.
    #[tokio::test]
    async fn a_template_a_request_names_has_to_resolve() {
        let (router, _engine) = crate::ec2_query::emulator();
        let launch_template_id =
            create_template(&router, "resolve.me", &[data("ImageId", "ami-00000000000000001")]).await;

        let rows: Vec<RefusalRow> = vec![
            (
                "an id that is not a launch template id",
                vec![p("LaunchTemplateId", "not-an-id")],
                "InvalidLaunchTemplateId.Malformed",
            ),
            (
                "an id of the wrong length",
                vec![p("LaunchTemplateId", "lt-0000")],
                "InvalidLaunchTemplateId.Malformed",
            ),
            (
                "a well-formed id no mint of this service ever reached",
                vec![p("LaunchTemplateId", "lt-00000000000000099")],
                "InvalidLaunchTemplateId.Malformed",
            ),
            (
                "a name no template carries",
                vec![p("LaunchTemplateName", "no.such.template")],
                "InvalidLaunchTemplateName.NotFoundException",
            ),
            (
                "both the id and the name",
                vec![
                    p("LaunchTemplateId", launch_template_id.clone()),
                    p("LaunchTemplateName", "resolve.me"),
                ],
                "InvalidParameterCombination",
            ),
        ];

        for (case, params, expected) in rows {
            let (status, body) = api(&router, "DescribeLaunchTemplateVersions", &params).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} with body {body}"
            );
            assert_eq!(
                error_code(&body),
                expected,
                "case {case:?}: expected {expected}, got body {body}"
            );
        }
    }

    /// A request that names no template describes the latest or the default version of EVERY
    /// template of the account, and that is the only thing `Versions` may ask for then.
    #[tokio::test]
    async fn an_account_wide_describe_reports_one_version_of_every_template() {
        let (router, engine) = crate::ec2_query::emulator();

        // Three templates, each with a different number of versions and a different default,
        // so "the latest" and "the default" are distinct answers per template.
        let first = create_template(&router, "account.one", &[data("ImageId", "ami-1")]).await;
        let second = create_template(&router, "account.two", &[data("ImageId", "ami-2")]).await;
        let third = create_template(&router, "account.three", &[data("ImageId", "ami-3")]).await;
        seed_versions(
            &engine,
            &second,
            &[(2, 60, RequestLaunchTemplateData::default())],
            1,
        );
        seed_versions(
            &engine,
            &third,
            &[
                (2, 120, RequestLaunchTemplateData::default()),
                (3, 60, RequestLaunchTemplateData::default()),
            ],
            2,
        );

        // Paged in (template id, version number newest first) order, and the ids are minted
        // in creation order, so the expected rows are written in that order.
        let rows: Vec<AccountRow> = vec![
            (
                "the latest version of every template",
                vec!["$Latest"],
                vec![(first.clone(), 1), (second.clone(), 2), (third.clone(), 3)],
            ),
            (
                "the default version of every template",
                vec!["$Default"],
                vec![(first.clone(), 1), (second.clone(), 1), (third.clone(), 2)],
            ),
            (
                "both aliases together",
                vec!["$Latest", "$Default"],
                vec![
                    (first.clone(), 1),
                    (second.clone(), 2),
                    (second.clone(), 1),
                    (third.clone(), 3),
                    (third.clone(), 2),
                ],
            ),
        ];

        for (case, aliases, expected) in rows {
            let params = indexed("LaunchTemplateVersion", &aliases);
            let (status, body) = api(&router, "DescribeLaunchTemplateVersions", &params).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with body {body}"
            );
            let reported: Vec<(String, i64)> = elements(&body, "launchTemplateId")
                .into_iter()
                .zip(reported_versions(&body))
                .collect();
            assert_eq!(
                reported, expected,
                "case {case:?}: expected {expected:?}, got {reported:?}"
            );
        }

        // Across the account a version is named by alias only, and naming no version at all
        // has named nothing to describe.
        let refusals: Vec<RefusalRow> = vec![
            ("no template and no version", Vec::new(), "MissingParameter"),
            (
                "a number, which names no version across the account",
                indexed("LaunchTemplateVersion", &["1"]),
                "InvalidParameterCombination",
            ),
            (
                "a number beside an alias",
                indexed("LaunchTemplateVersion", &["$Latest", "2"]),
                "InvalidParameterCombination",
            ),
        ];
        for (case, params, expected) in refusals {
            let (status, body) = api(&router, "DescribeLaunchTemplateVersions", &params).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} with body {body}"
            );
            assert_eq!(
                error_code(&body),
                expected,
                "case {case:?}: expected {expected}, got body {body}"
            );
        }

        // A template that cannot answer the alias contributes nothing to an account-wide
        // describe, which named the account's templates and not that one: the templates that
        // can answer it are still reported. Naming that template itself is refused, the caller
        // having asked about it in particular.
        {
            let state = engine.state::<Ec2State>();
            let mut store = state.lock();
            let record = store
                .launch_templates
                .get_mut(&second)
                .expect("the template must be stored");
            record.template.default_version_number = None;
        }
        let (status, body) = api(
            &router,
            "DescribeLaunchTemplateVersions",
            &indexed("LaunchTemplateVersion", &["$Default"]),
        )
        .await;
        assert_eq!(
            status, 200,
            "a template with no default version must not fail the account-wide describe, \
             got {status} with body {body}"
        );
        let reported: Vec<(String, i64)> = elements(&body, "launchTemplateId")
            .into_iter()
            .zip(reported_versions(&body))
            .collect();
        let expected = vec![(first.clone(), 1), (third.clone(), 2)];
        assert_eq!(
            reported, expected,
            "a template with no default version contributes nothing: expected {expected:?}, \
             got {reported:?}"
        );
        let (status, body) = api(
            &router,
            "DescribeLaunchTemplateVersions",
            &{
                let mut params = vec![p("LaunchTemplateId", second.clone())];
                params.extend(indexed("LaunchTemplateVersion", &["$Default"]));
                params
            },
        )
        .await;
        assert_eq!(
            status, 400,
            "naming the template whose default version is unset must be refused, got {status} \
             with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidLaunchTemplateId.VersionNotFound",
            "naming the template whose default version is unset must be refused with \
             InvalidLaunchTemplateId.VersionNotFound, got body {body}"
        );
    }

    /// The launch parameters the version-attribute filters select on, carried by version 1 of
    /// the filter suite's template.
    fn filterable_data() -> RequestLaunchTemplateData {
        RequestLaunchTemplateData {
            image_id: Some("ami-00000000000000001".to_string()),
            instance_type: Some("t3.micro".to_string()),
            kernel_id: Some("aki-00000000000000001".to_string()),
            ram_disk_id: Some("ari-00000000000000001".to_string()),
            ebs_optimized: Some(true),
            iam_instance_profile: Some(LaunchTemplateIamInstanceProfileSpecificationRequest {
                arn: Some(
                    "arn:aws:iam::123456789012:instance-profile/web-tier".to_string(),
                ),
                name: None,
            }),
            metadata_options: Some(LaunchTemplateInstanceMetadataOptionsRequest {
                http_tokens: Some("required".to_string()),
                http_endpoint: Some("enabled".to_string()),
                http_put_response_hop_limit: Some(2),
                http_protocol_ipv6: Some("disabled".to_string()),
                instance_metadata_tags: Some("enabled".to_string()),
            }),
            placement: Some(LaunchTemplatePlacementRequest {
                host_resource_group_arn: Some(
                    "arn:aws:resource-groups:us-east-1:123456789012:group/hosts".to_string(),
                ),
                ..LaunchTemplatePlacementRequest::default()
            }),
            license_specifications: Some(vec![LaunchTemplateLicenseConfigurationRequest {
                license_configuration_arn: Some(
                    "arn:aws:license-manager:us-east-1:123456789012:license-configuration:lic-1"
                        .to_string(),
                ),
            }]),
            network_interfaces: Some(vec![
                LaunchTemplateInstanceNetworkInterfaceSpecificationRequest {
                    device_index: Some(0),
                    network_card_index: Some(1),
                    ..LaunchTemplateInstanceNetworkInterfaceSpecificationRequest::default()
                },
            ]),
            ..RequestLaunchTemplateData::default()
        }
    }

    /// Every filter `DescribeLaunchTemplateVersionsRequest$Filters` documents selects on the
    /// attribute of the VERSION it names, with EC2's `*`/`?` wildcards, the values of one filter
    /// joined with OR and several filters joined with AND; a filter name the operation does not
    /// document is refused.
    ///
    /// The catalog of names comes from that member's own documentation
    /// (`smithy_query(query="doc",shape="DescribeLaunchTemplateVersionsRequest",member="Filters")`)
    /// and every one of them has a row here.
    #[tokio::test]
    async fn each_documented_filter_selects_on_the_attribute_it_names() {
        let (router, engine) = crate::ec2_query::emulator();

        // Version 1 carries every filterable attribute and is the default version; version 2
        // carries different values and is not, so a filter that selects on either is visible.
        let launch_template_id =
            create_template(&router, "filters.one", &[data("ImageId", "ami-00000000000000001")])
                .await;
        seed_versions(
            &engine,
            &launch_template_id,
            &[
                (1, 0, filterable_data()),
                (
                    2,
                    7 * 24 * 3600,
                    RequestLaunchTemplateData {
                        image_id: Some("ami-00000000000000002".to_string()),
                        instance_type: Some("t3.small".to_string()),
                        ebs_optimized: Some(false),
                        ..RequestLaunchTemplateData::default()
                    },
                ),
            ],
            1,
        );

        // The create time of version 1 as the wire reports it: the text a `create-time` filter
        // value is compared against.
        let (_, all) = api(
            &router,
            "DescribeLaunchTemplateVersions",
            &[
                p("LaunchTemplateId", launch_template_id.clone()),
                p("MaxVersion", "1"),
            ],
        )
        .await;
        let create_time = element(&all, "createTime").unwrap_or_default();

        let profile_arn = "arn:aws:iam::123456789012:instance-profile/web-tier";
        let host_group_arn = "arn:aws:resource-groups:us-east-1:123456789012:group/hosts";
        let license_arn =
            "arn:aws:license-manager:us-east-1:123456789012:license-configuration:lic-1";

        let rows: Vec<FilterRow<'_>> = vec![
            ("create-time", vec![create_time.as_str()], vec![1]),
            ("create-time", vec!["1999-*"], Vec::new()),
            ("ebs-optimized", vec!["true"], vec![1]),
            ("ebs-optimized", vec!["false"], vec![2]),
            ("host-resource-group-arn", vec![host_group_arn], vec![1]),
            ("host-resource-group-arn", vec!["arn:aws:*:group/other"], Vec::new()),
            ("http-endpoint", vec!["enabled"], vec![1]),
            ("http-endpoint", vec!["disabled"], Vec::new()),
            // Documented, but the modelled metadata options carry no IPv4 member, so no
            // version says anything about it and the filter selects nothing.
            ("http-protocol-ipv4", vec!["enabled"], Vec::new()),
            ("http-protocol-ipv4", vec!["disabled"], Vec::new()),
            ("http-tokens", vec!["required"], vec![1]),
            ("http-tokens", vec!["optional"], Vec::new()),
            ("iam-instance-profile", vec![profile_arn], vec![1]),
            ("iam-instance-profile", vec!["*web-tier"], vec![1]),
            ("image-id", vec!["ami-00000000000000001"], vec![1]),
            ("image-id", vec!["ami-*"], vec![2, 1]),
            ("image-id", vec!["ami-0000000000000000?"], vec![2, 1]),
            ("image-id", vec!["ami-00000000000000003"], Vec::new()),
            ("instance-type", vec!["t3.micro"], vec![1]),
            // Several values of one filter are joined with OR.
            ("instance-type", vec!["t3.micro", "t3.small"], vec![2, 1]),
            ("is-default-version", vec!["true"], vec![1]),
            ("is-default-version", vec!["false"], vec![2]),
            ("kernel-id", vec!["aki-00000000000000001"], vec![1]),
            ("kernel-id", vec!["aki-*"], vec![1]),
            ("license-configuration-arn", vec![license_arn], vec![1]),
            ("license-configuration-arn", vec!["arn:aws:*:lic-2"], Vec::new()),
            ("network-card-index", vec!["1"], vec![1]),
            ("network-card-index", vec!["0"], Vec::new()),
            ("ram-disk-id", vec!["ari-00000000000000001"], vec![1]),
            ("ram-disk-id", vec!["ari-00000000000000002"], Vec::new()),
        ];

        for (name, values, expected) in rows {
            let case = format!("{name} = {values:?}");
            let mut params = vec![p("LaunchTemplateId", launch_template_id.clone())];
            params.extend(filters(&[(name, values.as_slice())]));
            let (status, body) = api(&router, "DescribeLaunchTemplateVersions", &params).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with body {body}"
            );
            let reported = reported_versions(&body);
            assert_eq!(
                reported, expected,
                "case {case:?}: expected versions {expected:?}, got {reported:?}"
            );
        }

        // Several filters are joined with AND.
        let conjunctions: Vec<ConjunctionRow> = vec![
            (
                "both filters hold of version 1",
                vec![
                    ("image-id", &["ami-00000000000000001"][..]),
                    ("ebs-optimized", &["true"][..]),
                ],
                vec![1],
            ),
            (
                "the second filter holds of no version the first selects",
                vec![
                    ("image-id", &["ami-00000000000000001"][..]),
                    ("ebs-optimized", &["false"][..]),
                ],
                Vec::new(),
            ),
            (
                "two filters that hold of different versions",
                vec![
                    ("image-id", &["ami-*"][..]),
                    ("instance-type", &["t3.small"][..]),
                ],
                vec![2],
            ),
        ];
        for (case, entries, expected) in conjunctions {
            let mut params = vec![p("LaunchTemplateId", launch_template_id.clone())];
            params.extend(filters(&entries));
            let (status, body) = api(&router, "DescribeLaunchTemplateVersions", &params).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with body {body}"
            );
            let reported = reported_versions(&body);
            assert_eq!(
                reported, expected,
                "case {case:?}: expected versions {expected:?}, got {reported:?}"
            );
        }

        // A filter this operation does not document is refused rather than read as "no
        // results".
        for name in ["tag-key", "tag:Name", "version-number", "create_time", ""] {
            let mut params = vec![p("LaunchTemplateId", launch_template_id.clone())];
            params.extend(filters(&[(name, &["anything"])]));
            let (status, body) = api(&router, "DescribeLaunchTemplateVersions", &params).await;
            assert_eq!(
                status, 400,
                "filter {name:?}: expected HTTP 400, got {status} with body {body}"
            );
            assert_eq!(
                error_code(&body),
                "InvalidParameterValue",
                "filter {name:?}: expected InvalidParameterValue, got body {body}"
            );
        }
    }

    /// A page carries at most `MaxResults` versions and a token naming the version the next page
    /// starts at, so walking the pages reports every version exactly once, in one order,
    /// whatever page size is asked for. The published bounds of `MaxResults` are enforced, and a
    /// token this service did not mint is refused.
    #[tokio::test]
    async fn a_page_is_cut_at_max_results_and_resumed_with_the_token() {
        let (router, engine) = crate::ec2_query::emulator();
        let launch_template_id =
            template_with_five_versions(&router, &engine, "paging.five").await;
        let all: Vec<i64> = vec![5, 4, 3, 2, 1];

        for page_size in 1..=6 {
            let case = format!("MaxResults={page_size}");
            let mut walked: Vec<i64> = Vec::new();
            let mut token: Option<String> = None;
            let mut pages = 0;
            loop {
                let mut params = vec![
                    p("LaunchTemplateId", launch_template_id.clone()),
                    p("MaxResults", page_size.to_string()),
                ];
                if let Some(cursor) = &token {
                    params.push(p("NextToken", cursor.clone()));
                }
                let (status, body) = api(&router, "DescribeLaunchTemplateVersions", &params).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected HTTP 200, got {status} with body {body}"
                );
                let reported = reported_versions(&body);
                assert!(
                    reported.len() <= page_size,
                    "case {case:?}: a page may carry at most {page_size} versions, \
                     got {reported:?}"
                );
                walked.extend(reported);
                token = element(&body, "nextToken");
                pages += 1;
                assert!(
                    pages <= all.len() + 1,
                    "case {case:?}: the walk must terminate, got {walked:?}"
                );
                if token.is_none() {
                    break;
                }
            }
            assert_eq!(
                walked, all,
                "case {case:?}: walking the pages must report every version once, in order"
            );
        }

        // "This value can be between 1 and 200": each published bound is accepted and one past
        // it is refused.
        for (case, max_results, expected_status) in [
            ("the published minimum", "1", 200),
            ("one below the minimum", "0", 400),
            ("the published maximum", "200", 200),
            ("one above the maximum", "201", 400),
        ] {
            let params = vec![
                p("LaunchTemplateId", launch_template_id.clone()),
                p("MaxResults", max_results),
            ];
            let (status, body) = api(&router, "DescribeLaunchTemplateVersions", &params).await;
            assert_eq!(
                status, expected_status,
                "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
            );
            if expected_status == 400 {
                assert_eq!(
                    error_code(&body),
                    "InvalidParameterValue",
                    "case {case:?}: expected InvalidParameterValue, got body {body}"
                );
            }
        }

        // A token this service did not mint names no position.
        for token in ["", "not-a-token", "bHQtMDAwIzE="] {
            let params = vec![
                p("LaunchTemplateId", launch_template_id.clone()),
                p("NextToken", token),
            ];
            let (status, body) = api(&router, "DescribeLaunchTemplateVersions", &params).await;
            assert_eq!(
                status, 400,
                "token {token:?}: expected HTTP 400, got {status} with body {body}"
            );
            assert_eq!(
                error_code(&body),
                "InvalidPaginationToken",
                "token {token:?}: expected InvalidPaginationToken, got body {body}"
            );
        }
    }

    /// An `ImageId` that names a Systems Manager parameter is reported as the AMI the parameter
    /// resolves to exactly when `ResolveAlias` asks for it, and as the parameter itself
    /// otherwise.
    ///
    /// Every public parameter the image store publishes is a row — the catalog comes from
    /// `Image$PublicSsmParameterName`, read out of the store rather than written here — and a
    /// parameter no image publishes resolves to nothing.
    #[tokio::test]
    async fn an_ssm_parameter_image_id_resolves_only_when_resolve_alias_asks() {
        let (router, engine) = crate::ec2_query::emulator();

        let published: Vec<(String, String)> = engine
            .state::<Ec2State>()
            .lock()
            .images
            .values()
            .filter_map(|image| {
                Some((
                    image.public_ssm_parameter_name.clone()?,
                    image.image_id.clone()?,
                ))
            })
            .collect();
        assert!(
            published.len() > 1,
            "the image catalog must publish more than one Systems Manager parameter to test \
             against, got {published:?}"
        );

        for (index, (parameter, image_id)) in published.iter().enumerate() {
            let case = format!("{parameter} -> {image_id}");
            let alias = format!("{SSM_IMAGE_ALIAS_PREFIX}{parameter}");
            let name = format!("alias.template.{index}");
            let launch_template_id = create_template(&router, &name, &[data("ImageId", &alias)]).await;

            for (asked, expected) in [
                (None, alias.as_str()),
                (Some("false"), alias.as_str()),
                (Some("true"), image_id.as_str()),
            ] {
                let mut params = vec![p("LaunchTemplateId", launch_template_id.clone())];
                if let Some(value) = asked {
                    params.push(p("ResolveAlias", value));
                }
                let (status, body) = api(&router, "DescribeLaunchTemplateVersions", &params).await;
                assert_eq!(
                    status, 200,
                    "case {case:?} with ResolveAlias={asked:?}: expected HTTP 200, \
                     got {status} with body {body}"
                );
                assert_eq!(
                    element(&body, "imageId").as_deref(),
                    Some(expected),
                    "case {case:?} with ResolveAlias={asked:?}: expected imageId {expected}, \
                     got body {body}"
                );
            }

            // The store is what the alias resolves against, so the AMI reported is one
            // `DescribeImages` reports too.
            let (status, images) = api(
                &router,
                "DescribeImages",
                &indexed("ImageId", &[image_id.as_str()]),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: the resolved AMI must be describable, got {status} with {images}"
            );
            assert_eq!(
                element(&images, "imageId").as_deref(),
                Some(image_id.as_str()),
                "case {case:?}: expected DescribeImages to report {image_id}, got {images}"
            );
        }

        // A parameter no image publishes resolves to nothing. Such an alias is refused at
        // CREATE — "the Systems Manager parameter will resolve to an AMI ID on launch", so a
        // template that could never launch is not stored at all — while a version ALREADY
        // stored under one (the parameter having gone since) reports the parameter the caller
        // stored, whether or not the request asks for the alias to be resolved.
        let unresolvable = format!("{SSM_IMAGE_ALIAS_PREFIX}/aws/service/no/such/parameter");
        let (status, body) = api(
            &router,
            "CreateLaunchTemplate",
            &[
                p("LaunchTemplateName", "alias.unresolvable"),
                data("ImageId", &unresolvable),
            ],
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "SsmInvalidParameter"),
            "a create whose alias resolves to no AMI must be refused, got {status} with body \
             {body}"
        );

        let (resolvable_parameter, resolvable_image) = published
            .first()
            .cloned()
            .expect("the image catalog publishes at least one parameter");
        let alias = format!("{SSM_IMAGE_ALIAS_PREFIX}{resolvable_parameter}");
        let resolvable_template =
            create_template(&router, "alias.beside.unresolvable", &[data("ImageId", &alias)]).await;
        // Written into the store the way a version whose parameter has since gone reads, which
        // is the only way one can be there now that the create resolves the alias.
        seed_versions(
            &engine,
            &resolvable_template,
            &[(
                2,
                0,
                RequestLaunchTemplateData {
                    image_id: Some(unresolvable.clone()),
                    ..RequestLaunchTemplateData::default()
                },
            )],
            1,
        );
        for asked in [None, Some("false"), Some("true")] {
            let mut params = vec![
                p("LaunchTemplateId", resolvable_template.clone()),
                ("LaunchTemplateVersion.1".to_string(), "2".to_string()),
            ];
            if let Some(value) = asked {
                params.push(p("ResolveAlias", value));
            }
            let (status, body) = api(&router, "DescribeLaunchTemplateVersions", &params).await;
            assert_eq!(
                status, 200,
                "an unresolvable parameter with ResolveAlias={asked:?}: expected HTTP 200, \
                 got {status} with body {body}"
            );
            assert_eq!(
                element(&body, "imageId").as_deref(),
                Some(unresolvable.as_str()),
                "an unresolvable parameter with ResolveAlias={asked:?}: expected the stored \
                 parameter {unresolvable}, got body {body}"
            );
        }

        // One unresolvable alias does not hide the versions that do resolve: both versions are
        // described in one call, each reporting its own image id.
        let (status, body) = api(
            &router,
            "DescribeLaunchTemplateVersions",
            &[
                p("ResolveAlias", "true"),
                p("LaunchTemplateId", resolvable_template.clone()),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "a describe must survive an unresolvable alias, got {status} with body {body}"
        );
        let reported = elements(&body, "imageId");
        assert!(
            reported.contains(&resolvable_image) && reported.contains(&unresolvable),
            "expected the resolved AMI {resolvable_image} for {resolvable_template} beside the \
             unresolved parameter {unresolvable}, got {reported:?}"
        );
    }

    /// Every factor of the filtering cost the REQUEST chooses is bounded: the number of
    /// filters, the number of values one filter carries, the length of a value and the number
    /// of versions named. Each published bound is accepted and one past it is refused, so a
    /// caller cannot ask for unbounded work under the state lock.
    #[tokio::test]
    async fn the_request_chosen_factors_of_the_filtering_cost_are_bounded() {
        let (router, _engine) = crate::ec2_query::emulator();
        let launch_template_id = create_template(
            &router,
            "bounded.filters",
            &[data("ImageId", "ami-00000000000000001")],
        )
        .await;

        // The same documented filter repeated: what varies is HOW MANY the request carries.
        let repeated_filters = |count: usize| -> Params {
            (1..=count)
                .flat_map(|index| {
                    vec![
                        p(&format!("Filter.{index}.Name"), "image-id"),
                        p(&format!("Filter.{index}.Value.1"), "ami-*"),
                    ]
                })
                .collect()
        };
        // One documented filter carrying `count` values.
        let repeated_values = |count: usize| -> Params {
            let mut fields = vec![p("Filter.1.Name", "image-id")];
            fields.extend(
                (1..=count).map(|index| p(&format!("Filter.1.Value.{index}"), "ami-*")),
            );
            fields
        };
        // One filter value of `bytes` bytes.
        let value_of_bytes = |bytes: usize| -> Params {
            vec![
                p("Filter.1.Name", "image-id"),
                p("Filter.1.Value.1", "a".repeat(bytes)),
            ]
        };
        // `count` versions named, all naming version 1.
        let repeated_versions = |count: usize| -> Params {
            (1..=count)
                .map(|index| p(&format!("LaunchTemplateVersion.{index}"), "1"))
                .collect()
        };

        let rows: Vec<(String, Params, u16)> = vec![
            (
                format!("{MAX_FILTERS} filters, the published maximum"),
                repeated_filters(MAX_FILTERS),
                200,
            ),
            (
                format!("{} filters, one past it", MAX_FILTERS + 1),
                repeated_filters(MAX_FILTERS + 1),
                400,
            ),
            (
                format!("{MAX_FILTER_VALUES} values on one filter"),
                repeated_values(MAX_FILTER_VALUES),
                200,
            ),
            (
                format!("{} values, one past it", MAX_FILTER_VALUES + 1),
                repeated_values(MAX_FILTER_VALUES + 1),
                400,
            ),
            (
                format!("a value of {MAX_FILTER_VALUE_BYTES} bytes"),
                value_of_bytes(MAX_FILTER_VALUE_BYTES),
                200,
            ),
            (
                format!("a value of {} bytes, one past it", MAX_FILTER_VALUE_BYTES + 1),
                value_of_bytes(MAX_FILTER_VALUE_BYTES + 1),
                400,
            ),
            (
                format!("{MAX_RESOURCE_IDS} versions named"),
                repeated_versions(MAX_RESOURCE_IDS),
                200,
            ),
            (
                format!("{} versions, one past it", MAX_RESOURCE_IDS + 1),
                repeated_versions(MAX_RESOURCE_IDS + 1),
                400,
            ),
        ];

        for (case, selection, expected_status) in rows {
            let mut params = vec![p("LaunchTemplateId", launch_template_id.clone())];
            params.extend(selection);
            let (status, body) = api(&router, "DescribeLaunchTemplateVersions", &params).await;
            assert_eq!(
                status, expected_status,
                "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
            );
            if expected_status == 400 {
                assert_eq!(
                    error_code(&body),
                    "InvalidParameterValue",
                    "case {case:?}: expected InvalidParameterValue, got body {body}"
                );
            }
        }
    }
}
