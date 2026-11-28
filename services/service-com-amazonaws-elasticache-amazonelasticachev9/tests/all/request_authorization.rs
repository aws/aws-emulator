// SPDX-License-Identifier: CC0-1.0

//! WHO may perform an ElastiCache action at all, driven end to end over the real wire path (an
//! awsQuery form-urlencoded request through the multi-service router into the mounted service),
//! so what is asserted is what a client holding those credentials sees.
//!
//! THE RULE UNDER TEST: every request to this service is authorized BEFORE the operation acts,
//! against the documents in force on the principal whose credential signed it — the
//! identity-based policies of the IAM identity the credential names (the ROLE behind an
//! assumed-role session, the USER behind a long-term access key), capped by the permissions
//! boundary on that identity and by the session policies the mint of the credential passed —
//! evaluated as `elasticache:<OperationName>` on the resource the request ADDRESSES. A call no
//! statement allows (IAM's default deny), or one an explicit `Deny` covers, is answered HTTP
//! 403 `AccessDenied` and changes NOTHING: the resource it named still describes afterwards. A
//! call a matching `Allow` covers reaches the operation's own answer.
//!
//! The role, the user, their policies and the session live in the IAM and STS stores of the
//! SAME shared engine — the stores those services' own handlers write — so the documents under
//! test are the ones `iam:PutRolePolicy` stores and the principal a refusal blames is the one
//! `sts:GetCallerIdentity` answers for the key the request signed with.
//!
//! The recorded flow this was derived from calls `DeleteCacheSubnetGroup` with the credentials
//! of an agent role whose single statement allows creates and `Describe*` and no `Delete` at
//! all, and is one row of the tables below. The rows differ along what the rule generalises
//! over: the OPERATION (four deletes, a modify, two creates, a read, a tagging call), the
//! DOCUMENT in force (none at all, another service's actions, the creates alone, the delete
//! itself, the whole namespace, an explicit deny beside an allow of everything), the RESOURCE a
//! statement is scoped to (this resource, another of the same kind, the same name in another
//! Region, every resource), the PRINCIPAL (an assumed-role session, an IAM user's long-term
//! key, the account's own published credential) and the CEILING on the grant (a permissions
//! boundary, a session policy).

use std::sync::Arc;
use std::time::SystemTime;

use service_com_amazonaws_elasticache_amazonelasticachev9::state;
use service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam;
use service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts;

use crate::cache_cluster_lifecycle::{account_subnet_ids, subnet_group_params};
use crate::harness::{params, Emulator, Reply, ACCOUNT_KEY_ID, REGION};

/// The canonical emulator account id, spelled out so the suite pins the ARN a client sees.
const ACCOUNT: &str = "000000000000";

/// The access key id of the assumed-role session the role rows sign with.
const SESSION_KEY_ID: &str = "ASIAEXAMPLEELASTICACHE1";

/// The access key id of the IAM user's long-term key the user rows sign with.
const USER_KEY_ID: &str = "AKIAEXAMPLEELASTICACHE1";

/// The role the session is assumed from, and the session's own name.
const ROLE: &str = "vvd-agent-role";
const SESSION: &str = "vvd-agent";

/// The IAM user the long-term key belongs to.
const USER_NAME: &str = "cache-operator";

/// The name of the inline policy the role and the user carry.
const POLICY_NAME: &str = "inline";

/// How long a seeded session lasts: an hour, STS's own default for a role session
/// (`AssumeRoleRequest$DurationSeconds`, "Default: 3600 seconds").
const SESSION_SECONDS: i32 = 3_600;

/// The error code and the status an authorization refusal of this service carries.
const ACCESS_DENIED: &str = "AccessDenied";
const ACCESS_DENIED_STATUS: u16 = 403;

/// The document the agent role under test carries in the recorded flow: creates, tagging and
/// `Describe*`, and no `Delete` action of any kind.
const AGENT_GRANTS: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow",
    "Action":["elasticache:CreateCacheSubnetGroup","elasticache:CreateReplicationGroup",
    "elasticache:AddTagsToResource","elasticache:Describe*"],"Resource":"*"}]}"#;

