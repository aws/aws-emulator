// SPDX-License-Identifier: CC0-1.0

//! The Redshift IAM Identity Center application lifecycle, end to end over the
//! awsQuery wire.
//!
//! Every case drives a real HTTP request through the same `MultiServiceRouter`
//! the production router builds (the harness is shared with
//! [`crate::cluster_lifecycle`]), so each assertion covers the whole walk:
//! form-urlencoded deserialize -> handler -> XML serialize.
//!
//! The suite asserts RULES over tables of inputs rather than one recorded
//! request: a created application is stored exactly as the request configured
//! it — every optional member included, and a member the request left out left
//! out — with a stable `RedshiftIdcApplicationArn` later calls reach it by and
//! the `IdcManagedApplicationArn` of the IAM Identity Center instance it names;
//! the application NAME being what one account's applications are unique on, and
//! a delete freeing it; an ARN that names no application being refused with the
//! modelled 404 by every operation that takes one; a modify changing only the
//! members the request names; the service's `MaxRecords` window and `Marker`
//! cursor; an ARN-valued member that is not an ARN being refused with nothing
//! stored; and the documented tag limit and tag restrictions.

use std::collections::BTreeSet;

use service_com_amazonaws_redshift_redshiftserviceversion20121201::state;

use crate::cluster_lifecycle::{all_text, Answer, Emulator};

/// An IAM Identity Center instance ARN the ACCOUNT holds, read from the shared
/// harness: the instance is a global IAM Identity Center resource, so its ARN
/// carries no Region or account, and the managed application a create registers
/// can only be created in an instance the account holds.
const IDC_INSTANCE_ARN: &str = crate::cluster_lifecycle::IDENTITY_CENTER_INSTANCES[0];

/// A SECOND instance the account holds, so a row can vary the instance a create
/// names.
const OTHER_IDC_INSTANCE_ARN: &str = crate::cluster_lifecycle::IDENTITY_CENTER_INSTANCES[1];

/// The IAM role an application assumes to invoke the IAM Identity Center API.
const IAM_ROLE_ARN: &str = "arn:aws:iam::000000000000:role/RedshiftIdcRole";

/// The members of a `RedshiftIdcApplication` this suite compares between a
/// write's answer and a later read's, so the two records cannot disagree.
///
/// Source: `smithy_query(query="shape", shape="RedshiftIdcApplication")` — every
/// scalar member of the shape; its list members are compared on their own.
const APPLICATION_MEMBERS: [&str; 8] = [
    "RedshiftIdcApplicationArn",
    "RedshiftIdcApplicationName",
    "IdcInstanceArn",
    "IdcManagedApplicationArn",
    "IdcOnboardStatus",
    "IdcDisplayName",
    "IamRoleArn",
    "IdentityNamespace",
];

/// One row of the create table: what the request carries.
struct CreateCase {
    case: &'static str,
    name: String,
    display_name: String,
    instance_arn: &'static str,
    iam_role_arn: &'static str,
    identity_namespace: Option<&'static str>,
    application_type: Option<&'static str>,
    tags: Vec<(&'static str, &'static str)>,
    /// Each entry is one trusted token issuer ARN and the audiences authorized
    /// for it.
    token_issuers: Vec<(&'static str, Vec<&'static str>)>,
    sso_tag_keys: Vec<&'static str>,
    /// The Lake Formation query scope authorization, when the request carries a
    /// service integration at all.
    lake_formation_authorization: Option<&'static str>,
}

impl CreateCase {
    /// A row carrying only the four required members.
    fn required(case: &'static str, name: &str, display_name: &str) -> Self {
        Self {
            case,
            name: name.to_string(),
            display_name: display_name.to_string(),
            instance_arn: IDC_INSTANCE_ARN,
            iam_role_arn: IAM_ROLE_ARN,
            identity_namespace: None,
            application_type: None,
            tags: Vec::new(),
            token_issuers: Vec::new(),
            sso_tag_keys: Vec::new(),
            lake_formation_authorization: None,
        }
    }
}

/// The awsQuery form parameters one create row sends.
///
/// The list member names are the ones the model publishes: `TagList`'s member is
/// named `Tag` and `TagKeyList`'s is named `TagKey`, while
/// `AuthorizedTokenIssuerList`, `AuthorizedAudienceList`, `ServiceIntegrationList`
/// and `LakeFormationServiceIntegrations` name none and so serialize as `member`
/// (`smithy_query(query="shape", shape="AuthorizedTokenIssuerList,ServiceIntegrationList,TagKeyList")`).
fn create_params(row: &CreateCase) -> Vec<(String, String)> {
    let mut params: Vec<(String, String)> = vec![
        ("IdcInstanceArn".to_string(), row.instance_arn.to_string()),
        (
            "RedshiftIdcApplicationName".to_string(),
            row.name.clone(),
        ),
        ("IdcDisplayName".to_string(), row.display_name.clone()),
        ("IamRoleArn".to_string(), row.iam_role_arn.to_string()),
    ];
    if let Some(namespace) = row.identity_namespace {
        params.push(("IdentityNamespace".to_string(), namespace.to_string()));
    }
    if let Some(application_type) = row.application_type {
        params.push(("ApplicationType".to_string(), application_type.to_string()));
    }
    for (position, (key, value)) in row.tags.iter().enumerate() {
        params.push((format!("Tags.Tag.{}.Key", position + 1), (*key).to_string()));
        params.push((
            format!("Tags.Tag.{}.Value", position + 1),
            (*value).to_string(),
        ));
    }
    for (position, (issuer_arn, audiences)) in row.token_issuers.iter().enumerate() {
        let entry = format!("AuthorizedTokenIssuerList.member.{}", position + 1);
        params.push((
            format!("{entry}.TrustedTokenIssuerArn"),
            (*issuer_arn).to_string(),
        ));
        for (audience_position, audience) in audiences.iter().enumerate() {
            params.push((
                format!(
                    "{entry}.AuthorizedAudiencesList.member.{}",
                    audience_position + 1
                ),
                (*audience).to_string(),
            ));
        }
    }
    for (position, key) in row.sso_tag_keys.iter().enumerate() {
        params.push((
            format!("SsoTagKeys.TagKey.{}", position + 1),
            (*key).to_string(),
        ));
    }
    if let Some(authorization) = row.lake_formation_authorization {
        params.push((
            "ServiceIntegrations.member.1.LakeFormation.member.1.LakeFormationQuery.Authorization"
                .to_string(),
            authorization.to_string(),
        ));
    }
    params
}

