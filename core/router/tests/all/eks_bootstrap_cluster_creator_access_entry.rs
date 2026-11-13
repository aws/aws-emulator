// SPDX-License-Identifier: CC0-1.0

//! WHICH principal Amazon EKS files the cluster creator's bootstrap access entry
//! under: three services, one identity.
//!
//! RULE: when a `CreateCluster`'s effective
//! `accessConfig.bootstrapClusterCreatorAdminPermissions` holds, the access entry it
//! materialises is the one for the IAM principal THE REQUEST SPEAKS FOR — read from
//! the same place `sts:GetCallerIdentity` answers from: the session the access key it
//! signed with was minted for, else the IAM user that holds that long-term key. A
//! role session is a temporary principal that cannot hold an access entry ("You can't
//! use the STS session principal type with access entries because this is a temporary
//! principal for each session and not a permanent identity that can be assigned
//! permissions", `CreateAccessEntryRequest$principalArn`), so a request signed by one
//! files the entry under the ROLE the session is of.
//!
//! Three services have to agree for that to hold — IAM holds the user and the role,
//! STS mints the session credential and names the principal it speaks for, EKS writes
//! the entry — so this is the suite that can pin it; the EKS crate's own
//! `cluster_creator_access_entry.rs` pins what the entry CONTAINS and when it is
//! written at all, which needs no other service.
//!
//! The rows span who the caller is: the account's own published credential, an IAM
//! user's own long-term access key, and an assumed-role session of a role under a
//! path. The two IAM identities carry a policy allowing this service's actions, because
//! EKS decides every request against the policies in force on the caller and a principal
//! carrying none may call nothing; who the entry is filed under is what this suite pins.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The region every request here signs for.
const REGION: &str = "us-east-1";

/// A key id no store holds, which is the account's own published credential — the
/// position AWS account administrative credentials are in.
const AMBIENT_KEY: &str = "AKIAIOSFODNN7EXAMPLE";

/// The session name every `AssumeRole` below asks for.
const SESSION: &str = "creator";

/// A trust policy letting the account root assume a role. It carries none of the
/// characters an `application/x-www-form-urlencoded` body treats specially.
const TRUST_POLICY: &str = concat!(
    r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","#,
    r#""Principal":{"AWS":"arn:aws:iam::000000000000:root"},"Action":"sts:AssumeRole"}]}"#
);

/// An identity-based policy allowing every action of Amazon EKS on every resource.
///
/// A principal that creates a cluster has to be ALLOWED to: EKS decides every request
/// against the identity-based policies in force on the principal its credential speaks
/// for, so a user or role carrying no policy at all may call nothing (IAM's default
/// deny). The account's own published credential holds no IAM policies and is not bound
/// by that decision, which is why only the two rows that sign as an IAM identity need
/// this.
const EKS_ADMIN_POLICY: &str = concat!(
    r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","#,
    r#""Action":"eks:*","Resource":"*"}]}"#
);

/// The three services this rule spans, over ONE engine.
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_sts_awssecuritytokenservicev20110615::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_eks_awswesleyfrontend::EmulatedService::new(engine),
    );
    router
}

/// A SigV4 `Authorization` header whose credential is `access_key_id` — the key a
/// service reads the caller's identity from. The signature is never checked: this
/// suite runs with authentication off.
fn authorization(access_key_id: &str, signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential={access_key_id}/20260101/{REGION}/{signing_name}/\
         aws4_request, SignedHeaders=host, Signature=0000"
    )
}

/// Post one awsQuery form body to `signing_name` as `access_key_id`.
async fn query(
    router: &MultiServiceRouter,
    access_key_id: &str,
    signing_name: &str,
    body: String,
) -> (u16, String) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{signing_name}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization(access_key_id, signing_name))
        .body(Bytes::from(body))
        .expect("the test request builds");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// Drive one EKS request (restJson1) as `access_key_id`.
async fn eks(
    router: &MultiServiceRouter,
    access_key_id: &str,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let payload = body.map(|json| json.to_string()).unwrap_or_default();
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("eks.{REGION}.amazonaws.com"))
        .header("content-type", "application/json")
        .header("authorization", authorization(access_key_id, "eks"))
        .body(Bytes::from(payload))
        .expect("the test request builds");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, parsed)
}

/// The text of the first `<name>…</name>` element of an XML body.
fn xml_element(body: &str, name: &str) -> Option<String> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = body.find(&open)? + open.len();
    let end = body[start..].find(&close)? + start;
    Some(body[start..end].to_string())
}