/// The principal ARN the seeded session speaks for — what a refusal must name.
fn session_arn() -> String {
    format!("arn:aws:sts::{ACCOUNT}:assumed-role/{ROLE}/{SESSION}")
}

/// The ARN the account holds the role under, which is the identity whose policies an
/// assumed-role session's request is judged on.
fn role_arn() -> String {
    format!("arn:aws:iam::{ACCOUNT}:role/{ROLE}")
}

/// The IAM user's ARN, as IAM holds it and `sts:GetCallerIdentity` reports it.
fn user_arn() -> String {
    format!("arn:aws:iam::{ACCOUNT}:user/{USER_NAME}")
}

/// An `Allow` of `actions` on `resource`, as a policy document.
fn allow_on(actions: &str, resource: &str) -> String {
    format!(
        r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"Allow","Action":{actions},
        "Resource":{resource}}}]}}"#
    )
}

/// One inline policy holding `document`, as `iam:PutRolePolicy` / `iam:PutUserPolicy` store one.
fn inline(document: &str) -> std::collections::BTreeMap<String, iam::InlinePolicy> {
    std::collections::BTreeMap::from([(
        iam::name_key(POLICY_NAME),
        iam::InlinePolicy {
            policy_name: POLICY_NAME.to_string(),
            policy_document: document.to_string(),
        },
    )])
}

/// Record the role [`SESSION_KEY_ID`]'s session is of, as `iam:CreateRole` records it, and the
/// session itself, as `sts:AssumeRole` records one — scoped by `session_policy` when the mint
/// passed one. NO identity policy is attached.
fn given_session(engine: &Arc<engine::InMemoryEngine>, session_policy: Option<&str>) {
    let role_arn = role_arn();
    let iam_store = iam::service_state(&**engine).expect("the IAM store");
    iam_store.lock().roles.insert(
        iam::name_key(ROLE),
        ::service_com_amazonaws_iam_awsidentitymanagementv20100508::ops::types::Role {
            path: "/".to_string(),
            role_name: ROLE.to_string(),
            role_id: "AROAEXAMPLEELASTICACHE".to_string(),
            arn: role_arn.clone(),
            create_date: ::aws_smithy_types::DateTime::from_secs(0),
            assume_role_policy_document: None,
            description: None,
            max_session_duration: None,
            permissions_boundary: None,
            tags: None,
            role_last_used: None,
            source_role_template: None,
        },
    );
    let vend = sts::NewSession {
        role_arn: Some(role_arn.clone()),
        policies: sts::SessionPolicies {
            inline: session_policy.map(str::to_string),
            managed_arns: Vec::new(),
            none_means_no_permissions: false,
            ..sts::SessionPolicies::default()
        },
        ..sts::NewSession::new(
            sts::Identity {
                user_id: sts::role_id(&role_arn),
                account: ACCOUNT.to_string(),
                arn: session_arn(),
            },
            sts::SessionKind::AssumedRole,
            SESSION_SECONDS,
        )
    };
    let store = sts::service_state(&**engine).expect("the STS store");
    store
        .lock()
        .insert(vend.into_session(SESSION_KEY_ID, SystemTime::now()))
        .expect("the session is recorded");
}

/// Record the IAM user [`USER_KEY_ID`] belongs to, as `iam:CreateUser` and
/// `iam:CreateAccessKey` record one. NO policy is attached.
fn given_user_key(engine: &Arc<engine::InMemoryEngine>) {
    let minted = ::aws_smithy_types::DateTime::from_secs(0);
    let store = iam::service_state(&**engine).expect("the IAM store");
    let mut store = store.lock();
    store.users.insert(
        iam::name_key(USER_NAME),
        ::service_com_amazonaws_iam_awsidentitymanagementv20100508::ops::types::User {
            path: "/".to_string(),
            user_name: USER_NAME.to_string(),
            user_id: "AIDAEXAMPLEELASTICACHE".to_string(),
            arn: user_arn(),
            create_date: minted,
            password_last_used: None,
            permissions_boundary: None,
            tags: None,
        },
    );
    store.access_keys.insert(
        USER_KEY_ID.to_string(),
        iam::AccessKeyRecord {
            user_key: iam::name_key(USER_NAME),
            status: iam::ACCESS_KEY_ACTIVE_STATUS.to_string(),
            create_date: minted,
        },
    );
}