/// POST one awsQuery action with owned parameters.
async fn call_owned(emulator: &Emulator, action: &str, params: &[(String, String)]) -> Answer {
    let borrowed: Vec<(&str, &str)> = params
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect();
    emulator.call(action, &borrowed).await
}

/// `CreateRedshiftIdcApplication` for one table row.
async fn create_application(emulator: &Emulator, row: &CreateCase) -> Answer {
    call_owned(
        emulator,
        "CreateRedshiftIdcApplication",
        &create_params(row),
    )
    .await
}

/// `CreateRedshiftIdcApplication` with just the four required members.
async fn create_required(emulator: &Emulator, name: &str, display_name: &str) -> Answer {
    create_application(emulator, &CreateCase::required("required", name, display_name)).await
}

/// Assert one refusal row: the answer's status and `<Code>`.
fn assert_refused(case: &str, answer: &Answer, status: u16, code: &str) {
    assert_eq!(
        answer.status, status,
        "case {case}: expected status {status}, got {} with body {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code().as_deref(),
        Some(code),
        "case {case}: expected error code {code:?}, got {:?} with body {}",
        answer.error_code(),
        answer.body
    );
}

/// Every compared member of the single application a body carries.
fn application_members(body: &str) -> Vec<(&'static str, Option<String>)> {
    APPLICATION_MEMBERS
        .into_iter()
        .map(|member| (member, all_text(body, member).into_iter().next()))
        .collect()
}

/// The `<RedshiftIdcApplicationName>` of every application a body carries, in
/// the order the page lists them.
fn listed_applications(body: &str) -> Vec<String> {
    all_text(body, "RedshiftIdcApplicationName")
}

/// The tag key/value pairs a body carries, in document order.
fn listed_tags(body: &str) -> Vec<(String, String)> {
    all_text(body, "Key")
        .into_iter()
        .zip(all_text(body, "Value"))
        .collect()
}

/// `DescribeRedshiftIdcApplications` for one application ARN.
async fn describe_application(emulator: &Emulator, application_arn: &str) -> Answer {
    emulator
        .call(
            "DescribeRedshiftIdcApplications",
            &[("RedshiftIdcApplicationArn", application_arn)],
        )
        .await
}

/// A name of `len` characters that satisfies `RedshiftIdcApplicationName`'s
/// pattern (`^[a-z][a-z0-9]*(-[a-z0-9]+)*$`).
fn name_of_length(len: usize) -> String {
    std::iter::once('a')
        .chain(std::iter::repeat_n('b', len - 1))
        .collect()
}

/// A display name of `len` characters that satisfies `IdcDisplayNameString`'s
/// pattern (`^[\w+=,.@-]+$`).
fn display_name_of_length(len: usize) -> String {
    std::iter::repeat_n('x', len).collect()
}