/// Percent-encode one `@httpLabel` segment, the way an AWS SDK does: keep the
/// unreserved alphanumerics and escape every other byte, so a `/` inside a principal
/// ARN is not read as a segment separator.
fn encoded(value: &str) -> String {
    value
        .bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() {
                (byte as char).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect()
}

/// The ARN `sts:GetCallerIdentity` answers for a request signed with `access_key_id`.
async fn caller_identity_arn(router: &MultiServiceRouter, case: &str, access_key_id: &str) -> String {
    let (status, body) = query(
        router,
        access_key_id,
        "sts",
        "Action=GetCallerIdentity&Version=2011-06-15".to_string(),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetCallerIdentity expected 200, got {status} {body}"
    );
    xml_element(&body, "Arn")
        .unwrap_or_else(|| panic!("case {case:?}: GetCallerIdentity answered no <Arn>: {body}"))
}

/// Create an IAM role with [`TRUST_POLICY`] under `path`, carrying
/// [`EKS_ADMIN_POLICY`] as its inline policy, and return its ARN.
async fn create_role(router: &MultiServiceRouter, role_name: &str, path: &str) -> String {
    let (status, body) = query(
        router,
        AMBIENT_KEY,
        "iam",
        format!(
            "Action=CreateRole&Version=2010-05-08&RoleName={role_name}&Path={}\
             &AssumeRolePolicyDocument={TRUST_POLICY}",
            encoded(path)
        ),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: CreateRole({role_name}) expected 200, got {status} {body}"
    );
    let (status, put) = query(
        router,
        AMBIENT_KEY,
        "iam",
        format!(
            "Action=PutRolePolicy&Version=2010-05-08&RoleName={role_name}&PolicyName=eks\
             &PolicyDocument={}",
            encoded(EKS_ADMIN_POLICY)
        ),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: PutRolePolicy({role_name}) expected 200, got {status} {put}"
    );
    xml_element(&body, "Arn").unwrap_or_else(|| panic!("setup: no <Arn> in {body}"))
}

/// Create an IAM user carrying [`EKS_ADMIN_POLICY`] as its inline policy, and one
/// long-term access key of its own; returns `(user ARN, access key id)`.
async fn create_user_with_key(router: &MultiServiceRouter, user_name: &str) -> (String, String) {
    let (status, body) = query(
        router,
        AMBIENT_KEY,
        "iam",
        format!("Action=CreateUser&Version=2010-05-08&UserName={user_name}"),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: CreateUser({user_name}) expected 200, got {status} {body}"
    );
    let arn = xml_element(&body, "Arn").unwrap_or_else(|| panic!("setup: no <Arn> in {body}"));
    let (status, put) = query(
        router,
        AMBIENT_KEY,
        "iam",
        format!(
            "Action=PutUserPolicy&Version=2010-05-08&UserName={user_name}&PolicyName=eks\
             &PolicyDocument={}",
            encoded(EKS_ADMIN_POLICY)
        ),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: PutUserPolicy({user_name}) expected 200, got {status} {put}"
    );
    let (status, body) = query(
        router,
        AMBIENT_KEY,
        "iam",
        format!("Action=CreateAccessKey&Version=2010-05-08&UserName={user_name}"),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: CreateAccessKey({user_name}) expected 200, got {status} {body}"
    );
    let key = xml_element(&body, "AccessKeyId")
        .unwrap_or_else(|| panic!("setup: no <AccessKeyId> in {body}"));
    (arn, key)
}

/// Assume `role_arn` and return the access key id of the session STS minted.
async fn assume_role(router: &MultiServiceRouter, role_arn: &str) -> String {
    let (status, body) = query(
        router,
        AMBIENT_KEY,
        "sts",
        format!("Action=AssumeRole&Version=2011-06-15&RoleArn={role_arn}&RoleSessionName={SESSION}"),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: AssumeRole({role_arn}) expected 200, got {status} {body}"
    );
    xml_element(&body, "AccessKeyId")
        .unwrap_or_else(|| panic!("setup: no <AccessKeyId> in {body}"))
}

/// A `CreateCluster` body carrying nothing beyond the required members and the
/// `authenticationMode` that serves the access-entry API, so
/// `bootstrapClusterCreatorAdminPermissions` takes its documented default of `true`
/// while the entry it writes can be read back: a cluster created through this API
/// defaults to `CONFIG_MAP`, which authenticates through the `aws-auth` ConfigMap
/// alone and serves no access-entry call.
fn cluster_body(name: &str) -> Value {
    json!({
        "name": name,
        "roleArn": format!("arn:aws:iam::{}:role/eksClusterRole", engine::emulator_account_id()),
        "resourcesVpcConfig": { "subnetIds": ["subnet-aaaa1111", "subnet-bbbb2222"] },
        "accessConfig": { "authenticationMode": "API_AND_CONFIG_MAP" },
    })
}

/// The principal ARNs `ListAccessEntries` reports for `cluster`.
async fn listed_entries(router: &MultiServiceRouter, case: &str, cluster: &str) -> Vec<String> {
    let (status, body) = eks(
        router,
        AMBIENT_KEY,
        "GET",
        &format!("/clusters/{}/access-entries", encoded(cluster)),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListAccessEntries expected 200, got {status} {body}"
    );
    body["accessEntries"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

/// One row: who signs the create, and the IAM principal the bootstrap entry must then
/// be filed under.
struct Row {
    case: &'static str,
    cluster: &'static str,
    /// The access key id the `CreateCluster` is signed with.
    access_key_id: String,
    /// The principal the entry must name.
    expected_principal: String,
    /// Whether that principal is also what `sts:GetCallerIdentity` reports for the
    /// same credential: it is, except for a role session, whose own ARN no access
    /// entry may carry.
    agrees_with_sts: bool,
}

#[tokio::test]
async fn the_bootstrap_access_entry_names_the_principal_the_create_was_signed_by() {
    let router = build_router();

    // The IAM user the account's own published credential belongs to, as STS reports
    // it for that credential: read rather than written out, so this row names the
    // same identity the token service does.
    let ambient_principal =
        caller_identity_arn(&router, "the account's own published credential", AMBIENT_KEY).await;
    // An IAM user of the account, with a long-term access key of its own.
    let (user_arn, user_key) = create_user_with_key(&router, "cluster-creator").await;
    // A role under a path, and a session of it: the session's own ARN is an STS
    // `assumed-role` principal, which is not a principal an access entry can name.
    let role_arn = create_role(&router, "ClusterCreatorRole", "/service-role/").await;
    let role_session_key = assume_role(&router, &role_arn).await;

    let rows = vec![
        Row {
            case: "the account's own published credential",
            cluster: "creator-ambient",
            access_key_id: AMBIENT_KEY.to_string(),
            expected_principal: ambient_principal,
            agrees_with_sts: true,
        },
        Row {
            case: "an IAM user's own long-term access key",
            cluster: "creator-user",
            access_key_id: user_key,
            expected_principal: user_arn,
            agrees_with_sts: true,
        },
        Row {
            case: "an assumed-role session of a role under a path",
            cluster: "creator-session",
            access_key_id: role_session_key,
            expected_principal: role_arn.clone(),
            agrees_with_sts: false,
        },
    ];

    for row in &rows {
        let Row {
            case,
            cluster,
            access_key_id,
            expected_principal,
            agrees_with_sts,
        } = row;

        let reported_caller = caller_identity_arn(&router, case, access_key_id).await;
        let (status, created) = eks(
            &router,
            access_key_id,
            "POST",
            "/clusters",
            Some(cluster_body(cluster)),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateCluster expected 200, got {status} {created}"
        );

        assert_eq!(
            listed_entries(&router, case, cluster).await,
            vec![expected_principal.clone()],
            "case {case:?}: the bootstrap entry must be filed under {expected_principal:?}, \
             the principal the credential speaks for"
        );

        // The entry really resolves under that principal, and authenticates as it.
        let (status, described) = eks(
            &router,
            AMBIENT_KEY,
            "GET",
            &format!(
                "/clusters/{}/access-entries/{}",
                encoded(cluster),
                encoded(expected_principal)
            ),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeAccessEntry({expected_principal}) expected 200, got \
             {status} {described}"
        );
        assert_eq!(
            described["accessEntry"]["username"].as_str(),
            Some(expected_principal.as_str()),
            "case {case:?}: the entry must authenticate as its principal, got {described}"
        );

        if *agrees_with_sts {
            assert_eq!(
                expected_principal, &reported_caller,
                "case {case:?}: the entry must name the identity STS reports for the same \
                 credential, which answered {reported_caller:?}"
            );
        } else {
            // "You can't use the STS session principal type with access entries":
            // the session's own principal is what STS reports, and the entry is
            // filed under the role behind it instead.
            assert_ne!(
                expected_principal, &reported_caller,
                "case {case:?}: a session's own ARN cannot be an access entry's principal, \
                 and STS answered {reported_caller:?}"
            );
            assert!(
                reported_caller.contains(":assumed-role/"),
                "case {case:?}: expected STS to report a session principal, got \
                 {reported_caller:?}"
            );
        }
    }
}
