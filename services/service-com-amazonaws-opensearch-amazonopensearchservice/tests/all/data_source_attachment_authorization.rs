// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the AUTHORIZATION of the operations on an OpenSearch
//! application's data source attachments — `AttachDataSource`, `DetachDataSource`,
//! `DescribeDataSourceAttachment` and `ListDataSourceAttachments` — driven over the real
//! wire path (a `restJson1` HTTP request through the multi-service router into the
//! mounted service), so what is asserted is what a client sees.
//!
//! The rule under test: before it reads or writes anything, each of those operations
//! evaluates the identity-based IAM policies in force on the principal the request's
//! credentials name against `es:<OperationName>` on the domain resource of the account
//! and Region the request was signed for, and a request no statement allows — or one an
//! explicit `Deny` covers — is refused with the modelled `AccessDeniedException` (HTTP
//! 403) and performs nothing.
//!
//! The rows differ along what the rule generalises over: which of the four operations is
//! called, which principal form signs the request (an assumed-role session and an IAM
//! user signing with its own long-term key), which kind of identity policy carries the
//! grant (the principal's own inline policy and a policy of a group it belongs to) and
//! the statement's action pattern, resource scope and effect.
//!
//! The principals, their policies and the session live in the IAM and STS stores of the
//! SAME shared engine — the stores those services' own handlers write — so the documents
//! under test are the ones `iam:PutRolePolicy` and `iam:PutUserPolicy` store and the
//! principal is the one `sts:GetCallerIdentity` answers for the key the request signed
//! with. The wire harness comes from [`super::domain_lifecycle`] and the refusal wording
//! from [`super::index_authorization`]; this suite adds no second copy of either.

use std::collections::{BTreeMap, BTreeSet};
use std::time::SystemTime;

use serde_json::{json, Value};
use service_com_amazonaws_iam_awsidentitymanagementv20100508::ops::types::User;
use service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam;
use service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts;

use super::data_source_attachments::{application, attach, list_attachments};
use super::data_source_registry::created_domain;
use super::domain_lifecycle::{Answer, Emulator, PREFIX, REGION, SETTLE_WAIT};
use super::index_authorization::{assert_refused, Refusal};

/// The four operations of this service that act on an application's data source
/// attachments (`smithy_query(query="digest")`: each is bound under
/// `/opensearch/application/{id}/…DataSource…` and models `AccessDeniedException`, 403).
const ATTACHMENT_OPERATIONS: [&str; 4] = [
    "AttachDataSource",
    "DetachDataSource",
    "DescribeDataSourceAttachment",
    "ListDataSourceAttachments",
];

/// The role the session below is a session of.
const ROLE: &str = "opensearch-attachment-writer";

/// The session name the assumed-role credential carries.
const SESSION: &str = "attachment-session";

/// The access key id of the assumed-role session's credentials.
const SESSION_KEY_ID: &str = "ASIAEXAMPLEATTACH001";

/// The IAM user that signs with its own long-term access key.
const USER: &str = "opensearch-attachment-user";

/// That user's long-term access key id.
const USER_KEY_ID: &str = "AKIAEXAMPLEATTACH001";

/// The group the user belongs to, whose policies are in force on it too.
const GROUP: &str = "opensearch-attachment-group";

/// The name of the inline policy every principal below carries.
const POLICY_NAME: &str = "inline";

/// How long the seeded session lasts: an hour, STS's own default.
const SESSION_SECONDS: i32 = 3_600;

/// The domain the fixture holds, which is the data source its privileged attachment
/// links the application to.
const DOMAIN: &str = "attachment-authz-src";

/// Which principal a row signs as: the two forms whose identity policies this emulator
/// reads.
///
/// `pub(crate)` so a sibling suite that drives the SAME two principals through another
/// surface reaches these records rather than seeding a second copy of them.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Principal {
    /// An assumed-role session, whose ROLE carries the policies.
    Session,
    /// An IAM user signing with its own long-term key, which carries its own.
    User,
}