/// Attach `document` to the role as its inline policy, replacing whatever it carried. `None`
/// takes every policy off it, as `iam:DeleteRolePolicy` does.
fn given_role_policy(engine: &Arc<engine::InMemoryEngine>, document: Option<&str>) {
    let store = iam::service_state(&**engine).expect("the IAM store");
    let mut store = store.lock();
    store.role_inline_policies.remove(&iam::name_key(ROLE));
    if let Some(document) = document {
        store
            .role_inline_policies
            .insert(iam::name_key(ROLE), inline(document));
    }
}

/// The same for the IAM user, as `iam:PutUserPolicy` / `iam:DeleteUserPolicy` do.
fn given_user_policy(engine: &Arc<engine::InMemoryEngine>, document: Option<&str>) {
    let store = iam::service_state(&**engine).expect("the IAM store");
    let mut store = store.lock();
    store.user_inline_policies.remove(&iam::name_key(USER_NAME));
    if let Some(document) = document {
        store
            .user_inline_policies
            .insert(iam::name_key(USER_NAME), inline(document));
    }
}

/// Attach `document` to the role as the managed policy that caps it — a permissions boundary,
/// which "sets the maximum permissions that an identity-based policy can grant".
fn given_role_boundary(engine: &Arc<engine::InMemoryEngine>, document: &str) {
    use ::service_com_amazonaws_iam_awsidentitymanagementv20100508::ops::types::{
        AttachedPermissionsBoundary, Policy, PolicyVersion,
    };

    let boundary_name = "cache-ceiling";
    let policy_arn = iam::policy_arn("/", boundary_name);
    let store = iam::service_state(&**engine).expect("the IAM store");
    let mut store = store.lock();
    store.policies.insert(
        iam::name_key(boundary_name),
        iam::ManagedPolicy {
            policy: Policy {
                policy_name: Some(boundary_name.to_string()),
                policy_id: Some("ANPAEXAMPLEELASTICACHE".to_string()),
                arn: Some(policy_arn.clone()),
                path: Some("/".to_string()),
                default_version_id: Some(iam::FIRST_POLICY_VERSION_ID.to_string()),
                is_attachable: true,
                ..Policy::default()
            },
            versions: [(
                iam::FIRST_POLICY_VERSION_ID.to_string(),
                PolicyVersion {
                    document: Some(document.to_string()),
                    version_id: Some(iam::FIRST_POLICY_VERSION_ID.to_string()),
                    is_default_version: true,
                    ..PolicyVersion::default()
                },
            )]
            .into_iter()
            .collect(),
        },
    );
    if let Some(role) = store.roles.get_mut(&iam::name_key(ROLE)) {
        role.permissions_boundary = Some(AttachedPermissionsBoundary {
            permissions_boundary_type: Some("Policy".to_string()),
            permissions_boundary_arn: Some(policy_arn),
        });
    }
}

/// An emulator with ElastiCache, EC2, IAM and STS on one engine, carrying the role, the session
/// and the IAM user the rows sign with.
async fn given_emulator() -> Emulator {
    let emulator = Emulator::with_identity();
    given_session(emulator.engine(), None);
    given_user_key(emulator.engine());
    emulator
}

/// Create the cache subnet group `name` over every subnet the ACCOUNT has, as the account's own
/// published credential — the owner, whose calls this gate does not bound.
async fn given_group(emulator: &Emulator, name: &str) {
    let subnets = account_subnet_ids(emulator).await;
    let reply = emulator
        .call(
            "CreateCacheSubnetGroup",
            &subnet_group_params(name, "the estate under test", &subnets),
        )
        .await;
    assert_eq!(
        reply.status, 200,
        "the owner must be able to create {name}, got {} {}",
        reply.status, reply.body
    );
}

