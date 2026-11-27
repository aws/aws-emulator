// SPDX-License-Identifier: CC0-1.0

//! WHO a cluster's Kubernetes API server serves a request as: which of the
//! cluster's two mappings answers for the calling IAM principal, and the USERNAME
//! that mapping resolves to for the credential the request presented.
//!
//! The RULES under test:
//!
//! * the username a cluster authenticates an IAM principal as is its mapping's
//!   own, with the placeholders Amazon EKS resolves per request filled in from
//!   the credential that request presented — so an IAM ROLE, which cannot present
//!   itself, authenticates as the STS ASSUMED-ROLE SESSION it signs with
//!   (`arn:aws:sts::<account>:assumed-role/<role>/<session>`, the principal
//!   `sts:GetCallerIdentity` reports for the same credential) and not as the role
//!   ARN the entry is filed under. That is the name the API server's own refusal
//!   names: `<plural> is forbidden: User "<username>" cannot <verb> resource
//!   "<plural>" in API group "<group>" at the cluster scope`;
//! * a cluster whose `authenticationMode` includes `CONFIG_MAP` authenticates a
//!   principal its `kube-system/aws-auth` ConfigMap maps — as that entry's
//!   `username` and `groups`, so the cluster's RBAC objects bound to those groups
//!   decide what it may do — while an ACCESS ENTRY for the same principal TAKES
//!   PRECEDENCE over the mapping, and deleting that entry falls back to it. A
//!   cluster in `API` mode alone has no ConfigMap mapping at all;
//! * the identity the API server REPORTS about an authenticated request is a
//!   complete Kubernetes `UserInfo`: beside that username and those groups, the
//!   subject `uid` the cluster's authenticating authority minted for the calling
//!   IAM principal — its own name, the account the principal belongs to and that
//!   principal's unique id — and the `extra` the authority reports about it, whose
//!   `principalId` is the very id the uid's last segment carries. Both are facts of
//!   the credential that authenticated the request, so one IAM principal is one
//!   subject under either mapping and however many sessions of it sign requests.
//!
//! The rows differ along what the rules generalise over: the form of the username
//! the mapping carries (the one Amazon EKS specifies, one the caller chose, one
//! carrying each placeholder EKS resolves), which mapping carries it (an access
//! entry, the ConfigMap), what the cluster's authentication mode admits, and
//! whether the caller presents a session or a long-term credential.
//!
//! Every request to the cluster is a real one to the published API server address,
//! made by the kubeconfig-built client the `kubernetes_api` suite dials with, and
//! every session is one STS really minted for a role IAM really holds — all three
//! services over the one shared engine.

use serde_json::json;
use smithy_server_runtime::MultiServiceRouter;

use crate::cluster_lifecycle::minimal_request;
use crate::kubernetes_api::{
    bearer_token, dial, dial_with, kubeconfig_of, poll_until_active, ApiAnswer, Kubeconfig,
};
use crate::wire::{
    call, call_as, emulator_with_identity, encode_label, query, xml_element, Answer,
    DEFAULT_ACCESS_KEY_ID,
};

/// The Region every request here is signed for.
const REGION: &str = "us-west-2";

/// The namespace the ConfigMap-mapped principal lists pods in.
const NAMESPACE: &str = "default";

/// The Kubernetes group the `aws-auth` mapping puts its principal in, and which
/// the cluster's `ClusterRoleBinding` binds to a pod-reading role.
const CONFIG_MAP_GROUP: &str = "aws-auth-readers";

/// The cluster-scoped RBAC objects that make [`CONFIG_MAP_GROUP`] a group that may
/// list pods, created by the cluster's creator through the published address.
fn pod_reader_role() -> serde_json::Value {
    json!({
        "metadata": { "name": "aws-auth-pod-reader" },
        "rules": [{ "apiGroups": [""], "resources": ["pods"], "verbs": ["get", "list"] }],
    })
}

/// See [`pod_reader_role`].
fn pod_reader_binding() -> serde_json::Value {
    json!({
        "metadata": { "name": "aws-auth-pod-readers" },
        "roleRef": {
            "apiGroup": "rbac.authorization.k8s.io",
            "kind": "ClusterRole",
            "name": "aws-auth-pod-reader",
        },
        "subjects": [{ "kind": "Group", "name": CONFIG_MAP_GROUP }],
    })
}

/// One session of one role: the role IAM holds and the credential STS minted for a
/// session of it.
struct RoleSession {
    role_arn: String,
    session_name: String,
    access_key_id: String,
    /// The unique id of the ROLE this is a session of: the part of the session's
    /// `UserId` (`AssumedRoleUser$AssumedRoleId`, "<RoleId>:<RoleSessionName>")
    /// before the session name, which is the id of the IAM principal the
    /// credential speaks for however many sessions of it are minted.
    principal_id: String,
}

impl RoleSession {
    /// The STS principal this session presents, which is what
    /// `sts:GetCallerIdentity` answers for its credential.
    fn session_principal(&self) -> String {
        let role_name = self
            .role_arn
            .rsplit('/')
            .next()
            .expect("a role ARN names a role");
        format!(
            "arn:aws:sts::{}:assumed-role/{role_name}/{}",
            engine::emulator_account_id(),
            self.session_name
        )
    }
}