/// The table every create case walks: rows differ along what the rule
/// generalises over — the length of the name (both modelled bounds) and of the
/// display name, the punctuation each pattern admits, which OPTIONAL members the
/// request carries, how many entries each of its lists has, and the form of the
/// IAM Identity Center instance ARN.
///
/// Bounds and value lists source: `smithy_query(query="operation",
/// operation="CreateRedshiftIdcApplication")` — `RedshiftIdcApplicationName`
/// len 1..63 pattern `^[a-z][a-z0-9]*(-[a-z0-9]+)*$`, `IdcDisplayName` len
/// 1..127 pattern `^[\w+=,.@-]+$`, `IdentityNamespace` len 1..127 pattern
/// `^[a-zA-Z0-9_+.#@$-]+$`, `ApplicationType` one of `None` | `Lakehouse`, and
/// `smithy_query(query="shape", shape="ServiceAuthorization")` — `Enabled` |
/// `Disabled`.
fn create_cases() -> Vec<CreateCase> {
    vec![
        CreateCase::required("the four required members alone", "idc-app", "RedshiftIdc"),
        CreateCase::required(
            "the shortest name and display name",
            &name_of_length(1),
            &display_name_of_length(1),
        ),
        CreateCase::required(
            "the longest name and display name",
            &name_of_length(63),
            &display_name_of_length(127),
        ),
        CreateCase::required(
            "a hyphenated name and every punctuation the display name admits",
            "my-idc-app-1",
            "Team+Editor=1,v.2@aws-x",
        ),
        CreateCase {
            identity_namespace: Some("corp.directory_1"),
            ..CreateCase::required("an identity namespace", "namespaced-app", "Namespaced")
        },
        CreateCase {
            application_type: Some("Lakehouse"),
            ..CreateCase::required("the Lakehouse application type", "lakehouse-app", "Lakehouse")
        },
        CreateCase {
            application_type: Some("None"),
            ..CreateCase::required("the None application type", "plain-app", "Plain")
        },
        CreateCase {
            tags: vec![("owner", "alice")],
            ..CreateCase::required("one tag", "tagged-once", "TaggedOnce")
        },
        CreateCase {
            tags: vec![("owner", "alice"), ("env", "prod"), ("cost-center", "42")],
            sso_tag_keys: vec!["owner", "env"],
            ..CreateCase::required("three tags and two propagated keys", "tagged-thrice", "TaggedThrice")
        },
        CreateCase {
            token_issuers: vec![(
                "arn:aws:sso::000000000000:trustedTokenIssuer/ssoins-1111111111111111/tti-1",
                vec!["https://example.com/app"],
            )],
            ..CreateCase::required("one authorized token issuer", "one-issuer-app", "OneIssuer")
        },
        CreateCase {
            token_issuers: vec![
                (
                    "arn:aws:sso::000000000000:trustedTokenIssuer/ssoins-1111111111111111/tti-1",
                    vec!["https://example.com/app", "https://example.com/other"],
                ),
                (
                    "arn:aws:sso::000000000000:trustedTokenIssuer/ssoins-1111111111111111/tti-2",
                    vec![],
                ),
            ],
            ..CreateCase::required("two authorized token issuers", "two-issuer-app", "TwoIssuers")
        },
        CreateCase {
            lake_formation_authorization: Some("Enabled"),
            ..CreateCase::required("a Lake Formation service integration", "integrated-app", "Integrated")
        },
        CreateCase {
            instance_arn: OTHER_IDC_INSTANCE_ARN,
            iam_role_arn: "arn:aws:iam::000000000000:role/service-role/OtherRedshiftIdcRole",
            ..CreateCase::required("another instance and a path-qualified role", "second-instance-app", "SecondInstance")
        },
    ]
}

/// Each create stores the application exactly as the request configured it,
/// mints an ARN of its own that a later read reaches it by, and reports the IAM
/// Identity Center managed application of the instance the request named.
#[tokio::test]
async fn every_created_application_is_stored_exactly_as_the_request_configured_it() {
    let emulator = Emulator::new();
    let mut arns: BTreeSet<String> = BTreeSet::new();

    for row in create_cases() {
        let case = row.case;
        let created = create_application(&emulator, &row).await;
        assert_eq!(
            created.status, 200,
            "case {case}: expected 200, got {} with body {}",
            created.status, created.body
        );

        for (member, expected) in [
            ("RedshiftIdcApplicationName", Some(row.name.as_str())),
            ("IdcDisplayName", Some(row.display_name.as_str())),
            ("IdcInstanceArn", Some(row.instance_arn)),
            ("IamRoleArn", Some(row.iam_role_arn)),
            ("IdentityNamespace", row.identity_namespace),
            ("ApplicationType", row.application_type),
        ] {
            assert_eq!(
                created.text(member).as_deref(),
                expected,
                "case {case}: expected {member} {expected:?}, got {:?}",
                created.text(member)
            );
        }
        assert_eq!(
            created.text("IdcOnboardStatus").as_deref(),
            Some(state::REDSHIFT_IDC_ONBOARD_STATUS),
            "case {case}: expected the registered onboard status, got {:?}",
            created.text("IdcOnboardStatus")
        );

        // Every list the request carried is reported back, in the order it was
        // given; a list the request left out is reported by no entry at all.
        let expected_issuers: Vec<String> = row
            .token_issuers
            .iter()
            .map(|(issuer_arn, _)| (*issuer_arn).to_string())
            .collect();
        assert_eq!(
            all_text(&created.body, "TrustedTokenIssuerArn"),
            expected_issuers,
            "case {case}: expected the requested token issuers, got {:?}",
            all_text(&created.body, "TrustedTokenIssuerArn")
        );
        let expected_audiences: Vec<String> = row
            .token_issuers
            .iter()
            .flat_map(|(_, audiences)| audiences.iter().map(|value| (*value).to_string()))
            .collect();
        assert_eq!(
            audiences_of(&created.body),
            expected_audiences,
            "case {case}: expected the requested audiences, got {:?}",
            audiences_of(&created.body)
        );
        let expected_keys: Vec<String> = row
            .sso_tag_keys
            .iter()
            .map(|key| (*key).to_string())
            .collect();
        assert_eq!(
            all_text(&created.body, "TagKey"),
            expected_keys,
            "case {case}: expected the requested propagated tag keys, got {:?}",
            all_text(&created.body, "TagKey")
        );
        assert_eq!(
            all_text(&created.body, "Authorization")
                .into_iter()
                .next()
                .as_deref(),
            row.lake_formation_authorization,
            "case {case}: expected the requested service integration, got {:?}",
            all_text(&created.body, "Authorization")
        );

        let arn = created
            .text("RedshiftIdcApplicationArn")
            .unwrap_or_else(|| panic!("case {case}: a created application carries an ARN"));
        // The ARN this emulator mints for a Redshift resource of the Region the
        // shared harness signs every request for, with the canonical emulator
        // account.
        let expected_prefix = engine::emulator_arn("redshift", "us-west-2", "redshiftidcapplication:");
        assert!(
            arn.starts_with(&expected_prefix),
            "case {case}: expected an ARN under {expected_prefix:?}, got {arn:?}"
        );
        assert!(
            arns.insert(arn.clone()),
            "case {case}: expected an ARN of its own, got the already-minted {arn:?}"
        );

        let managed = created
            .text("IdcManagedApplicationArn")
            .unwrap_or_else(|| panic!("case {case}: a created application carries a managed ARN"));
        let instance_id = row
            .instance_arn
            .split('/')
            .next_back()
            .expect("the case's instance ARN carries an id");
        assert!(
            managed.starts_with("arn:aws:sso::")
                && managed.contains(&format!("application/{instance_id}/apl-")),
            "case {case}: expected the managed application of instance {instance_id:?}, got {managed:?}"
        );

        // The round trip: the application the create answered is exactly what a
        // later read of that ARN reports, tags included.
        let described = describe_application(&emulator, &arn).await;
        assert_eq!(
            described.status, 200,
            "case {case}: expected the created application readable, got {} with body {}",
            described.status, described.body
        );
        assert_eq!(
            listed_applications(&described.body),
            vec![row.name.clone()],
            "case {case}: expected exactly the created application, got {:?}",
            listed_applications(&described.body)
        );
        assert_eq!(
            application_members(&described.body),
            application_members(&created.body),
            "case {case}: expected the read to report the created record, got {:?}",
            application_members(&described.body)
        );
        let expected_tags: Vec<(String, String)> = row
            .tags
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        assert_eq!(
            listed_tags(&described.body),
            expected_tags,
            "case {case}: expected the requested tags, got {:?}",
            listed_tags(&described.body)
        );
    }
}