/// Call `action` as the assumed-role session.
async fn as_agent(emulator: &Emulator, action: &str, pairs: &[(String, String)]) -> Reply {
    emulator
        .call_signed_by(SESSION_KEY_ID, REGION, action, pairs)
        .await
}

/// Whether the group `name` still describes — what makes a refusal "and nothing changed".
async fn group_describes(emulator: &Emulator, name: &str) -> bool {
    let reply = emulator
        .call(
            "DescribeCacheSubnetGroups",
            &params(&[("CacheSubnetGroupName", name)]),
        )
        .await;
    reply.status == 200
        && reply
            .all("CacheSubnetGroups/CacheSubnetGroup/CacheSubnetGroupName")
            .contains(&name)
}

/// The sentence IAM refuses an unauthorized request with, for a call no document allows.
fn no_grant_message(principal: &str, action: &str, resource: &str) -> String {
    format!(
        "User: {principal} is not authorized to perform: {action} on resource: {resource} \
         because no identity-based policy allows the {action} action"
    )
}

/// Assert `reply` is this service's authorization refusal carrying `message`.
fn assert_refused(case: &str, reply: &Reply, message: &str) {
    assert_eq!(
        (reply.status, reply.error_code()),
        (ACCESS_DENIED_STATUS, ACCESS_DENIED),
        "case {case:?}: expected {ACCESS_DENIED_STATUS} {ACCESS_DENIED}, got {} {}",
        reply.status,
        reply.body
    );
    assert_eq!(
        reply.error_message(),
        message,
        "case {case:?}: expected IAM's sentence, got {:?}",
        reply.error_message()
    );
}

/// One row of [`a_role_granted_no_delete_action_performs_none`]: a call the agent makes, and
/// what the gate must answer it.
struct ActionCase {
    /// What the row is about.
    case: &'static str,
    /// The operation the agent calls.
    action: &'static str,
    /// The request's own form pairs.
    pairs: Vec<(String, String)>,
    /// The resource ARN the decision is taken against, which a refusal must name.
    resource: String,
    /// Whether the document in force allows the call.
    allowed: bool,
}