/// Create the IAM role `role_name` and assume it under `session_name`, answering
/// the session STS minted — a real record in the registry every service of this
/// emulator resolves a temporary credential against.
async fn assumed_role(
    router: &MultiServiceRouter,
    role_name: &str,
    session_name: &str,
) -> RoleSession {
    let account = engine::emulator_account_id();
    let role_arn = format!("arn:aws:iam::{account}:role/{role_name}");
    let trust = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "AWS": format!("arn:aws:iam::{account}:root") },
            "Action": "sts:AssumeRole",
        }],
    })
    .to_string();
    let (status, body) = query(
        router,
        "iam",
        "2010-05-08",
        "CreateRole",
        &[
            ("RoleName", role_name.to_string()),
            ("AssumeRolePolicyDocument", trust),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: CreateRole {role_name:?} must succeed, got {status} {body}"
    );
    another_session(router, &role_arn, session_name).await
}

/// Assume a role IAM ALREADY holds under `session_name`, answering the session STS
/// minted — a SECOND credential of one IAM principal, which is what shows what a
/// cluster reports about the principal rather than about the credential.
async fn another_session(
    router: &MultiServiceRouter,
    role_arn: &str,
    session_name: &str,
) -> RoleSession {
    let (status, body) = query(
        router,
        "sts",
        "2011-06-15",
        "AssumeRole",
        &[
            ("RoleArn", role_arn.to_string()),
            ("RoleSessionName", session_name.to_string()),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: AssumeRole {role_arn:?} must succeed, got {status} {body}"
    );
    let assumed_role_id = xml_element(&body, "AssumedRoleId")
        .unwrap_or_else(|| panic!("setup: AssumeRole must answer an AssumedRoleId, got {body}"));
    RoleSession {
        role_arn: role_arn.to_string(),
        session_name: session_name.to_string(),
        access_key_id: xml_element(&body, "AccessKeyId")
            .unwrap_or_else(|| panic!("setup: AssumeRole must answer an AccessKeyId, got {body}")),
        principal_id: assumed_role_id
            .split_once(':')
            .map_or_else(
                || assumed_role_id.clone(),
                |(role_id, _session)| role_id.to_string(),
            ),
    }
}

/// A cluster created in `mode`, waited out until it publishes its API server
/// address, with the kubeconfig members a client builds its connection from.
async fn active_cluster(router: &MultiServiceRouter, name: &str, mode: &str) -> Kubeconfig {
    let mut body = minimal_request(name);
    body["accessConfig"] = json!({ "authenticationMode": mode });
    let created = call(router, REGION, "POST", "/clusters", &body.to_string()).await;
    assert_eq!(
        created.status, 200,
        "setup: the cluster {name:?} must be created, got {created:?}"
    );
    let described = poll_until_active(router, REGION, name).await;
    kubeconfig_of(name, &described)
}

/// `CreateAccessEntry` for `principal_arn`, with the `username` and `groups` the
/// row asks for.
async fn create_access_entry(
    router: &MultiServiceRouter,
    cluster: &str,
    principal_arn: &str,
    username: Option<&str>,
    groups: &[&str],
) -> Answer {
    let mut body = json!({ "principalArn": principal_arn, "kubernetesGroups": groups });
    if let Some(username) = username {
        body["username"] = json!(username);
    }
    call(
        router,
        REGION,
        "POST",
        &format!("/clusters/{cluster}/access-entries"),
        &body.to_string(),
    )
    .await
}

/// `DeleteAccessEntry` for `principal_arn`.
async fn delete_access_entry(
    router: &MultiServiceRouter,
    cluster: &str,
    principal_arn: &str,
) -> Answer {
    call_as(
        router,
        DEFAULT_ACCESS_KEY_ID,
        REGION,
        "DELETE",
        &format!(
            "/clusters/{cluster}/access-entries/{}",
            encode_label(principal_arn)
        ),
        "",
    )
    .await
}

/// `GET /api/v1/namespaces/<ns>/pods?limit=1` as the credential `access_key_id`:
/// the read a client makes to find out whether the cluster lets it list pods.
async fn list_pods(config: &Kubeconfig, case: &str, access_key_id: &str) -> ApiAnswer {
    dial_with(
        config,
        case,
        "GET",
        &format!("/api/v1/namespaces/{NAMESPACE}/pods?limit=1"),
        "application/json",
        Some(&bearer_token(access_key_id)),
        None,
    )
    .await
}

/// `POST /api/v1/namespaces` as the credential `access_key_id`: a cluster-scoped
/// create, which is the request the API server's refusal names the caller in.
async fn create_namespace(
    config: &Kubeconfig,
    case: &str,
    access_key_id: &str,
    name: &str,
) -> ApiAnswer {
    dial_with(
        config,
        case,
        "POST",
        "/api/v1/namespaces",
        "application/json",
        Some(&bearer_token(access_key_id)),
        Some(&json!({ "metadata": { "name": name } })),
    )
    .await
}

/// `POST /apis/authentication.k8s.io/v1/selfsubjectreviews` as the credential
/// `access_key_id`: the request a client makes to be told which identity the API
/// server is serving it as.
async fn self_subject_review(
    config: &Kubeconfig,
    case: &str,
    access_key_id: &str,
) -> ApiAnswer {
    dial_with(
        config,
        case,
        "POST",
        "/apis/authentication.k8s.io/v1/selfsubjectreviews",
        "application/json",
        Some(&bearer_token(access_key_id)),
        Some(&json!({ "apiVersion": "authentication.k8s.io/v1", "kind": "SelfSubjectReview" })),
    )
    .await
}

/// The username and the groups a `SelfSubjectReview` answer reports.
fn reviewed_identity(case: &str, answered: &ApiAnswer) -> (String, Vec<String>) {
    assert_eq!(
        answered.status, 201,
        "case {case:?}: a self subject review must be created, got {answered:?}"
    );
    let info = &answered.body["status"]["userInfo"];
    (
        info["username"].as_str().unwrap_or_default().to_string(),
        info["groups"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter_map(|group| group.as_str().map(str::to_string))
            .collect(),
    )
}

/// The refusal the API server words for `username` on a cluster-scoped create of a
/// namespace — fixed text, with the username, the verb, the resource and the API
/// group taken from the request and the authenticated identity.
fn forbidden_message(username: &str) -> String {
    format!(
        "namespaces is forbidden: User \"{username}\" cannot create resource \"namespaces\" \
         in API group \"\" at the cluster scope"
    )
}

// ---------------------------------------------------------------------------
// RULE: the username a cluster authenticates an IAM principal as is its mapping's
// own with the placeholders Amazon EKS resolves per request filled in from the
// presented credential — so a ROLE principal authenticates as the assumed-role
// session that signed the request.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_role_principal_is_named_by_the_session_it_presents_not_by_its_role_arn() {
    let router = emulator_with_identity();
    let cluster = "session-usernames";
    let config = active_cluster(&router, cluster, "API_AND_CONFIG_MAP").await;
    let account = engine::emulator_account_id();

    // (case, the username the entry carries, the username the API server must
    // name) — the rows differ along the form of the mapping's username: the one
    // Amazon EKS specifies for a role, one the caller chose outright, and one
    // carrying each placeholder EKS resolves per request.
    struct Row {
        case: &'static str,
        role: &'static str,
        session: &'static str,
        username: Option<&'static str>,
        /// `{session}` stands for the session principal of the row's own session,
        /// which only the row's run knows.
        expected: String,
    }
    let rows = vec![
        Row {
            case: "the username Amazon EKS specifies for a role principal",
            role: "SessionNamedRole",
            session: "s2r-specified",
            username: None,
            expected: "{session}".to_string(),
        },
        Row {
            case: "a username the caller chose",
            role: "ChosenNameRole",
            session: "s2r-chosen",
            username: Some("chosen-kubernetes-user"),
            expected: "chosen-kubernetes-user".to_string(),
        },
        Row {
            case: "a username carrying the session placeholder",
            role: "TemplatedNameRole",
            session: "s2r-templated",
            username: Some("cluster-user-{{SessionName}}"),
            expected: "cluster-user-s2r-templated".to_string(),
        },
        Row {
            case: "a username carrying the account placeholder",
            role: "AccountNameRole",
            session: "s2r-account",
            username: Some("user-of-{{AccountID}}"),
            expected: format!("user-of-{account}"),
        },
    ];

    for row in &rows {
        let case = row.case;
        let session = assumed_role(&router, row.role, row.session).await;
        let created = create_access_entry(
            &router,
            cluster,
            &session.role_arn,
            row.username,
            // No group and no access policy: the identity authenticates and is
            // then refused, which is the answer that NAMES it.
            &[],
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: CreateAccessEntry must be accepted, got {created:?}"
        );

        // The stored entry keeps the username as EKS specified or the caller wrote
        // it — the template, placeholders and all, which is what a client reads
        // back.
        let stored = created.body["accessEntry"]["username"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        if row.username.is_none() {
            assert_eq!(
                stored,
                format!("arn:aws:sts::{account}:assumed-role/{}/{{{{SessionName}}}}", row.role),
                "case {case:?}: Amazon EKS specifies the assumed-role username of a role \
                 principal, got {created:?}"
            );
        } else {
            assert_eq!(
                Some(stored.as_str()),
                row.username,
                "case {case:?}: the stored entry keeps the username the caller wrote, got \
                 {created:?}"
            );
        }

        let expected = row.expected.replace("{session}", &session.session_principal());
        let refused = create_namespace(&config, case, &session.access_key_id, "denied-ns").await;
        assert_eq!(
            refused.status, 403,
            "case {case:?}: an identity with no permission is refused 403, got {refused:?}"
        );
        assert_eq!(
            refused.body["message"],
            json!(forbidden_message(&expected)),
            "case {case:?}: expected the refusal to name {expected:?}, got {refused:?}"
        );
        // And the name it was refused under is NOT the role ARN the entry is filed
        // under, for any row whose mapping resolves a placeholder.
        assert_eq!(
            refused.body["reason"],
            json!("Forbidden"),
            "case {case:?}: the refusal is the Kubernetes Forbidden Status, got {refused:?}"
        );
        assert_eq!(
            refused.body["details"]["kind"],
            json!("namespaces"),
            "case {case:?}: the refusal names the resource it addressed, got {refused:?}"
        );
    }
}

/// A long-term credential names no session, so a username that asks for one says
/// nothing about that caller: the placeholder is reported as written rather than
/// filled with nothing.
#[tokio::test]
async fn a_long_term_credential_resolves_no_session_name() {
    let router = emulator_with_identity();
    let cluster = "long-term-username";
    let config = active_cluster(&router, cluster, "API_AND_CONFIG_MAP").await;

    // The account's own published credential belongs to an IAM user, and the
    // cluster's creator entry is filed under it: the identity it authenticates as
    // is that user, with no session in the name.
    let listed = call(
        &router,
        REGION,
        "GET",
        &format!("/clusters/{cluster}/access-entries"),
        "",
    )
    .await;
    let principal = listed.body["accessEntries"][0]
        .as_str()
        .unwrap_or_else(|| panic!("the creator's entry must be listed, got {listed:?}"))
        .to_string();
    let described = call(
        &router,
        REGION,
        "GET",
        &format!(
            "/clusters/{cluster}/access-entries/{}",
            encode_label(&principal)
        ),
        "",
    )
    .await;
    assert_eq!(
        described.body["accessEntry"]["username"],
        json!(principal),
        "an IAM user authenticates as its own ARN, got {described:?}"
    );

    // That creator is the cluster's admin, so its own requests are admitted: the
    // identity resolved for a long-term credential is a working one.
    let created = dial(
        &config,
        "the cluster creator",
        "POST",
        "/api/v1/namespaces",
        Some(&json!({ "metadata": { "name": "creator-made" } })),
    )
    .await;
    assert_eq!(
        created.status, 201,
        "the creator's own credential must be admitted, got {created:?}"
    );
}

// ---------------------------------------------------------------------------
// RULE: a cluster whose authenticationMode includes CONFIG_MAP authenticates a
// principal its kube-system/aws-auth ConfigMap maps; an access entry for the same
// principal takes precedence, and deleting it falls back to the ConfigMap.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_aws_auth_config_map_authorizes_the_principal_it_maps() {
    let router = emulator_with_identity();
    let cluster = "aws-auth-mapping";
    let config = active_cluster(&router, cluster, "API_AND_CONFIG_MAP").await;
    let case = "a principal the aws-auth ConfigMap maps";

    // The cluster's creator — its admin — publishes the RBAC objects that make the
    // mapped group one that may list pods, and writes the mapping itself.
    for (what, body) in [
        ("the ClusterRole", pod_reader_role()),
        ("the ClusterRoleBinding", pod_reader_binding()),
    ] {
        let path = if what.ends_with("Binding") {
            "/apis/rbac.authorization.k8s.io/v1/clusterrolebindings"
        } else {
            "/apis/rbac.authorization.k8s.io/v1/clusterroles"
        };
        let created = dial(&config, what, "POST", path, Some(&body)).await;
        assert_eq!(
            created.status, 201,
            "setup: {what} must be created, got {created:?}"
        );
    }

    let mapped = assumed_role(&router, "AwsAuthMappedRole", "s2r-config-map").await;
    let unmapped = assumed_role(&router, "AwsAuthUnmappedRole", "s2r-unmapped").await;

    // The mapping as a client writes it: one `mapRoles` entry naming the role, the
    // username it authenticates as and the group it is in.
    let map_roles = format!(
        "- rolearn: {}\n  username: config-map-user\n  groups:\n    - {CONFIG_MAP_GROUP}\n",
        mapped.role_arn
    );
    let created = dial(
        &config,
        case,
        "POST",
        "/api/v1/namespaces/kube-system/configmaps",
        Some(&json!({
            "metadata": { "name": "aws-auth" },
            "data": { "mapRoles": map_roles },
        })),
    )
    .await;
    assert_eq!(
        created.status, 201,
        "setup: the aws-auth ConfigMap must be created, got {created:?}"
    );

    // The mapped principal lists pods, and the one the ConfigMap does not name
    // authenticates as nobody.
    let allowed = list_pods(&config, case, &mapped.access_key_id).await;
    assert_eq!(
        allowed.status, 200,
        "case {case:?}: the mapped principal must be allowed to list pods, got {allowed:?}"
    );
    let stranger = list_pods(&config, "a principal no mapping names", &unmapped.access_key_id).await;
    assert_eq!(
        stranger.status, 401,
        "case \"unmapped\": a principal neither mapping names authenticates as nobody, got \
         {stranger:?}"
    );

    // And the API server reports that identity to the client that asks: the
    // mapping's username, and its group beside the one every authenticated
    // request carries.
    let (username, groups) =
        reviewed_identity(case, &self_subject_review(&config, case, &mapped.access_key_id).await);
    assert_eq!(
        username, "config-map-user",
        "case {case:?}: the review must report the mapped username, got {username:?}"
    );
    for expected in [CONFIG_MAP_GROUP, "system:authenticated"] {
        assert!(
            groups.iter().any(|group| group == expected),
            "case {case:?}: the review must report the group {expected:?}, got {groups:?}"
        );
    }

    // And the mapping's own username is the name the API server knows it by: a
    // request it may not make is refused under it.
    let refused = create_namespace(&config, case, &mapped.access_key_id, "mapped-ns").await;
    assert_eq!(
        (refused.status, refused.body["message"].clone()),
        (403, json!(forbidden_message("config-map-user"))),
        "case {case:?}: expected the refusal to name the mapped username, got {refused:?}"
    );

    // AN ACCESS ENTRY FOR THE SAME PRINCIPAL TAKES PRECEDENCE: the entry's
    // username is what the API server names it by, and its groups are the only
    // ones it carries — so the ConfigMap's group, and the pod listing it allowed,
    // are gone.
    let entry = create_access_entry(
        &router,
        cluster,
        &mapped.role_arn,
        Some("access-entry-user"),
        &["access-entry-group"],
    )
    .await;
    assert_eq!(
        entry.status, 200,
        "case {case:?}: the access entry must be created, got {entry:?}"
    );
    // The review now reports the ENTRY's identity, and the ConfigMap's group is
    // absent from it: one principal is served under one mapping, not both.
    let (username, groups) = reviewed_identity(
        "the access entry taking precedence",
        &self_subject_review(
            &config,
            "the access entry taking precedence",
            &mapped.access_key_id,
        )
        .await,
    );
    assert_eq!(
        username, "access-entry-user",
        "case \"precedence\": the review must report the access entry's username, got \
         {username:?}"
    );
    assert!(
        groups.iter().any(|group| group == "access-entry-group"),
        "case \"precedence\": the review must report the entry's group, got {groups:?}"
    );
    assert!(
        !groups.iter().any(|group| group == CONFIG_MAP_GROUP),
        "case \"precedence\": the ConfigMap's group must be absent, got {groups:?}"
    );
    let overridden = list_pods(&config, "the access entry taking precedence", &mapped.access_key_id)
        .await;
    assert_eq!(
        overridden.status, 403,
        "case \"precedence\": the access entry's groups are the only ones the principal \
         carries, got {overridden:?}"
    );
    assert_eq!(
        overridden.body["message"],
        json!(
            "pods is forbidden: User \"access-entry-user\" cannot list resource \"pods\" in \
             API group \"\" in the namespace \"default\""
        ),
        "case \"precedence\": the refusal must name the access entry's username, got \
         {overridden:?}"
    );

    // DELETING THE ENTRY FALLS BACK to the ConfigMap mapping: the same credential
    // lists pods again, under the ConfigMap's username.
    let deleted = delete_access_entry(&router, cluster, &mapped.role_arn).await;
    assert_eq!(
        deleted.status, 200,
        "case {case:?}: DeleteAccessEntry must be accepted, got {deleted:?}"
    );
    let fallen_back = list_pods(&config, "the ConfigMap mapping again", &mapped.access_key_id).await;
    assert_eq!(
        fallen_back.status, 200,
        "case \"fall back\": the ConfigMap mapping must authorize the principal again, got \
         {fallen_back:?}"
    );
    let (username, groups) = reviewed_identity(
        "the ConfigMap mapping again",
        &self_subject_review(&config, "the ConfigMap mapping again", &mapped.access_key_id).await,
    );
    assert_eq!(
        (username.as_str(), groups.iter().any(|group| group == CONFIG_MAP_GROUP)),
        ("config-map-user", true),
        "case \"fall back\": the review must report the ConfigMap's identity again, got \
         {username:?} {groups:?}"
    );
}

/// The group a client asks about itself in is DISCOVERABLE: it is named at
/// `/apis`, it has its own `APIGroup` document, and its version names the one
/// resource this server serves in it.
#[tokio::test]
async fn the_api_server_discovers_the_group_a_client_asks_about_itself_in() {
    let router = emulator_with_identity();
    let cluster = "self-review-discovery";
    let config = active_cluster(&router, cluster, "API_AND_CONFIG_MAP").await;

    let groups = dial(&config, "the groups", "GET", "/apis", None).await;
    assert_eq!(
        groups.status, 200,
        "the group list must be served, got {groups:?}"
    );
    let named: Vec<String> = groups.body["groups"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|group| group["name"].as_str().map(str::to_string))
        .collect();
    assert!(
        named.iter().any(|name| name == "authentication.k8s.io"),
        "the group list must name the authentication group, got {named:?}"
    );

    let group = dial(
        &config,
        "the authentication group",
        "GET",
        "/apis/authentication.k8s.io",
        None,
    )
    .await;
    assert_eq!(
        (group.status, group.body["kind"].clone()),
        (200, json!("APIGroup")),
        "the group's own document must be served, got {group:?}"
    );

    let resources = dial(
        &config,
        "the authentication group's resources",
        "GET",
        "/apis/authentication.k8s.io/v1",
        None,
    )
    .await;
    assert_eq!(
        resources.status, 200,
        "the group's resource list must be served, got {resources:?}"
    );
    let found = resources.body["resources"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .find(|resource| resource["name"] == json!("selfsubjectreviews"))
        .unwrap_or_else(|| panic!("the resource list must name it, got {resources:?}"));
    assert_eq!(
        (found["kind"].clone(), found["namespaced"].clone()),
        (json!("SelfSubjectReview"), json!(false)),
        "the resource must be the cluster-scoped SelfSubjectReview, got {found:?}"
    );
    assert_eq!(
        found["verbs"],
        json!(["create"]),
        "a client CREATES a self subject review, got {found:?}"
    );
}

/// The mapping is the cluster's own object, so what a client WRITES into it is
/// what authenticates: a replaced ConfigMap maps what it now says, and a username
/// it carries resolves the placeholders Amazon EKS resolves per request.
#[tokio::test]
async fn a_replaced_aws_auth_mapping_is_what_authenticates() {
    let router = emulator_with_identity();
    let cluster = "aws-auth-replaced";
    let config = active_cluster(&router, cluster, "CONFIG_MAP").await;
    let case = "a replaced aws-auth mapping";

    let first = assumed_role(&router, "FirstMappedRole", "s2r-first").await;
    let second = assumed_role(&router, "SecondMappedRole", "s2r-second").await;

    let created = dial(
        &config,
        case,
        "POST",
        "/api/v1/namespaces/kube-system/configmaps",
        Some(&json!({
            "metadata": { "name": "aws-auth" },
            "data": {
                "mapRoles": format!("- rolearn: {}\n  username: first-user\n", first.role_arn),
            },
        })),
    )
    .await;
    assert_eq!(
        created.status, 201,
        "setup: the aws-auth ConfigMap must be created, got {created:?}"
    );

    // (case, the credential, the username the refusal must name) — before the
    // replace, only the first role is mapped.
    let before = create_namespace(&config, case, &first.access_key_id, "ns-one").await;
    assert_eq!(
        (before.status, before.body["message"].clone()),
        (403, json!(forbidden_message("first-user"))),
        "case \"before\": the mapped principal is named by its mapping, got {before:?}"
    );
    let unmapped = create_namespace(&config, case, &second.access_key_id, "ns-two").await;
    assert_eq!(
        unmapped.status, 401,
        "case \"before\": the principal the mapping does not name authenticates as nobody, \
         got {unmapped:?}"
    );

    // The replace: the second role is mapped now, under a username carrying the
    // session placeholder, and the first one is mapped no longer.
    let replaced = dial(
        &config,
        case,
        "PUT",
        "/api/v1/namespaces/kube-system/configmaps/aws-auth",
        Some(&json!({
            "metadata": { "name": "aws-auth" },
            "data": {
                "mapRoles": format!(
                    "- rolearn: {}\n  username: session-{{{{SessionName}}}}\n",
                    second.role_arn
                ),
            },
        })),
    )
    .await;
    assert_eq!(
        replaced.status, 200,
        "case {case:?}: the mapping must be replaceable, got {replaced:?}"
    );

    let after = create_namespace(&config, case, &second.access_key_id, "ns-three").await;
    assert_eq!(
        (after.status, after.body["message"].clone()),
        (403, json!(forbidden_message("session-s2r-second"))),
        "case \"after\": the replaced mapping resolves the session placeholder, got {after:?}"
    );
    let dropped = create_namespace(&config, case, &first.access_key_id, "ns-four").await;
    assert_eq!(
        dropped.status, 401,
        "case \"after\": a principal the replaced mapping no longer names authenticates as \
         nobody, got {dropped:?}"
    );
}

/// A cluster in `API` mode alone has no ConfigMap mapping: "the cluster relies
/// only on access entries", so a principal the ConfigMap maps authenticates as
/// nobody there.
#[tokio::test]
async fn an_api_only_cluster_does_not_authenticate_through_the_config_map() {
    let router = emulator_with_identity();
    let cluster = "api-only-cluster";
    let config = active_cluster(&router, cluster, "API").await;
    let case = "a ConfigMap mapping on an API-only cluster";

    let mapped = assumed_role(&router, "ApiOnlyMappedRole", "s2r-api-only").await;
    let created = dial(
        &config,
        case,
        "POST",
        "/api/v1/namespaces/kube-system/configmaps",
        Some(&json!({
            "metadata": { "name": "aws-auth" },
            "data": {
                "mapRoles": format!(
                    "- rolearn: {}\n  username: ignored-user\n  groups:\n    - {CONFIG_MAP_GROUP}\n",
                    mapped.role_arn
                ),
            },
        })),
    )
    .await;
    assert_eq!(
        created.status, 201,
        "setup: the ConfigMap must be created, got {created:?}"
    );

    let refused = list_pods(&config, case, &mapped.access_key_id).await;
    assert_eq!(
        refused.status, 401,
        "case {case:?}: an API-only cluster maps no principal through its ConfigMap, got \
         {refused:?}"
    );

    // The same principal, given an access entry, authenticates: it is the mode
    // that decides which mapping answers, not the principal.
    let entry = create_access_entry(
        &router,
        cluster,
        &mapped.role_arn,
        Some("entry-user"),
        &[CONFIG_MAP_GROUP],
    )
    .await;
    assert_eq!(
        entry.status, 200,
        "case {case:?}: the access entry must be created, got {entry:?}"
    );
    let allowed = list_pods(&config, case, &mapped.access_key_id).await;
    assert_eq!(
        allowed.status, 403,
        "case {case:?}: the entry's principal authenticates and is decided by RBAC, got \
         {allowed:?}"
    );
    assert_eq!(
        allowed.body["message"],
        json!(
            "pods is forbidden: User \"entry-user\" cannot list resource \"pods\" in API \
             group \"\" in the namespace \"default\""
        ),
        "case {case:?}: the refusal names the entry's username, got {allowed:?}"
    );
}

/// A `mapRoles` value this server does not read as a list of mappings states NO
/// mapping: the principal it would have named authenticates as nobody rather than
/// as something guessed at — and the read is bounded, so one ConfigMap cannot
/// decide what a request to this address costs.
#[tokio::test]
async fn a_value_the_mapping_is_not_read_from_maps_nobody() {
    let router = emulator_with_identity();
    let cluster = "aws-auth-unreadable";
    let config = active_cluster(&router, cluster, "API_AND_CONFIG_MAP").await;

    let mapped = assumed_role(&router, "UnreadableMappedRole", "s2r-unreadable").await;
    let entry = format!(
        "- rolearn: {}\n  username: read-user\n  groups:\n    - {CONFIG_MAP_GROUP}\n",
        mapped.role_arn
    );

    // The RBAC objects that make the mapped group one that may list pods, so a row
    // whose value IS read reaches the 200 a mapped principal gets.
    for (what, body) in [
        ("the ClusterRole", pod_reader_role()),
        ("the ClusterRoleBinding", pod_reader_binding()),
    ] {
        let path = if what.ends_with("Binding") {
            "/apis/rbac.authorization.k8s.io/v1/clusterrolebindings"
        } else {
            "/apis/rbac.authorization.k8s.io/v1/clusterroles"
        };
        let created = dial(&config, what, "POST", path, Some(&body)).await;
        assert_eq!(
            created.status, 201,
            "setup: {what} must be created, got {created:?}"
        );
    }

    // A list whose one entry nests deeper than a mapping list is read: a list of
    // mappings whose groups are a list of scalars is three levels, and this is far
    // past the bound.
    let mut deep = entry.clone();
    deep.push_str("- deep:\n");
    for level in 0..12_usize {
        deep.push_str(&format!("{}nested:\n", " ".repeat(4 + level * 2)));
    }
    deep.push_str(&format!("{}leaf: x\n", " ".repeat(4 + 12 * 2)));

    // (case, the value written into `mapRoles`, whether the mapping is read from
    // it) — the rows differ along WHY a value states no mapping: it is not a list,
    // it is not YAML at all, it references an anchor (which a loader resolves by
    // copying the anchored node into every alias position), it nests past what is
    // read, or it is larger than is read.
    let rows: Vec<(&str, String, bool)> = vec![
        ("a list of mappings", entry.clone(), true),
        (
            "a mapping rather than a list",
            format!("rolearn: {}\nusername: read-user\n", mapped.role_arn),
            false,
        ),
        ("a value that is not YAML", "a: b: c\n".to_string(), false),
        (
            "a value that references an anchor",
            format!("- &anchor\n  rolearn: {}\n  username: read-user\n- *anchor\n", mapped.role_arn),
            false,
        ),
        ("a value nested past what is read", deep, false),
        (
            // Past the 64 KiB this server reads one mapping list as, which the
            // ConfigMap's own 1 MiB ceiling leaves room for.
            "a value larger than is read",
            format!("{entry}{}", "# padding\n".repeat(8_000)),
            false,
        ),
        ("the list of mappings again", entry.clone(), true),
    ];

    let created = dial(
        &config,
        "the aws-auth ConfigMap",
        "POST",
        "/api/v1/namespaces/kube-system/configmaps",
        Some(&json!({
            "metadata": { "name": "aws-auth" },
            "data": { "mapRoles": entry },
        })),
    )
    .await;
    assert_eq!(
        created.status, 201,
        "setup: the aws-auth ConfigMap must be created, got {created:?}"
    );

    for (case, value, read) in &rows {
        let replaced = dial(
            &config,
            case,
            "PUT",
            "/api/v1/namespaces/kube-system/configmaps/aws-auth",
            Some(&json!({
                "metadata": { "name": "aws-auth" },
                "data": { "mapRoles": value },
            })),
        )
        .await;
        assert_eq!(
            replaced.status, 200,
            "case {case:?}: the ConfigMap must hold whatever a client writes, got {replaced:?}"
        );
        let answered = list_pods(&config, case, &mapped.access_key_id).await;
        let expected = if *read { 200 } else { 401 };
        assert_eq!(
            answered.status, expected,
            "case {case:?}: expected HTTP {expected} for a value the mapping is \
             {}read from, got {answered:?}",
            if *read { "" } else { "not " }
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: the identity the API server reports about an authenticated request is a
// COMPLETE Kubernetes UserInfo — beside the username its mapping resolved and the
// groups it carries, the SUBJECT UID the cluster's authenticating authority minted
// for the calling IAM principal (that authority's name, the account the principal
// belongs to and the principal's own unique id, joined by colons) and the `extra`
// the authority reports about it, whose `principalId` is the very id the uid's last
// segment carries. Both are facts of the CREDENTIAL that authenticated the request
// and of the account's own records, so one principal is one subject under either of
// the cluster's mappings and however many sessions of it sign requests.
// ---------------------------------------------------------------------------

/// The subject uid an Amazon EKS cluster's authenticating authority mints for the
/// principal `principal_id`: its own name, the account of the principal — read from
/// the account contract rather than written out — and that principal's unique id.
fn expected_subject_uid(principal_id: &str) -> String {
    format!(
        "aws-iam-authenticator:{}:{principal_id}",
        engine::emulator_account_id()
    )
}

/// The group every authenticated identity carries, whatever its mapping names.
const AUTHENTICATED_GROUP: &str = "system:authenticated";

/// The rows naming two sessions of ONE role, which must be one subject, and the row
/// naming the principal the ConfigMap maps, which must be another.
const FIRST_SESSION_CASE: &str = "a session of a role an access entry maps";
const SECOND_SESSION_CASE: &str = "another session of the same role";
const LONG_TERM_CASE: &str = "the long-term credential of an IAM user";
const CONFIG_MAP_CASE: &str = "a session of a role the aws-auth ConfigMap maps";

#[tokio::test]
async fn the_reported_identity_carries_the_subject_uid_of_the_authenticated_principal() {
    let router = emulator_with_identity();
    let cluster = "subject-uid";
    let config = active_cluster(&router, cluster, "API_AND_CONFIG_MAP").await;

    // ONE role assumed TWICE: two credentials of one IAM principal, which the
    // cluster maps through the one access entry filed under that role.
    let first = assumed_role(&router, "SubjectUidRole", "s2r-uid-first").await;
    let second = another_session(&router, &first.role_arn, "s2r-uid-second").await;
    let entry =
        create_access_entry(&router, cluster, &first.role_arn, Some("uid-entry-user"), &[]).await;
    assert_eq!(
        entry.status, 200,
        "setup: the access entry must be created, got {entry:?}"
    );

    // And another role, mapped by the cluster's OTHER mapping, so the rule is
    // asserted under both of them.
    let mapped = assumed_role(&router, "SubjectUidMappedRole", "s2r-uid-mapped").await;
    let created = dial(
        &config,
        "the aws-auth ConfigMap",
        "POST",
        "/api/v1/namespaces/kube-system/configmaps",
        Some(&json!({
            "metadata": { "name": "aws-auth" },
            "data": {
                "mapRoles": format!(
                    "- rolearn: {}\n  username: uid-config-map-user\n",
                    mapped.role_arn
                ),
            },
        })),
    )
    .await;
    assert_eq!(
        created.status, 201,
        "setup: the aws-auth ConfigMap must be created, got {created:?}"
    );

    /// One credential the cluster authenticates, and what the API server must
    /// report about the identity it serves it as.
    struct Row {
        case: &'static str,
        access_key_id: String,
        /// The username the cluster's mapping resolves for it.
        username: String,
        /// The permanent IAM principal the credential speaks for.
        principal_arn: String,
        /// That principal's own unique id.
        principal_id: String,
        /// The session the credential was minted for, empty for a long-term one.
        session_name: String,
    }

    // The rows differ along what the rule generalises over: the KIND of credential
    // (a long-term one, a session), WHICH of the cluster's two mappings maps the
    // principal (the creator's own access entry, an entry the caller created, the
    // aws-auth ConfigMap), and whether two rows are two sessions of ONE principal.
    let rows = vec![
        Row {
            case: LONG_TERM_CASE,
            access_key_id: DEFAULT_ACCESS_KEY_ID.to_string(),
            // The cluster's creator is the account's own published credential, and
            // the entry Amazon EKS files for it names that principal.
            username: service_common::caller_principal::ambient_user_arn(),
            principal_arn: service_common::caller_principal::ambient_user_arn(),
            principal_id: service_common::caller_principal::ambient_user_id(),
            session_name: String::new(),
        },
        Row {
            case: FIRST_SESSION_CASE,
            access_key_id: first.access_key_id.clone(),
            username: "uid-entry-user".to_string(),
            principal_arn: first.role_arn.clone(),
            principal_id: first.principal_id.clone(),
            session_name: first.session_name.clone(),
        },
        Row {
            case: SECOND_SESSION_CASE,
            access_key_id: second.access_key_id.clone(),
            username: "uid-entry-user".to_string(),
            principal_arn: second.role_arn.clone(),
            principal_id: second.principal_id.clone(),
            session_name: second.session_name.clone(),
        },
        Row {
            case: CONFIG_MAP_CASE,
            access_key_id: mapped.access_key_id.clone(),
            username: "uid-config-map-user".to_string(),
            principal_arn: mapped.role_arn.clone(),
            principal_id: mapped.principal_id.clone(),
            session_name: mapped.session_name.clone(),
        },
    ];

    let mut subjects: Vec<(&str, String)> = Vec::new();
    for row in &rows {
        let case = row.case;
        let answered = self_subject_review(&config, case, &row.access_key_id).await;
        assert_eq!(
            answered.status, 201,
            "case {case:?}: a self subject review must be created, got {answered:?}"
        );
        let info = &answered.body["status"]["userInfo"];
        assert_eq!(
            info["username"],
            json!(row.username),
            "case {case:?}: expected the username {:?} the mapping resolves, got {answered:?}",
            row.username
        );
        let expected = expected_subject_uid(&row.principal_id);
        assert_eq!(
            info["uid"],
            json!(expected),
            "case {case:?}: expected the subject uid {expected:?}, got {answered:?}"
        );
        assert_eq!(
            info["extra"]["principalId"],
            json!([row.principal_id]),
            "case {case:?}: expected the principal id {:?} reported, got {answered:?}",
            row.principal_id
        );
        // The uid is not opaque: its last segment IS the reported principal id, so
        // the two members agree.
        let uid = info["uid"].as_str().unwrap_or_default();
        let last_segment = uid.rsplit(':').next().unwrap_or_default();
        assert_eq!(
            json!(last_segment),
            info["extra"]["principalId"][0],
            "case {case:?}: the uid's last segment must be the reported principalId, got \
             {answered:?}"
        );
        assert_eq!(
            info["extra"]["canonicalArn"],
            json!([row.principal_arn]),
            "case {case:?}: expected the IAM principal {:?} reported, got {answered:?}",
            row.principal_arn
        );
        assert_eq!(
            info["extra"]["sessionName"],
            json!([row.session_name]),
            "case {case:?}: expected the session {:?} the credential was minted for, got \
             {answered:?}",
            row.session_name
        );
        assert!(
            info["groups"]
                .as_array()
                .is_some_and(|groups| groups.contains(&json!(AUTHENTICATED_GROUP))),
            "case {case:?}: an authenticated identity is in {AUTHENTICATED_GROUP:?}, got \
             {answered:?}"
        );
        subjects.push((case, uid.to_string()));
    }

    let subject_of = |wanted: &str| -> String {
        subjects
            .iter()
            .find(|(case, _)| *case == wanted)
            .map(|(_, uid)| uid.clone())
            .unwrap_or_else(|| panic!("case {wanted:?}: the row must have been reviewed"))
    };

    // Two sessions of one role are ONE subject: the uid names the IAM principal,
    // not the credential or the session it was minted for.
    assert_eq!(
        subject_of(FIRST_SESSION_CASE),
        subject_of(SECOND_SESSION_CASE),
        "case {SECOND_SESSION_CASE:?}: two sessions of one principal are one subject, got \
         {subjects:?}"
    );
    // And two principals are two subjects, however the mappings name them.
    for (left, right) in [
        (LONG_TERM_CASE, FIRST_SESSION_CASE),
        (LONG_TERM_CASE, CONFIG_MAP_CASE),
        (FIRST_SESSION_CASE, CONFIG_MAP_CASE),
    ] {
        assert_ne!(
            subject_of(left),
            subject_of(right),
            "case {left:?} against {right:?}: two IAM principals are two subjects, got \
             {subjects:?}"
        );
    }
}