/// Every audience a body carries, in document order: the entries nested in the
/// `AuthorizedAudiencesList` of each token issuer.
fn audiences_of(body: &str) -> Vec<String> {
    all_text(body, "AuthorizedAudiencesList")
        .iter()
        .flat_map(|list| all_text(list, "member"))
        .collect()
}

/// An application NAME is what one account's applications are unique on, and a
/// delete frees the name for a new application.
#[tokio::test]
async fn an_application_name_is_unique_and_a_delete_frees_it() {
    let emulator = Emulator::new();

    // The rows differ along the spelling and the length of the name a second
    // create re-uses, and each is followed by a name one character off it, which
    // must be admitted: the refusal is keyed on the whole name, not a prefix.
    for (case, name) in [
        ("a plain name", "unique-app".to_string()),
        ("the shortest name", name_of_length(1)),
        ("the longest name", name_of_length(63)),
        ("a name of digits and hyphens", "app-2-0".to_string()),
    ] {
        let first = create_required(&emulator, &name, "First").await;
        assert_eq!(
            first.status, 200,
            "case {case}: expected the first create served with 200, got {} with body {}",
            first.status, first.body
        );
        let arn = first
            .text("RedshiftIdcApplicationArn")
            .unwrap_or_else(|| panic!("case {case}: a created application carries an ARN"));

        // A second create of the SAME name creates nothing, whatever else it
        // carries: a different display name does not make it a new application.
        let again = create_required(&emulator, &name, "Second").await;
        assert_refused(case, &again, 400, "RedshiftIdcApplicationAlreadyExists");
        // The refusal is the service's own sentence, with the name the request
        // carried filled into it: the fixed text is written out here so a
        // wording change is caught, and compared with the helper every handler
        // uses so two operations cannot word it differently.
        assert_eq!(
            again.text("Message"),
            Some(format!(
                "You already have a RedshiftIdcApplication with the name {name}. Use a different name."
            )),
            "case {case}: expected the refusal to name the taken name, got {:?}",
            again.text("Message")
        );
        assert_eq!(
            again.text("Message"),
            Some(state::redshift_idc_application_already_exists(&name)),
            "case {case}: expected every create to word the refusal alike, got {:?}",
            again.text("Message")
        );
        let described = describe_application(&emulator, &arn).await;
        assert_eq!(
            described.text("IdcDisplayName").as_deref(),
            Some("First"),
            "case {case}: expected the refused create to change nothing, got {:?}",
            described.text("IdcDisplayName")
        );

        // A name one character off is a different application. The longest row
        // is already at the modelled maximum, so its neighbour is built to stay
        // inside the bound rather than past it.
        let neighbour = if name.len() + 2 > 63 {
            format!("{}-x", &name[..61])
        } else {
            format!("{name}-x")
        };
        let other = create_required(&emulator, &neighbour, "Neighbour").await;
        assert_eq!(
            other.status, 200,
            "case {case}: expected the name {neighbour:?} admitted, got {} with body {}",
            other.status, other.body
        );

        // The delete frees the name: the same create now succeeds again, and the
        // deleted application is gone from every later read.
        let deleted = emulator
            .call(
                "DeleteRedshiftIdcApplication",
                &[("RedshiftIdcApplicationArn", arn.as_str())],
            )
            .await;
        assert_eq!(
            deleted.status, 200,
            "case {case}: expected the delete served with 200, got {} with body {}",
            deleted.status, deleted.body
        );
        let gone = describe_application(&emulator, &arn).await;
        assert_refused(case, &gone, 404, "RedshiftIdcApplicationNotExists");
        let recreated = create_required(&emulator, &name, "Third").await;
        assert_eq!(
            recreated.status, 200,
            "case {case}: expected the freed name re-usable, got {} with body {}",
            recreated.status, recreated.body
        );
    }
}