/// RULE: the action the gate decides is `elasticache:<OperationName>` on the resource the
/// request addresses, so a role granted the creates and `Describe*` alone performs those and no
/// other action of this service — and each refused call leaves the estate exactly as it was.
#[tokio::test]
async fn a_role_granted_no_delete_action_performs_none() {
    let emulator = given_emulator().await;
    given_role_policy(emulator.engine(), Some(AGENT_GRANTS));
    given_group(&emulator, "vvd-cache-subnets").await;
    let subnets = account_subnet_ids(&emulator).await;

    let rows: Vec<ActionCase> = vec![
        ActionCase {
            case: "the delete the recorded flow makes",
            action: "DeleteCacheSubnetGroup",
            pairs: params(&[("CacheSubnetGroupName", "vvd-cache-subnets")]),
            resource: state::subnet_group_arn(REGION, "vvd-cache-subnets"),
            allowed: false,
        },
        ActionCase {
            case: "a delete of another kind of resource",
            action: "DeleteCacheParameterGroup",
            pairs: params(&[("CacheParameterGroupName", "vvd-parameters")]),
            resource: state::parameter_group_arn(REGION, "vvd-parameters"),
            allowed: false,
        },
        ActionCase {
            case: "a delete of a snapshot",
            action: "DeleteSnapshot",
            pairs: params(&[("SnapshotName", "vvd-snapshot")]),
            resource: state::snapshot_arn(REGION, "vvd-snapshot"),
            allowed: false,
        },
        ActionCase {
            case: "a delete of a replication group",
            action: "DeleteReplicationGroup",
            pairs: params(&[("ReplicationGroupId", "vvd-valkey")]),
            resource: state::replication_group_arn(REGION, "vvd-valkey"),
            allowed: false,
        },
        ActionCase {
            case: "a modify, which is no Delete and still no grant",
            action: "ModifyCacheSubnetGroup",
            pairs: params(&[
                ("CacheSubnetGroupName", "vvd-cache-subnets"),
                ("CacheSubnetGroupDescription", "renamed by the agent"),
            ]),
            resource: state::subnet_group_arn(REGION, "vvd-cache-subnets"),
            allowed: false,
        },
        ActionCase {
            case: "a create of a kind the document does not name",
            action: "CreateCacheParameterGroup",
            pairs: params(&[
                ("CacheParameterGroupName", "vvd-parameters"),
                ("CacheParameterGroupFamily", "redis7"),
                ("Description", "the agent's own"),
            ]),
            resource: state::parameter_group_arn(REGION, "vvd-parameters"),
            allowed: false,
        },
        ActionCase {
            case: "the create the document names",
            action: "CreateCacheSubnetGroup",
            pairs: subnet_group_params("vvd-agent-made", "made by the agent", &subnets),
            resource: state::subnet_group_arn(REGION, "vvd-agent-made"),
            allowed: true,
        },
        ActionCase {
            case: "the read the document names",
            action: "DescribeCacheSubnetGroups",
            pairs: params(&[("CacheSubnetGroupName", "vvd-cache-subnets")]),
            resource: state::subnet_group_arn(REGION, "vvd-cache-subnets"),
            allowed: true,
        },
        ActionCase {
            case: "the tagging call the document names, addressed by ARN",
            action: "AddTagsToResource",
            pairs: params(&[
                (
                    "ResourceName",
                    state::subnet_group_arn(REGION, "vvd-cache-subnets").as_str(),
                ),
                ("Tags.Tag.1.Key", "owner"),
                ("Tags.Tag.1.Value", "vvd"),
            ]),
            resource: state::subnet_group_arn(REGION, "vvd-cache-subnets"),
            allowed: true,
        },
    ];

    for row in rows {
        let reply = as_agent(&emulator, row.action, &row.pairs).await;
        if row.allowed {
            assert_eq!(
                reply.status,
                200,
                "case {:?}: expected {} to be served, got {} {}",
                row.case,
                row.action,
                reply.status,
                reply.body
            );
        } else {
            assert_refused(
                row.case,
                &reply,
                &no_grant_message(
                    &session_arn(),
                    &format!("elasticache:{}", row.action),
                    &row.resource,
                ),
            );
        }
    }

    // Nothing a refusal touched: the group the deletes and the modify named is still there, with
    // the description it was created under.
    let described = emulator
        .call(
            "DescribeCacheSubnetGroups",
            &params(&[("CacheSubnetGroupName", "vvd-cache-subnets")]),
        )
        .await;
    assert_eq!(
        described.text("CacheSubnetGroups/CacheSubnetGroup/CacheSubnetGroupDescription"),
        Some("the estate under test"),
        "a refused call must change nothing, got {}",
        described.body
    );
    // ... and the parameter group the refused create named was never stored.
    let groups = emulator
        .call(
            "DescribeCacheParameterGroups",
            &params(&[("CacheParameterGroupName", "vvd-parameters")]),
        )
        .await;
    assert_ne!(
        groups.status, 200,
        "a refused create must store nothing, got {}",
        groups.body
    );
}