impl Principal {
    /// What the case is called when an assertion fails.
    pub(crate) fn case(self) -> &'static str {
        match self {
            Self::Session => "an assumed-role session",
            Self::User => "an IAM user's own access key",
        }
    }

    /// The access key id a request of this principal carries.
    pub(crate) fn access_key_id(self) -> &'static str {
        match self {
            Self::Session => SESSION_KEY_ID,
            Self::User => USER_KEY_ID,
        }
    }

    /// The principal ARN this caller speaks for — what a refusal must name.
    pub(crate) fn arn(self) -> String {
        let account = ::engine::emulator_account_id();
        match self {
            Self::Session => format!("arn:aws:sts::{account}:assumed-role/{ROLE}/{SESSION}"),
            Self::User => format!("arn:aws:iam::{account}:user/{USER}"),
        }
    }
}

/// The `Resource` these operations are decided against: every domain of the account and
/// Region the request was signed for, composed here from the partition, this service's
/// namespace, the Region, the account this emulator serves and the domain resource
/// pattern — so the assertion does not ask the service to confirm its own answer.
fn every_domain_arn() -> String {
    format!(
        "arn:aws:es:{REGION}:{}:domain/*",
        ::engine::emulator_account_id()
    )
}

/// One attachment operation, signed with `access_key_id`, against the application `id`
/// and the data source `data_source_arn` (which `ListDataSourceAttachments` names none
/// of, taking the application alone).
async fn attachment_call(
    emulator: &Emulator,
    access_key_id: &str,
    operation: &str,
    id: &str,
    data_source_arn: &str,
) -> Answer {
    let named = json!({ "dataSourceArn": data_source_arn });
    let (segment, body) = match operation {
        "AttachDataSource" => ("attachDataSource", named),
        "DetachDataSource" => ("detachDataSource", named),
        "DescribeDataSourceAttachment" => ("describeDataSourceAttachment", named),
        "ListDataSourceAttachments" => ("listDataSourceAttachments", json!({})),
        other => panic!("case {other:?}: not a data source attachment operation of this service"),
    };
    emulator
        .call_as(
            access_key_id,
            "POST",
            &format!("{PREFIX}/opensearch/application/{id}/{segment}"),
            Some(body),
        )
        .await
}

/// Record the assumed-role session [`SESSION_KEY_ID`] speaks for, as `sts:AssumeRole`
/// records one, and the IAM user [`USER_KEY_ID`] belongs to, as `iam:CreateUser` and
/// `iam:CreateAccessKey` record one — both in the stores those services' own handlers
/// write.
///
/// `pub(crate)` for the reason [`Principal`] is.
pub(crate) fn given_principals(emulator: &Emulator) {
    let now = SystemTime::now();
    let account = ::engine::emulator_account_id();
    let role_arn = format!("arn:aws:iam::{account}:role/{ROLE}");
    let vend = sts::NewSession {
        role_arn: Some(role_arn.clone()),
        ..sts::NewSession::new(
            sts::Identity {
                user_id: sts::role_id(&role_arn),
                account: account.to_string(),
                arn: Principal::Session.arn(),
            },
            sts::SessionKind::AssumedRole,
            SESSION_SECONDS,
        )
    };
    let sts_store = sts::service_state(&**emulator.engine()).expect("the STS store");
    sts_store
        .lock()
        .insert(vend.into_session(SESSION_KEY_ID, now))
        .expect("the session is recorded");

    let iam_store = iam::service_state(&**emulator.engine()).expect("the IAM store");
    let mut store = iam_store.lock();
    let user_id = store.next_user_id();
    let user_key = iam::name_key(USER);
    store.users.insert(
        user_key.clone(),
        User {
            path: iam::DEFAULT_PATH.to_string(),
            user_name: USER.to_string(),
            user_id,
            arn: iam::user_arn(iam::DEFAULT_PATH, USER),
            create_date: ::aws_smithy_types::DateTime::from(now),
            password_last_used: None,
            permissions_boundary: None,
            tags: None,
        },
    );
    store.access_keys.insert(
        USER_KEY_ID.to_string(),
        iam::AccessKeyRecord {
            user_key,
            status: iam::ACCESS_KEY_ACTIVE_STATUS.to_string(),
            create_date: ::aws_smithy_types::DateTime::from(now),
        },
    );
}