/// An ARN that names no application is refused with the modelled 404 fault by
/// every operation that takes one, and an ARN that names one is served.
#[tokio::test]
async fn an_arn_naming_no_application_is_refused_by_every_operation_that_takes_one() {
    let emulator = Emulator::new();
    let created = create_required(&emulator, "present-app", "Present").await;
    assert_eq!(
        created.status, 200,
        "case seed present-app: expected 200, got {} with body {}",
        created.status, created.body
    );
    let present = created
        .text("RedshiftIdcApplicationArn")
        .expect("the seeded application carries an ARN");

    // The rows differ along the FORM of the ARN that names nothing: another
    // account's, another Region's, another resource type's, a well-formed ARN of
    // this type with an unknown id, and a value that is no ARN at all.
    let absent = [
        (
            "another account",
            "arn:aws:redshift:us-west-2:111111111111:redshiftidcapplication:00000000-0000-0000-0000-000000000000".to_string(),
        ),
        (
            "another region",
            present.replace("us-west-2", "eu-central-1"),
        ),
        (
            "another resource type",
            present.replace("redshiftidcapplication:", "qev2idcapplication:"),
        ),
        (
            "an unknown id of this type",
            format!("{}-nope", present),
        ),
        ("no ARN at all", "not-an-arn".to_string()),
    ];

    for (case, application_arn) in absent {
        for operation in [
            "DescribeRedshiftIdcApplications",
            "ModifyRedshiftIdcApplication",
            "DeleteRedshiftIdcApplication",
        ] {
            let answer = emulator
                .call(
                    operation,
                    &[("RedshiftIdcApplicationArn", application_arn.as_str())],
                )
                .await;
            assert_refused(
                &format!("{operation} on {case}"),
                &answer,
                404,
                "RedshiftIdcApplicationNotExists",
            );
            // The refusal is the service's own sentence with the ARN the
            // request supplied filled into it, written out here so a wording
            // change is caught …
            assert_eq!(
                answer.text("Message"),
                Some(format!(
                    "The RedshiftIdcApplication with the ARN {application_arn} wasn't found."
                )),
                "case {operation} on {case}: expected the refusal to name the ARN, got {:?}",
                answer.text("Message")
            );
            // … and compared with the helper every operation enforcing this
            // precondition uses, so none of them can word it differently.
            assert_eq!(
                answer.text("Message"),
                Some(state::redshift_idc_application_not_exists(
                    &application_arn
                )),
                "case {operation} on {case}: expected every operation to word it alike, got {:?}",
                answer.text("Message")
            );
        }
    }

    // The ARN that DOES name an application is served by each of them, and the
    // seeded application survives every refusal above.
    let described = describe_application(&emulator, &present).await;
    assert_eq!(
        described.status, 200,
        "case the present ARN: expected the read served, got {} with body {}",
        described.status, described.body
    );
    let modified = emulator
        .call(
            "ModifyRedshiftIdcApplication",
            &[
                ("RedshiftIdcApplicationArn", present.as_str()),
                ("IdcDisplayName", "StillPresent"),
            ],
        )
        .await;
    assert_eq!(
        modified.status, 200,
        "case the present ARN: expected the modify served, got {} with body {}",
        modified.status, modified.body
    );
    let deleted = emulator
        .call(
            "DeleteRedshiftIdcApplication",
            &[("RedshiftIdcApplicationArn", present.as_str())],
        )
        .await;
    assert_eq!(
        deleted.status, 200,
        "case the present ARN: expected the delete served, got {} with body {}",
        deleted.status, deleted.body
    );
}

/// One row of the modify table: the parameters the request carries beyond the
/// ARN, and the members a later read must report changed.
struct ModifyCase {
    case: &'static str,
    params: Vec<(&'static str, &'static str)>,
    expected: Vec<(&'static str, &'static str)>,
}