/// RULE: a grant is read for the resource the request ADDRESSES, so a statement scoped to one
/// resource covers that one and no other — and the call it does cover goes through to the
/// operation, which performs it.
#[tokio::test]
async fn a_grant_is_read_for_the_resource_the_request_addresses() {
    let emulator = given_emulator().await;
    let action = "elasticache:DeleteCacheSubnetGroup";
    // (case, the resource the statement is scoped to, whether it covers the delete)
    let rows: Vec<(&str, String, bool)> = vec![
        (
            "the group the request names",
            state::subnet_group_arn(REGION, "vvd-scoped-1"),
            true,
        ),
        (
            "another group of the same account and Region",
            state::subnet_group_arn(REGION, "vvd-elsewhere"),
            false,
        ),
        (
            "the same group name in another Region",
            state::subnet_group_arn("eu-west-3", "vvd-scoped-3"),
            false,
        ),
        (
            "a cluster of the same name, which is another kind of resource",
            state::cluster_arn(REGION, "vvd-scoped-4"),
            false,
        ),
        ("every resource", "*".to_string(), true),
    ];
    for (index, (case, scoped_to, covered)) in rows.into_iter().enumerate() {
        let group = format!("vvd-scoped-{}", index + 1);
        given_group(&emulator, &group).await;
        given_role_policy(
            emulator.engine(),
            Some(&allow_on(
                &format!("\"{action}\""),
                &format!("\"{scoped_to}\""),
            )),
        );
        let reply = as_agent(
            &emulator,
            "DeleteCacheSubnetGroup",
            &params(&[("CacheSubnetGroupName", group.as_str())]),
        )
        .await;
        if covered {
            assert_eq!(
                reply.status, 200,
                "case {case:?}: expected the delete to be served, got {} {}",
                reply.status, reply.body
            );
            assert!(
                !group_describes(&emulator, &group).await,
                "case {case:?}: expected {group} gone once the delete was served"
            );
        } else {
            assert_refused(
                case,
                &reply,
                &no_grant_message(
                    &session_arn(),
                    action,
                    &state::subnet_group_arn(REGION, &group),
                ),
            );
            assert!(
                group_describes(&emulator, &group).await,
                "case {case:?}: expected {group} untouched by a refused delete"
            );
        }
    }
}

/// RULE: the principal the decision is taken for — and the one a refusal names — is the one STS
/// reports for the key the request signed with: the role behind an assumed-role session, the
/// user behind a long-term key, and the account's own published credential, which no
/// identity-based policy bounds.
#[tokio::test]
async fn the_principal_decided_is_the_one_sts_reports_for_the_key() {
    let emulator = given_emulator().await;
    given_group(&emulator, "vvd-principals").await;
    let action = "elasticache:DescribeCacheSubnetGroups";
    let resource = state::subnet_group_arn(REGION, "vvd-principals");
    let read = params(&[("CacheSubnetGroupName", "vvd-principals")]);

    // The account's own published credential: no IAM record holds it, so nothing bounds it.
    let owner = emulator
        .call_signed_by(ACCOUNT_KEY_ID, REGION, "DescribeCacheSubnetGroups", &read)
        .await;
    assert_eq!(
        owner.status, 200,
        "case \"the account's own credential\": expected the read to be served, got {} {}",
        owner.status, owner.body
    );

    // The session, with nothing granted to the role it is a session of.
    given_role_policy(emulator.engine(), None);
    let refused = as_agent(&emulator, "DescribeCacheSubnetGroups", &read).await;
    assert_refused(
        "the session, with no document on its role",
        &refused,
        &no_grant_message(&session_arn(), action, &resource),
    );

    // The same session once the ROLE carries the grant: the session is judged on the role's
    // documents.
    given_role_policy(
        emulator.engine(),
        Some(&allow_on(&format!("\"{action}\""), "\"*\"")),
    );
    let served = as_agent(&emulator, "DescribeCacheSubnetGroups", &read).await;
    assert_eq!(
        served.status, 200,
        "case \"the session, with the grant on its role\": expected the read to be served, got \
         {} {}",
        served.status, served.body
    );

    // The IAM user's long-term key, judged on the USER's own documents.
    given_user_policy(emulator.engine(), None);
    let refused = emulator
        .call_signed_by(USER_KEY_ID, REGION, "DescribeCacheSubnetGroups", &read)
        .await;
    assert_refused(
        "the user's long-term key, with no document",
        &refused,
        &no_grant_message(&user_arn(), action, &resource),
    );
    given_user_policy(
        emulator.engine(),
        Some(&allow_on("\"elasticache:*\"", "\"*\"")),
    );
    let served = emulator
        .call_signed_by(USER_KEY_ID, REGION, "DescribeCacheSubnetGroups", &read)
        .await;
    assert_eq!(
        served.status, 200,
        "case \"the user's long-term key, with the grant\": expected the read to be served, got \
         {} {}",
        served.status, served.body
    );
}