/// Put `document` on the principal as its only inline identity policy — what
/// `iam:PutRolePolicy` and `iam:PutUserPolicy` do — or take every policy off it when
/// `None`, which `iam:DeleteRolePolicy` and `iam:DeleteUserPolicy` do.
///
/// `pub(crate)` for the reason [`Principal`] is.
pub(crate) fn given_policy(emulator: &Emulator, principal: Principal, document: Option<&Value>) {
    let iam_store = iam::service_state(&**emulator.engine()).expect("the IAM store");
    let mut store = iam_store.lock();
    let (policies, key) = match principal {
        Principal::Session => (&mut store.role_inline_policies, iam::name_key(ROLE)),
        Principal::User => (&mut store.user_inline_policies, iam::name_key(USER)),
    };
    match document {
        Some(document) => {
            policies.insert(
                key,
                BTreeMap::from([(
                    iam::name_key(POLICY_NAME),
                    iam::InlinePolicy {
                        policy_name: POLICY_NAME.to_string(),
                        policy_document: document.to_string(),
                    },
                )]),
            );
        }
        None => {
            policies.remove(&key);
        }
    }
}

/// Make the user a member of [`GROUP`] and put `document` on that group — what
/// `iam:AddUserToGroup` and `iam:PutGroupPolicy` do.
fn given_group_policy(emulator: &Emulator, document: &Value) {
    let iam_store = iam::service_state(&**emulator.engine()).expect("the IAM store");
    let mut store = iam_store.lock();
    let group_key = iam::name_key(GROUP);
    store
        .group_members
        .insert(group_key.clone(), BTreeSet::from([iam::name_key(USER)]));
    store.group_inline_policies.insert(
        group_key,
        BTreeMap::from([(
            iam::name_key(POLICY_NAME),
            iam::InlinePolicy {
                policy_name: POLICY_NAME.to_string(),
                policy_document: document.to_string(),
            },
        )]),
    );
}

/// A policy allowing `actions` on `resources`.
fn allow(actions: Value, resources: Value) -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{ "Effect": "Allow", "Action": actions, "Resource": resources }]
    })
}