/// A modify changes only the members the request NAMES; every other member of
/// the application is exactly what the create left.
#[tokio::test]
async fn a_modify_changes_only_the_members_the_request_names() {
    // The rows differ along WHICH modifiable member the request carries and how
    // many of them at once.
    //
    // Source: `smithy_query(query="operation",
    // operation="ModifyRedshiftIdcApplication")` — the modifiable members are
    // `IdentityNamespace`, `IamRoleArn`, `IdcDisplayName`,
    // `AuthorizedTokenIssuerList` and `ServiceIntegrations`.
    let rows = [
        ModifyCase {
            case: "the display name alone",
            params: vec![("IdcDisplayName", "Renamed")],
            expected: vec![("IdcDisplayName", "Renamed")],
        },
        ModifyCase {
            case: "the identity namespace alone",
            params: vec![("IdentityNamespace", "other.namespace#1")],
            expected: vec![("IdentityNamespace", "other.namespace#1")],
        },
        ModifyCase {
            case: "the IAM role alone",
            params: vec![("IamRoleArn", "arn:aws:iam::000000000000:role/Rotated")],
            expected: vec![("IamRoleArn", "arn:aws:iam::000000000000:role/Rotated")],
        },
        ModifyCase {
            case: "the token issuer list alone",
            params: vec![(
                "AuthorizedTokenIssuerList.member.1.TrustedTokenIssuerArn",
                "arn:aws:sso::000000000000:trustedTokenIssuer/ssoins-1111111111111111/tti-9",
            )],
            expected: vec![(
                "TrustedTokenIssuerArn",
                "arn:aws:sso::000000000000:trustedTokenIssuer/ssoins-1111111111111111/tti-9",
            )],
        },
        ModifyCase {
            case: "a service integration alone",
            params: vec![(
                "ServiceIntegrations.member.1.LakeFormation.member.1.LakeFormationQuery.Authorization",
                "Disabled",
            )],
            expected: vec![("Authorization", "Disabled")],
        },
        ModifyCase {
            case: "the display name, the namespace and the role at once",
            params: vec![
                ("IdcDisplayName", "AllThree"),
                ("IdentityNamespace", "all_three"),
                ("IamRoleArn", "arn:aws:iam::000000000000:role/AllThree"),
            ],
            expected: vec![
                ("IdcDisplayName", "AllThree"),
                ("IdentityNamespace", "all_three"),
                ("IamRoleArn", "arn:aws:iam::000000000000:role/AllThree"),
            ],
        },
        ModifyCase {
            case: "no modifiable member at all",
            params: Vec::new(),
            expected: Vec::new(),
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        // A fresh emulator per row, so each row's "unchanged" assertion is about
        // the create's own record and not a previous row's modify.
        let emulator = Emulator::new();
        let seed = CreateCase {
            identity_namespace: Some("seed.namespace"),
            application_type: Some("Lakehouse"),
            tags: vec![("owner", "alice")],
            token_issuers: vec![(
                "arn:aws:sso::000000000000:trustedTokenIssuer/ssoins-1111111111111111/tti-1",
                vec!["https://example.com/app"],
            )],
            sso_tag_keys: vec!["owner"],
            ..CreateCase::required("seed", &format!("modify-app-{index}"), "Seeded")
        };
        let created = create_application(&emulator, &seed).await;
        assert_eq!(
            created.status, 200,
            "case {case}: expected the seed created, got {} with body {}",
            created.status, created.body
        );
        let arn = created
            .text("RedshiftIdcApplicationArn")
            .unwrap_or_else(|| panic!("case {case}: the seed carries an ARN"));

        let mut params: Vec<(&str, &str)> = vec![("RedshiftIdcApplicationArn", arn.as_str())];
        params.extend(row.params.iter().copied());
        let modified = emulator
            .call("ModifyRedshiftIdcApplication", &params)
            .await;
        assert_eq!(
            modified.status, 200,
            "case {case}: expected the modify served with 200, got {} with body {}",
            modified.status, modified.body
        );

        for (member, value) in &row.expected {
            assert_eq!(
                modified.text(member).as_deref(),
                Some(*value),
                "case {case}: expected {member} {value:?} after the modify, got {:?}",
                modified.text(member)
            );
        }

        // Every member the request did not name is still the create's value.
        let changed: Vec<&str> = row.expected.iter().map(|(member, _)| *member).collect();
        for (member, seeded) in [
            ("RedshiftIdcApplicationName", seed.name.as_str()),
            ("IdcDisplayName", "Seeded"),
            ("IdentityNamespace", "seed.namespace"),
            ("IamRoleArn", IAM_ROLE_ARN),
            ("IdcInstanceArn", IDC_INSTANCE_ARN),
            ("ApplicationType", "Lakehouse"),
            ("IdcOnboardStatus", state::REDSHIFT_IDC_ONBOARD_STATUS),
            (
                "TrustedTokenIssuerArn",
                "arn:aws:sso::000000000000:trustedTokenIssuer/ssoins-1111111111111111/tti-1",
            ),
        ] {
            if changed.contains(&member) {
                continue;
            }
            assert_eq!(
                modified.text(member).as_deref(),
                Some(seeded),
                "case {case}: expected the unnamed {member} to keep {seeded:?}, got {:?}",
                modified.text(member)
            );
        }
        assert_eq!(
            listed_tags(&modified.body),
            vec![("owner".to_string(), "alice".to_string())],
            "case {case}: expected the create's tags kept, got {:?}",
            listed_tags(&modified.body)
        );

        // The modify's answer is what a later read reports: one record, not two.
        let described = describe_application(&emulator, &arn).await;
        assert_eq!(
            application_members(&described.body),
            application_members(&modified.body),
            "case {case}: expected the read to report the modified record, got {:?}",
            application_members(&described.body)
        );
        assert_eq!(
            listed_applications(&described.body).len(),
            1,
            "case {case}: expected the modify to change one application, got {:?}",
            listed_applications(&described.body)
        );
    }
}

/// `DescribeRedshiftIdcApplications` hands out one page of the service's
/// documented `MaxRecords` window at a time, with a `Marker` cursor a later call
/// resumes the same sequence from.
///
/// The window is the one every paginated `Describe*` of this service shares
/// (`state::MAX_RECORDS_MIN` ..= `state::MAX_RECORDS_MAX`).
#[tokio::test]
async fn the_read_pages_under_the_services_maxrecords_window() {
    let emulator = Emulator::new();
    let minimum = state::MAX_RECORDS_MIN;
    let maximum = state::MAX_RECORDS_MAX;
    let total = minimum as usize + 5;
    for index in 0..total {
        let answer = create_required(&emulator, &format!("page-app-{index}"), "Paged").await;
        assert_eq!(
            answer.status, 200,
            "case seed {index}: expected 200, got {} with body {}",
            answer.status, answer.body
        );
    }

    // Each published bound of the window is served, and below the minimum — the
    // only bound the service enforces — is refused.
    for (case, max_records, accepted) in [
        ("one below the minimum", (minimum - 1).to_string(), false),
        ("the minimum", minimum.to_string(), true),
        ("the maximum", maximum.to_string(), true),
        ("one past the maximum", (maximum + 1).to_string(), true),
        ("zero", "0".to_string(), false),
    ] {
        let answer = emulator
            .call(
                "DescribeRedshiftIdcApplications",
                &[("MaxRecords", max_records.as_str())],
            )
            .await;
        if accepted {
            assert_eq!(
                answer.status, 200,
                "case {case}: expected MaxRecords={max_records} served with 200, got {} with body {}",
                answer.status, answer.body
            );
        } else {
            assert_refused(case, &answer, 400, "InvalidParameterValue");
        }
    }

    // The default window takes every application in one page, with no cursor.
    let unpaged = emulator
        .call("DescribeRedshiftIdcApplications", &[])
        .await;
    let all_names = listed_applications(&unpaged.body);
    assert_eq!(
        all_names.len(),
        total,
        "case the default window: expected all {total} applications in one page, got {all_names:?}"
    );
    assert_eq!(
        unpaged.text("Marker"),
        None,
        "case the default window: expected no cursor, got {:?}",
        unpaged.text("Marker")
    );

    // A page at the window's minimum leaves a cursor the next call resumes from,
    // and the two pages are the unpaged sequence split in order.
    let page_size = minimum.to_string();
    let first_page = emulator
        .call(
            "DescribeRedshiftIdcApplications",
            &[("MaxRecords", page_size.as_str())],
        )
        .await;
    let first_names = listed_applications(&first_page.body);
    assert_eq!(
        first_names.len(),
        minimum as usize,
        "case the first page: expected {minimum} applications, got {first_names:?}"
    );
    let marker = first_page
        .text("Marker")
        .expect("case the first page: records remain, so the page carries a cursor");
    let second_page = emulator
        .call(
            "DescribeRedshiftIdcApplications",
            &[
                ("MaxRecords", page_size.as_str()),
                ("Marker", marker.as_str()),
            ],
        )
        .await;
    let second_names = listed_applications(&second_page.body);
    assert_eq!(
        second_names.len(),
        total - minimum as usize,
        "case the second page: expected the remaining {} applications, got {second_names:?}",
        total - minimum as usize
    );
    assert_eq!(
        second_page.text("Marker"),
        None,
        "case the second page: nothing remains, so expected no cursor, got {:?}",
        second_page.text("Marker")
    );
    let walked: Vec<String> = first_names.into_iter().chain(second_names).collect();
    assert_eq!(
        walked, all_names,
        "case the walked pages: expected the unpaged sequence in order, got {walked:?}"
    );

    // A cursor no page handed out resumes nothing.
    for (case, cursor) in [
        ("an invented cursor", "nonsense"),
        ("a bare offset", "20"),
        ("an offset with the wrong tag", "20.deadbeef"),
    ] {
        let bogus = emulator
            .call("DescribeRedshiftIdcApplications", &[("Marker", cursor)])
            .await;
        assert_refused(case, &bogus, 400, "InvalidParameterValue");
    }
}

/// `IdcInstanceArn`, `IamRoleArn` and each `TrustedTokenIssuerArn` name an AWS
/// resource, so a value that is not an ARN at all — or is longer than an ARN can
/// be — names none and is refused with nothing stored, by the create and by the
/// modify alike.
#[tokio::test]
async fn an_arn_valued_member_that_is_not_an_arn_is_refused_and_stores_nothing() {
    let emulator = Emulator::new();
    let too_long = format!(
        "arn:aws:sso:::instance/{}",
        "a".repeat(state::MAX_ARN_LENGTH)
    );

    // The rows differ along HOW the value fails to be an ARN: no prefix, an
    // empty partition or service, a missing segment, an empty resource, and a
    // value past the published ARN length. Each is sent for every ARN-valued
    // member the request carries, so no member is admitted by a rule another
    // refuses.
    let malformed = [
        ("a bare word", "not-an-arn".to_string()),
        ("an empty partition", "arn::sso:::instance/i".to_string()),
        ("an empty service", "arn:aws::::instance/i".to_string()),
        ("too few segments", "arn:aws:sso:instance".to_string()),
        ("an empty resource", "arn:aws:sso::::".to_string()),
        ("longer than an ARN can be", too_long),
    ];

    for (position, (case, value)) in malformed.iter().enumerate() {
        for member in [
            "IdcInstanceArn",
            "IamRoleArn",
            "AuthorizedTokenIssuerList.member.1.TrustedTokenIssuerArn",
        ] {
            let row = CreateCase::required(
                "malformed",
                &format!("malformed-app-{position}"),
                "Malformed",
            );
            let mut params = create_params(&row);
            let mut substituted = false;
            for param in params.iter_mut() {
                if param.0 == member {
                    param.1 = value.clone();
                    substituted = true;
                }
            }
            if !substituted {
                // A member no create row carries by default — the token issuer
                // list — is added to the request for this case.
                params.push((member.to_string(), value.clone()));
            }
            let answer = call_owned(&emulator, "CreateRedshiftIdcApplication", &params).await;
            assert_refused(
                &format!("{member} {case}"),
                &answer,
                400,
                "InvalidParameterValue",
            );
            // The refusal names the member and what is wrong with its value:
            // the length and the bound when the value is past the published ARN
            // length, the value itself otherwise. Both sentences are written out
            // here, so a wording change is caught.
            let expected_message = if value.len() > state::MAX_ARN_LENGTH {
                format!(
                    "Invalid value for {member}: the value is {} characters long; an Amazon Resource Name (ARN) is at most {} characters.",
                    value.len(),
                    state::MAX_ARN_LENGTH
                )
            } else {
                format!(
                    "Invalid value for {member}: '{value}'. The value must be an Amazon Resource Name (ARN)."
                )
            };
            assert_eq!(
                answer.text("Message"),
                Some(expected_message),
                "case {member} {case}: expected the refusal to name the member and the value, got {:?}",
                answer.text("Message")
            );
        }
    }

    // Nothing was stored: an unfiltered read of the store reports an empty page.
    let described = emulator
        .call("DescribeRedshiftIdcApplications", &[])
        .await;
    assert_eq!(
        described.status, 200,
        "case the store after the refusals: expected the read served, got {} with body {}",
        described.status, described.body
    );
    assert!(
        listed_applications(&described.body).is_empty(),
        "case the store after the refusals: expected no application stored, got {:?}",
        listed_applications(&described.body)
    );

    // A well-formed pair of ARNs for the same name is admitted, so the refusals
    // above are about the VALUES and not about the request.
    let admitted = create_required(&emulator, "malformed-app-0", "Admitted").await;
    assert_eq!(
        admitted.status, 200,
        "case a well-formed pair: expected 200, got {} with body {}",
        admitted.status, admitted.body
    );
    let arn = admitted
        .text("RedshiftIdcApplicationArn")
        .expect("the admitted application carries an ARN");

    // The modify holds the same members to the same rule, and changes nothing
    // when it refuses.
    for (case, member, value) in [
        ("the IAM role", "IamRoleArn", "not-an-arn"),
        (
            "a token issuer",
            "AuthorizedTokenIssuerList.member.1.TrustedTokenIssuerArn",
            "also-not-an-arn",
        ),
    ] {
        let refused = emulator
            .call(
                "ModifyRedshiftIdcApplication",
                &[
                    ("RedshiftIdcApplicationArn", arn.as_str()),
                    ("IdcDisplayName", "Rejected"),
                    (member, value),
                ],
            )
            .await;
        assert_refused(case, &refused, 400, "InvalidParameterValue");
        assert_eq!(
            refused.text("Message"),
            Some(format!(
                "Invalid value for {member}: '{value}'. The value must be an Amazon Resource Name (ARN)."
            )),
            "case {case}: expected the refusal to name the member and the value, got {:?}",
            refused.text("Message")
        );
        let described = describe_application(&emulator, &arn).await;
        assert_eq!(
            described.text("IdcDisplayName").as_deref(),
            Some("Admitted"),
            "case {case}: expected the refused modify to change nothing, got {:?}",
            described.text("IdcDisplayName")
        );
    }
}

/// The documented per-resource tag limit and the documented tag restrictions
/// bound the tags a created application carries, and a refused create stores
/// nothing.
#[tokio::test]
async fn the_documented_tag_limit_and_restrictions_bound_a_created_application() {
    let emulator = Emulator::new();
    let limit = state::MAX_TAGS_PER_RESOURCE;

    // At the limit the create is served; one past it is refused.
    let keys: Vec<String> = (0..=limit).map(|index| format!("key-{index}")).collect();
    for (case, count, accepted) in [
        ("one tag below the limit", limit - 1, true),
        ("exactly the limit", limit, true),
        ("one tag past the limit", limit + 1, false),
    ] {
        let tags: Vec<(&str, &str)> = keys
            .iter()
            .take(count)
            .map(|key| (key.as_str(), "v"))
            .collect();
        let mut params = vec![
            ("IdcInstanceArn".to_string(), IDC_INSTANCE_ARN.to_string()),
            (
                "RedshiftIdcApplicationName".to_string(),
                format!("tag-app-{count}"),
            ),
            ("IdcDisplayName".to_string(), "Tagged".to_string()),
            ("IamRoleArn".to_string(), IAM_ROLE_ARN.to_string()),
        ];
        for (position, (key, value)) in tags.iter().enumerate() {
            params.push((format!("Tags.Tag.{}.Key", position + 1), (*key).to_string()));
            params.push((
                format!("Tags.Tag.{}.Value", position + 1),
                (*value).to_string(),
            ));
        }
        let answer = call_owned(&emulator, "CreateRedshiftIdcApplication", &params).await;
        if accepted {
            assert_eq!(
                answer.status, 200,
                "case {case}: expected {count} tags served with 200, got {} with body {}",
                answer.status, answer.body
            );
            assert_eq!(
                listed_tags(&answer.body).len(),
                count,
                "case {case}: expected {count} tags on the application, got {:?}",
                listed_tags(&answer.body)
            );
        } else {
            assert_refused(case, &answer, 400, "TagLimitExceededFault");
        }
    }

    // A tag the published restrictions refuse is refused whichever side of the
    // pair breaks them: an empty key, a key or value longer than its bound, and
    // the prefix AWS reserves for its own tags.
    let long_key = "k".repeat(state::TAG_KEY_MAX_LEN + 1);
    let long_value = "v".repeat(state::TAG_VALUE_MAX_LEN + 1);
    for (case, key, value) in [
        ("an empty key", "", "v"),
        ("a key past its bound", long_key.as_str(), "v"),
        ("a value past its bound", "k", long_value.as_str()),
        ("a reserved key prefix", "aws:owner", "v"),
        ("a reserved value prefix", "owner", "aws:alice"),
    ] {
        let params = vec![
            ("IdcInstanceArn".to_string(), IDC_INSTANCE_ARN.to_string()),
            (
                "RedshiftIdcApplicationName".to_string(),
                "invalid-tag-app".to_string(),
            ),
            ("IdcDisplayName".to_string(), "InvalidTag".to_string()),
            ("IamRoleArn".to_string(), IAM_ROLE_ARN.to_string()),
            ("Tags.Tag.1.Key".to_string(), key.to_string()),
            ("Tags.Tag.1.Value".to_string(), value.to_string()),
        ];
        let answer = call_owned(&emulator, "CreateRedshiftIdcApplication", &params).await;
        assert_refused(case, &answer, 400, "InvalidTagFault");
    }

    // The refused creates stored nothing: the name each was refused under is
    // still free.
    let free = create_required(&emulator, "invalid-tag-app", "Free").await;
    assert_eq!(
        free.status, 200,
        "case the name of a refused create: expected it still free, got {} with body {}",
        free.status, free.body
    );
}