/// One row of [`a_deny_and_a_ceiling_withhold_what_a_grant_allows`]: the documents in force on
/// the agent, and the clause the refusal must close with.
struct CeilingCase {
    /// What the row is about.
    case: &'static str,
    /// The role's own identity-based document.
    document: String,
    /// The permissions boundary attached to the role, when the row attaches one.
    boundary: Option<String>,
    /// The inline session policy the credential was minted with, when the row passes one.
    scope: Option<String>,
    /// The clause the refusal closes with, or `None` where the delete is served.
    clause: Option<String>,
}

/// RULE: an explicit `Deny` refuses whatever a grant says, and a grant is capped by the
/// permissions boundary on the identity and by the session policies the credential was minted
/// with — each refusal naming the document that withheld it, because they need different fixes.
#[tokio::test]
async fn a_deny_and_a_ceiling_withhold_what_a_grant_allows() {
    let action = "elasticache:DeleteCacheSubnetGroup";
    let deny_beside_allow = r#"{"Version":"2012-10-17","Statement":[
        {"Effect":"Allow","Action":"elasticache:*","Resource":"*"},
        {"Effect":"Deny","Action":"elasticache:DeleteCacheSubnetGroup","Resource":"*"}]}"#;
    let describes_only = allow_on("\"elasticache:Describe*\"", "\"*\"");

    let rows: Vec<CeilingCase> = vec![
        CeilingCase {
            case: "an allow of everything, with nothing capping it",
            document: allow_on("\"elasticache:*\"", "\"*\""),
            boundary: None,
            scope: None,
            clause: None,
        },
        CeilingCase {
            case: "an explicit deny beside that allow",
            document: deny_beside_allow.to_string(),
            boundary: None,
            scope: None,
            clause: Some("with an explicit deny in an identity-based policy".to_string()),
        },
        CeilingCase {
            case: "a boundary that allows only the reads",
            document: allow_on("\"elasticache:*\"", "\"*\""),
            boundary: Some(describes_only.clone()),
            scope: None,
            clause: Some(format!(
                "because no permissions boundary allows the {action} action"
            )),
        },
        CeilingCase {
            case: "a session policy that allows only the reads",
            document: allow_on("\"elasticache:*\"", "\"*\""),
            boundary: None,
            scope: Some(describes_only.clone()),
            clause: Some(format!(
                "because no session policy allows the {action} action"
            )),
        },
    ];

    for (index, row) in rows.into_iter().enumerate() {
        // One emulator per row: the scope is fixed at the MINT of the credential, so a row that
        // varies it mints its own session.
        let emulator = Emulator::with_identity();
        given_session(emulator.engine(), row.scope.as_deref());
        let group = format!("vvd-ceiling-{}", index + 1);
        given_group(&emulator, &group).await;
        given_role_policy(emulator.engine(), Some(&row.document));
        if let Some(boundary) = &row.boundary {
            given_role_boundary(emulator.engine(), boundary);
        }
        let reply = as_agent(
            &emulator,
            "DeleteCacheSubnetGroup",
            &params(&[("CacheSubnetGroupName", group.as_str())]),
        )
        .await;
        let case = row.case;
        match row.clause {
            None => {
                assert_eq!(
                    reply.status, 200,
                    "case {case:?}: expected the delete to be served, got {} {}",
                    reply.status, reply.body
                );
                assert!(
                    !group_describes(&emulator, &group).await,
                    "case {case:?}: expected {group} gone once the delete was served"
                );
            }
            Some(clause) => {
                assert_refused(
                    case,
                    &reply,
                    &format!(
                        "User: {} is not authorized to perform: {action} on resource: {} {clause}",
                        session_arn(),
                        state::subnet_group_arn(REGION, &group)
                    ),
                );
                assert!(
                    group_describes(&emulator, &group).await,
                    "case {case:?}: expected {group} untouched by a refused delete"
                );
            }
        }
    }
}