/// The data source ARNs the application `id` holds an attachment for, read as the
/// account's own identity so the read is never the call under test.
async fn attached_arns(emulator: &Emulator, id: &str, case: &str) -> Vec<String> {
    let answer = list_attachments(emulator, id, json!({})).await;
    assert_eq!(
        answer.status, 200,
        "case {case}: the privileged listing must be served, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body["attachments"]
        .as_array()
        .map(|entries| {
            entries
                .iter()
                .map(|entry| entry["dataSourceArn"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// One application with a domain of this account attached to it, and both principals
/// recorded — the precondition each case below starts from.
struct Fixture {
    emulator: Emulator,
    /// The application every call addresses.
    id: String,
    /// The ARN of the attached data source: what a refused detach or read names.
    attached: String,
}

impl Fixture {
    async fn new(name: &str) -> Self {
        let emulator = Emulator::new();
        given_principals(&emulator);
        let id = application(&emulator, name).await;
        created_domain(&emulator, DOMAIN).await;
        let attached = format!(
            "arn:aws:es:{REGION}:{}:domain/{DOMAIN}",
            ::engine::emulator_account_id()
        );
        tokio::time::sleep(SETTLE_WAIT).await;
        let answer = attach(&emulator, &id, json!({ "dataSourceArn": attached })).await;
        assert_eq!(
            answer.status, 200,
            "case {name:?}: the fixture's privileged attach must be served, got {} with {:?}",
            answer.status, answer.body
        );
        Self {
            emulator,
            id,
            attached,
        }
    }

    /// The data source an operation addresses: one the application does NOT hold for the
    /// attach (so a served call would add a record) and the attached one for the
    /// operations that address an existing attachment (so a served call would read or
    /// remove it).
    fn target(&self, operation: &str) -> String {
        if operation == "AttachDataSource" {
            format!(
                "arn:aws:glue:{REGION}:{}:catalog",
                ::engine::emulator_account_id()
            )
        } else {
            self.attached.clone()
        }
    }
}

/// RULE: every operation on an application's data source attachments takes the decision —
/// not only the one the attach is made through — so a principal no identity-based policy
/// permits is refused with 403 on each of them, in IAM's own wording, and the refused
/// call leaves the application's attachment set exactly as it was. Both principal forms
/// this emulator reads policies for are driven through it.
#[tokio::test]
async fn every_attachment_operation_refuses_a_principal_no_policy_permits() {
    for principal in [Principal::Session, Principal::User] {
        let fixture = Fixture::new("attachauthz").await;
        given_policy(&fixture.emulator, principal, None);
        let before = attached_arns(&fixture.emulator, &fixture.id, principal.case()).await;

        for operation in ATTACHMENT_OPERATIONS {
            let case = format!("{} calling {operation}", principal.case());
            let answer = attachment_call(
                &fixture.emulator,
                principal.access_key_id(),
                operation,
                &fixture.id,
                &fixture.target(operation),
            )
            .await;
            assert_refused(
                &answer,
                &case,
                &principal.arn(),
                &format!("es:{operation}"),
                &every_domain_arn(),
                Refusal::DefaultDeny,
            );
            assert_eq!(
                attached_arns(&fixture.emulator, &fixture.id, &case).await,
                before,
                "case {case}: the refused call must leave the attachment set untouched"
            );
        }
    }
}

/// RULE: the decision follows the identity policies in force on the calling principal in
/// IAM's own order — an `Allow` matching the action AND the resource permits, an explicit
/// `Deny` wins whichever side of an `Allow` it is written on, and nothing matching is
/// IAM's default deny — and it is the same decision for either principal form. The rows
/// differ along the action pattern, the resource a statement is scoped to and the
/// statement's effect.
#[tokio::test]
async fn an_attach_is_decided_by_the_policies_in_force_on_the_principal() {
    let account = ::engine::emulator_account_id();
    // Each row: the case, the policy the principal carries (`None` takes every policy off
    // it), and the expected HTTP status of the attach.
    let rows: Vec<(&str, Option<Value>, u16)> = vec![
        ("no policy at all is IAM's default deny", None, 403),
        (
            "the exact action on the domain resource of this account",
            Some(allow(
                json!("es:AttachDataSource"),
                json!(every_domain_arn()),
            )),
            200,
        ),
        (
            "every action of this service on every resource",
            Some(allow(json!("es:*"), json!("*"))),
            200,
        ),
        (
            "an action pattern matching this one by prefix",
            Some(allow(json!("es:Attach*"), json!("*"))),
            200,
        ),
        (
            "only another operation on the same attachments",
            Some(allow(json!("es:DetachDataSource"), json!("*"))),
            403,
        ),
        (
            "another service's actions",
            Some(allow(json!("glue:GetDatabase"), json!("*"))),
            403,
        ),
        (
            "an Allow scoped to one domain rather than the account's domains",
            Some(allow(
                json!("es:AttachDataSource"),
                json!(format!("arn:aws:es:{REGION}:{account}:domain/{DOMAIN}")),
            )),
            403,
        ),
        (
            "an explicit deny of this action beside an allow of everything",
            Some(json!({
                "Statement": [
                    { "Effect": "Allow", "Action": "es:*", "Resource": "*" },
                    { "Effect": "Deny", "Action": "es:AttachDataSource", "Resource": "*" }
                ]
            })),
            403,
        ),
    ];

    for principal in [Principal::Session, Principal::User] {
        let fixture = Fixture::new("attachdecide").await;
        for (index, (case, policy, expected)) in rows.iter().enumerate() {
            let case = format!("{} with {case}", principal.case());
            given_policy(&fixture.emulator, principal, policy.as_ref());
            // Each row attaches a data source of its own, so what a row is answered
            // depends on its policy rather than on what an earlier row attached.
            let data_source_arn = format!("arn:aws:glue:{REGION}:{account}:catalog/decide{index}");
            let answer = attachment_call(
                &fixture.emulator,
                principal.access_key_id(),
                "AttachDataSource",
                &fixture.id,
                &data_source_arn,
            )
            .await;
            assert_eq!(
                answer.status, *expected,
                "case {case}: expected HTTP {expected}, got {} with {:?}",
                answer.status, answer.body
            );
            let held = attached_arns(&fixture.emulator, &fixture.id, &case).await;
            assert_eq!(
                held.contains(&data_source_arn),
                *expected == 200,
                "case {case}: a served attach records the data source and a refused one \
                 records nothing, got {held:?}"
            );
        }
    }
}

/// RULE: a user also holds the permissions of the groups it belongs to, so a grant
/// written on the GROUP decides its request exactly as one written on the user itself
/// does — and an explicit deny on the group refuses it just the same.
#[tokio::test]
async fn a_grant_on_the_users_group_decides_its_request() {
    // Each row: the case, the policy the GROUP carries, and the expected HTTP status.
    let rows: Vec<(&str, Value, u16)> = vec![
        (
            "a group that grants this action",
            allow(json!("es:AttachDataSource"), json!("*")),
            200,
        ),
        (
            "a group that grants another action only",
            allow(json!("es:GetApplication"), json!("*")),
            403,
        ),
        (
            "a group that denies this action explicitly",
            json!({
                "Statement": [{ "Effect": "Deny", "Action": "es:AttachDataSource", "Resource": "*" }]
            }),
            403,
        ),
    ];

    for (index, (case, document, expected)) in rows.iter().enumerate() {
        let fixture = Fixture::new("attachgroup").await;
        // The user itself carries nothing, so whatever decides the request came from the
        // group.
        given_policy(&fixture.emulator, Principal::User, None);
        given_group_policy(&fixture.emulator, document);
        let data_source_arn = format!(
            "arn:aws:glue:{REGION}:{}:catalog/group{index}",
            ::engine::emulator_account_id()
        );
        let answer = attachment_call(
            &fixture.emulator,
            Principal::User.access_key_id(),
            "AttachDataSource",
            &fixture.id,
            &data_source_arn,
        )
        .await;
        assert_eq!(
            answer.status, *expected,
            "case {case}: expected HTTP {expected}, got {} with {:?}",
            answer.status, answer.body
        );
    }
}

/// RULE: a request whose access key id no IAM user holds and no session was minted for
/// speaks for the account's own identity — the long-term key this emulator publishes — so
/// it is served whatever the account's roles and users are denied; the bounded
/// principals' identical requests are refused, so the callers really are different.
#[tokio::test]
async fn a_request_that_names_no_bounded_principal_is_served_as_the_accounts_own_identity() {
    let fixture = Fixture::new("attachambient").await;
    let denied = json!({
        "Statement": [{ "Effect": "Deny", "Action": "es:*", "Resource": "*" }]
    });
    given_policy(&fixture.emulator, Principal::Session, Some(&denied));
    given_policy(&fixture.emulator, Principal::User, Some(&denied));

    for principal in [Principal::Session, Principal::User] {
        let answer = attachment_call(
            &fixture.emulator,
            principal.access_key_id(),
            "ListDataSourceAttachments",
            &fixture.id,
            &fixture.attached,
        )
        .await;
        assert_refused(
            &answer,
            principal.case(),
            &principal.arn(),
            "es:ListDataSourceAttachments",
            &every_domain_arn(),
            Refusal::ExplicitDeny,
        );
    }

    // …which says nothing about a request that speaks for neither of them: it reads the
    // set as before.
    assert_eq!(
        attached_arns(&fixture.emulator, &fixture.id, "the account's own identity").await,
        vec![fixture.attached.clone()],
        "case the account's own identity: the privileged caller still reads the set"
    );
}
