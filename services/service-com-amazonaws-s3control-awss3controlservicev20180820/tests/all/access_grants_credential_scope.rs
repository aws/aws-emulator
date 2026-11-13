// SPDX-License-Identifier: CC0-1.0

//! What the temporary credential S3 Access Grants `GetDataAccess` vends may DO.
//!
//! The rule under test: the credential a vend hands out is a session of the registered
//! location's IAM role capped by a session policy derived from the scope the token carries
//! and the permission the vend asked for — so a request signed with it for an action that
//! permission covers, on data under that scope, is left to the role's own policies, and ANY
//! other request signed with it is refused 403 `AccessDenied` by Amazon S3, naming the
//! vended session, the action attempted and the resource addressed.
//!
//! Every row below differs along something that rule generalises over: the form of the
//! scope the token carries (a prefix wildcard, a whole bucket, a nested prefix, one object
//! key reached through the `Minimal` privilege), where the key that is NOT covered sits
//! (another bucket, a sibling prefix, a sibling key), and the permission the vend asks for
//! against the action it then attempts.
//!
//! The vended session's ARN is never written here: it is read back through
//! `sts:GetCallerIdentity` with the very credentials under test, which is the question that
//! service owns — S3 Access Grants mints a new session per call, so nothing about it can be
//! pinned.
//!
//! Every assertion goes through the same front door a client uses: a buffered
//! `http::Request` dispatched by the multi-service router, with S3 Control, Amazon S3 and
//! the token service mounted on one shared engine ([`ObjectStore::with_sts`]).

use std::sync::Arc;

use service_com_amazonaws_iam_awsidentitymanagementv20100508::ops::types::Role;
use service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam;
use service_com_amazonaws_s3control_awss3controlservicev20180820::state::{
    PERMISSION_READ, PERMISSION_READWRITE, PERMISSION_WRITE, PRIVILEGE_MINIMAL,
};
use service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts;

use crate::access_grants_data_plane::query_escape;
use crate::access_point_lifecycle::{xml_escape, xml_unescape, Reply, REGION, XMLNS};
use crate::object_store::{elements, ObjectStore};

/// The role every location below is registered with — the role S3 Access Grants assumes for
/// that location, held in the account's IAM records because the vended session is a session
/// OF it.
const LOCATION_ROLE_NAME: &str = "S3AccessGrantsLocationRole";

/// The name of the inline policy that role carries.
const LOCATION_ROLE_POLICY: &str = "S3AccessGrantsLocationAccess";

/// The service principal a location's role trusts: "the IAM role that S3 Access Grants
/// assumes must have the following permissions specified in the trust policy when
/// registering the location: sts:AssumeRole" (`GetDataAccess`).
const ACCESS_GRANTS_PRINCIPAL: &str = "access-grants.s3.amazonaws.com";

/// The grantee every grant below is created for, as `Grantee$GranteeIdentifier` spells an
/// IAM grantee.
const GRANTEE_IDENTIFIER: &str = "arn:aws:iam::123456789012:role/data-reader";

/// The bucket the granted data lives in, and a SECOND bucket no grant below covers.
const GRANTED_BUCKET: &str = "ledger";
const OTHER_BUCKET: &str = "vault";

/// The STS awsQuery API version `GetCallerIdentity` is called under, and its signing name.
const STS_API_VERSION: &str = "2011-06-15";
const STS_SIGNING_NAME: &str = "sts";

/// The bytes every object below holds, one per key, so a read that answers 200 can be held
/// to the object's own content rather than to a non-empty body.
fn content(bucket: &str, key: &str) -> Vec<u8> {
    format!("the bytes of {bucket}/{key}\n").into_bytes()
}

/// Record the location's role in the account's IAM, with an inline policy that allows the
/// object actions below on every bucket of the account.
///
/// Deliberately BROAD: the role is what the vended session's permissions are the
/// intersection WITH, so a refusal in the rows below can only come from the session policy
/// the vend derived — which is the rule under test.
fn hold_location_role(engine: &engine::InMemoryEngine) -> String {
    let account = engine::emulator_account_id();
    let arn = format!("arn:aws:iam::{account}:role/{LOCATION_ROLE_NAME}");
    let trust = format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Effect\":\"Allow\",\
         \"Principal\":{{\"Service\":\"{ACCESS_GRANTS_PRINCIPAL}\"}},\
         \"Action\":[\"sts:AssumeRole\",\"sts:SetSourceIdentity\",\"sts:SetContext\"]}}]}}"
    );
    let document = "{\"Version\":\"2012-10-17\",\"Statement\":[{\"Effect\":\"Allow\",\
                    \"Action\":[\"s3:GetObject\",\"s3:PutObject\",\"s3:DeleteObject\",\
                    \"s3:ListBucket\"],\
                    \"Resource\":[\"arn:aws:s3:::*\",\"arn:aws:s3:::*/*\"]}]}"
        .to_string();
    let state = iam::service_state(engine).expect("the in-memory engine carries the IAM store");
    let mut store = state.lock();
    store.roles.insert(
        iam::name_key(LOCATION_ROLE_NAME),
        Role {
            path: "/".to_string(),
            role_name: LOCATION_ROLE_NAME.to_string(),
            role_id: sts::role_id(&arn),
            arn: arn.clone(),
            create_date: ::aws_smithy_types::DateTime::from_secs(0),
            assume_role_policy_document: Some(trust),
            description: None,
            max_session_duration: Some(3_600),
            permissions_boundary: None,
            tags: None,
            role_last_used: None,
            source_role_template: None,
        },
    );
    store
        .role_inline_policies
        .entry(iam::name_key(LOCATION_ROLE_NAME))
        .or_default()
        .insert(
            iam::name_key(LOCATION_ROLE_POLICY),
            iam::InlinePolicy {
                policy_name: LOCATION_ROLE_POLICY.to_string(),
                policy_document: document,
            },
        );
    arn
}

/// Create the account's Access Grants instance in [`REGION`].
async fn create_instance(case: &str, store: &ObjectStore) {
    let reply = store
        .s3control(
            REGION,
            "POST",
            "/v20180820/accessgrantsinstance",
            &format!("<CreateAccessGrantsInstanceRequest xmlns=\"{XMLNS}\"/>"),
        )
        .await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: the Access Grants instance must be created first, got {} with body {}",
        reply.status, reply.body
    );
}

/// Register `location_scope` with `role_arn`, and answer the id the service assigned.
async fn create_location(
    case: &str,
    store: &ObjectStore,
    location_scope: &str,
    role_arn: &str,
) -> String {
    let body = format!(
        "<CreateAccessGrantsLocationRequest xmlns=\"{XMLNS}\">\
         <LocationScope>{}</LocationScope><IAMRoleArn>{}</IAMRoleArn>\
         </CreateAccessGrantsLocationRequest>",
        xml_escape(location_scope),
        xml_escape(role_arn)
    );
    let reply = store
        .s3control(
            REGION,
            "POST",
            "/v20180820/accessgrantsinstance/location",
            &body,
        )
        .await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: the location {location_scope} must be registered first, got {} with \
         body {}",
        reply.status, reply.body
    );
    reply
        .text("AccessGrantsLocationId")
        .unwrap_or_else(|| panic!("case {case:?}: no AccessGrantsLocationId in {}", reply.body))
}

/// Create a grant on `location_id` narrowed by `sub_prefix`, and answer the grant scope it
/// reported.
async fn create_grant(
    case: &str,
    store: &ObjectStore,
    location_id: &str,
    sub_prefix: &str,
    permission: &str,
) -> String {
    let body = format!(
        "<CreateAccessGrantRequest xmlns=\"{XMLNS}\">\
         <AccessGrantsLocationId>{}</AccessGrantsLocationId>\
         <AccessGrantsLocationConfiguration><S3SubPrefix>{}</S3SubPrefix>\
         </AccessGrantsLocationConfiguration>\
         <Grantee><GranteeType>IAM</GranteeType><GranteeIdentifier>{}</GranteeIdentifier>\
         </Grantee><Permission>{permission}</Permission></CreateAccessGrantRequest>",
        xml_escape(location_id),
        xml_escape(sub_prefix),
        xml_escape(GRANTEE_IDENTIFIER)
    );
    let reply = store
        .s3control(REGION, "POST", "/v20180820/accessgrantsinstance/grant", &body)
        .await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: the grant must be created first, got {} with body {}",
        reply.status, reply.body
    );
    reply
        .text("GrantScope")
        .unwrap_or_else(|| panic!("case {case:?}: no GrantScope in {}", reply.body))
}

/// The credential one vend hands out: the access key id and the session token of the
/// `GetDataAccess` answer.
#[derive(Debug, Clone)]
struct Vended {
    access_key_id: String,
    session_token: String,
}

/// Ask for temporary access to `target` with `permission`, optionally naming a privilege.
async fn get_data_access(
    store: &ObjectStore,
    target: &str,
    permission: &str,
    privilege: Option<&str>,
) -> Reply {
    let mut uri = format!(
        "/v20180820/accessgrantsinstance/dataaccess?target={}&permission={permission}",
        query_escape(target)
    );
    if let Some(privilege) = privilege {
        uri.push_str(&format!("&privilege={privilege}"));
    }
    store.s3control(REGION, "GET", &uri, "").await
}

/// [`get_data_access`], held to answering a credential.
async fn vend(case: &str, store: &ObjectStore, target: &str, permission: &str, privilege: Option<&str>) -> Vended {
    let reply = get_data_access(store, target, permission, privilege).await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: the vend for {target} must succeed, got {} with body {}",
        reply.status, reply.body
    );
    let access_key_id = reply
        .text("AccessKeyId")
        .unwrap_or_else(|| panic!("case {case:?}: no AccessKeyId in {}", reply.body));
    let session_token = reply
        .text("SessionToken")
        .unwrap_or_else(|| panic!("case {case:?}: no SessionToken in {}", reply.body));
    Vended {
        access_key_id,
        session_token,
    }
}

/// One path-style Amazon S3 request signed (in the routing sense) with `vended`: its status
/// and the body it answered.
async fn s3_as(
    store: &ObjectStore,
    vended: &Vended,
    method: &str,
    path: &str,
    body: &[u8],
) -> (u16, String) {
    let host = format!("s3.{REGION}.amazonaws.com");
    let request = ::http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{path}"))
        .header("host", &host)
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential={}/20260101/{REGION}/s3/aws4_request, \
                 SignedHeaders=host, Signature=0123456789abcdef",
                vended.access_key_id
            ),
        )
        .header("x-amz-security-token", &vended.session_token)
        .body(::bytes::Bytes::copy_from_slice(body))
        .expect("the request builds");
    let response = store.router.dispatch(request).await;
    (
        response.status().as_u16(),
        String::from_utf8_lossy(&response.into_body()).to_string(),
    )
}

/// The ARN `sts:GetCallerIdentity` reports for `vended` — the identity those very
/// credentials speak for, which is what a refusal of a request they signed names.
async fn session_arn(case: &str, store: &ObjectStore, vended: &Vended) -> String {
    let host = format!("{STS_SIGNING_NAME}.amazonaws.com");
    let request = ::http::Request::builder()
        .method("POST")
        .uri(format!("https://{host}/"))
        .header("host", &host)
        .header("content-type", "application/x-www-form-urlencoded")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential={}/20260101/{REGION}/{STS_SIGNING_NAME}/\
                 aws4_request, SignedHeaders=host, Signature=0123456789abcdef",
                vended.access_key_id
            ),
        )
        .header("x-amz-security-token", &vended.session_token)
        .body(::bytes::Bytes::from(format!(
            "Action=GetCallerIdentity&Version={STS_API_VERSION}"
        )))
        .expect("the request builds");
    let response = store.router.dispatch(request).await;
    let status = response.status().as_u16();
    let body = String::from_utf8_lossy(&response.into_body()).to_string();
    assert_eq!(
        status, 200,
        "case {case:?}: sts:GetCallerIdentity must answer for the vended credential, got \
         {status} with body {body}"
    );
    elements(&body, "Arn")
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("case {case:?}: no <Arn> in {body}"))
}

/// The sentence Amazon S3 refuses a request its session policy does not allow with, as IAM
/// words it: the principal, the action, the resource it addressed, and the document that
/// withheld the grant.
fn expected_refusal(principal: &str, action: &str, resource: &str) -> String {
    format!(
        "User: {principal} is not authorized to perform: {action} on resource: \
         \"{resource}\" because no session policy allows the {action} action"
    )
}

/// The ARN of one object, as a policy and a refusal name it.
fn object_resource(bucket: &str, key: &str) -> String {
    format!("arn:aws:s3:::{bucket}/{key}")
}

/// The `<Message>` of an S3 error body, with its XML escapes resolved — the sentence a
/// client reads, whose resource is quoted and so reaches the wire escaped.
fn message(case: &str, body: &str) -> String {
    elements(body, "Message")
        .into_iter()
        .next()
        .map(|text| xml_unescape(&text))
        .unwrap_or_else(|| panic!("case {case:?}: no <Message> in {body}"))
}

/// Stand both buckets up with the keys every row reads, and hold the location's role.
async fn granted_data(case: &str) -> (ObjectStore, Arc<engine::InMemoryEngine>, String) {
    let (store, engine) = ObjectStore::with_sts();
    let role_arn = hold_location_role(&engine);
    for bucket in [GRANTED_BUCKET, OTHER_BUCKET] {
        store.create_bucket(case, REGION, bucket).await;
    }
    for (bucket, key) in [
        (GRANTED_BUCKET, "granted/inside.txt"),
        (GRANTED_BUCKET, "granted/deep/inside.txt"),
        (GRANTED_BUCKET, "granted/other.txt"),
        (GRANTED_BUCKET, "ungranted/outside.txt"),
        (OTHER_BUCKET, "secret.txt"),
    ] {
        store
            .put_object(case, REGION, bucket, key, &content(bucket, key))
            .await;
    }
    (store, engine, role_arn)
}

/// One row of the scope table: what the token is vended for, the key it covers and the key
/// it does not.
struct ScopeRow {
    case: &'static str,
    /// The scope the location is registered for.
    location_scope: &'static str,
    /// The `S3SubPrefix` the grant narrows it by.
    sub_prefix: &'static str,
    /// The target the vend asks for, and the privilege it asks for it under.
    target: &'static str,
    privilege: Option<&'static str>,
    /// The scope the vend must report as `MatchedGrantTarget`.
    matched: &'static str,
    /// A key the scope covers, which the credential must be served.
    inside: (&'static str, &'static str),
    /// A key the scope does NOT cover, which the credential must be refused.
    outside: (&'static str, &'static str),
}

/// A credential is served exactly the data under the scope its token carries: a read inside
/// it answers the object's own bytes, and a read outside it is refused 403 `AccessDenied` in
/// the sentence IAM words for a request no session policy allows.
#[tokio::test]
async fn a_vended_credential_serves_only_the_data_its_scope_covers() {
    let rows = [
        ScopeRow {
            case: "a prefix wildcard",
            location_scope: "s3://ledger/granted/",
            sub_prefix: "*",
            target: "s3://ledger/granted/*",
            privilege: None,
            matched: "s3://ledger/granted/*",
            inside: (GRANTED_BUCKET, "granted/inside.txt"),
            outside: (GRANTED_BUCKET, "ungranted/outside.txt"),
        },
        ScopeRow {
            case: "a whole bucket, granted on the default location",
            location_scope: "s3://",
            sub_prefix: "ledger/*",
            target: "s3://ledger/*",
            privilege: None,
            matched: "s3://ledger/*",
            inside: (GRANTED_BUCKET, "ungranted/outside.txt"),
            outside: (OTHER_BUCKET, "secret.txt"),
        },
        ScopeRow {
            case: "a nested prefix inside the location",
            location_scope: "s3://ledger/granted/",
            sub_prefix: "deep/*",
            target: "s3://ledger/granted/deep/*",
            privilege: None,
            matched: "s3://ledger/granted/deep/*",
            inside: (GRANTED_BUCKET, "granted/deep/inside.txt"),
            outside: (GRANTED_BUCKET, "granted/other.txt"),
        },
        ScopeRow {
            case: "one object key, under the Minimal privilege",
            location_scope: "s3://ledger/granted/",
            sub_prefix: "*",
            target: "s3://ledger/granted/inside.txt",
            privilege: Some(PRIVILEGE_MINIMAL),
            matched: "s3://ledger/granted/inside.txt",
            inside: (GRANTED_BUCKET, "granted/inside.txt"),
            outside: (GRANTED_BUCKET, "granted/other.txt"),
        },
    ];
    for row in rows {
        let case = row.case;
        let (store, _engine, role_arn) = granted_data(case).await;
        create_instance(case, &store).await;
        let location_id = create_location(case, &store, row.location_scope, &role_arn).await;
        create_grant(case, &store, &location_id, row.sub_prefix, PERMISSION_READ).await;

        let reply = get_data_access(&store, row.target, PERMISSION_READ, row.privilege).await;
        let matched = reply.text("MatchedGrantTarget").unwrap_or_default();
        assert_eq!(
            matched, row.matched,
            "case {case:?}: expected the token to carry the scope {:?}, got {matched:?}",
            row.matched
        );
        let vended = vend(case, &store, row.target, PERMISSION_READ, row.privilege).await;
        let principal = session_arn(case, &store, &vended).await;

        let (bucket, key) = row.inside;
        let (status, body) = s3_as(&store, &vended, "GET", &format!("/{bucket}/{key}"), b"").await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the read of {bucket}/{key} inside the scope to be served, \
             got {status} with body {body}"
        );
        let expected = String::from_utf8(content(bucket, key)).expect("UTF-8 content");
        assert_eq!(
            body, expected,
            "case {case:?}: expected the object's own bytes {expected:?}, got {body:?}"
        );

        let (bucket, key) = row.outside;
        let (status, body) = s3_as(&store, &vended, "GET", &format!("/{bucket}/{key}"), b"").await;
        assert_eq!(
            status, 403,
            "case {case:?}: expected the read of {bucket}/{key} outside the scope to be \
             refused 403, got {status} with body {body}"
        );
        let code = elements(&body, "Code").into_iter().next().unwrap_or_default();
        assert_eq!(
            code, "AccessDenied",
            "case {case:?}: expected the refusal code AccessDenied, got {code:?} in {body}"
        );
        let answered = message(case, &body);
        let expected = expected_refusal(
            &principal,
            "s3:GetObject",
            &object_resource(bucket, key),
        );
        assert_eq!(
            answered, expected,
            "case {case:?}: expected the refusal {expected:?}, got {answered:?}"
        );
    }
}

/// One row of the permission table: the permission the vend asks for, and what the
/// credential may then do.
struct PermissionRow {
    case: &'static str,
    permission: &'static str,
    /// Whether a read of a key under the scope is served.
    reads: bool,
    /// Whether a write of a key under the scope is served.
    writes: bool,
}

/// The permission a vend asks for decides which actions the credential may perform on the
/// data its scope covers: a read-only credential is refused a write of a key it may read,
/// and a write-only one is refused the read — each in the same sentence, naming the action
/// attempted.
#[tokio::test]
async fn the_vended_permission_decides_which_actions_the_credential_may_perform() {
    let rows = [
        PermissionRow {
            case: "READ",
            permission: PERMISSION_READ,
            reads: true,
            writes: false,
        },
        PermissionRow {
            case: "WRITE",
            permission: PERMISSION_WRITE,
            reads: false,
            writes: true,
        },
        PermissionRow {
            case: "READWRITE",
            permission: PERMISSION_READWRITE,
            reads: true,
            writes: true,
        },
    ];
    let read_key = "granted/inside.txt";
    let write_key = "granted/written.txt";
    for row in rows {
        let case = row.case;
        let (store, _engine, role_arn) = granted_data(case).await;
        create_instance(case, &store).await;
        let location_id = create_location(case, &store, "s3://ledger/granted/", &role_arn).await;
        // The GRANT covers both halves, so only the permission the VEND asks for can
        // narrow what the credential may do.
        create_grant(case, &store, &location_id, "*", PERMISSION_READWRITE).await;
        let vended = vend(case, &store, "s3://ledger/granted/*", row.permission, None).await;
        let principal = session_arn(case, &store, &vended).await;

        let (status, body) = s3_as(
            &store,
            &vended,
            "GET",
            &format!("/{GRANTED_BUCKET}/{read_key}"),
            b"",
        )
        .await;
        if row.reads {
            assert_eq!(
                status, 200,
                "case {case:?}: expected a read to be served, got {status} with body {body}"
            );
        } else {
            assert_eq!(
                status, 403,
                "case {case:?}: expected a read to be refused 403, got {status} with body {body}"
            );
            let answered = message(case, &body);
            let expected = expected_refusal(
                &principal,
                "s3:GetObject",
                &object_resource(GRANTED_BUCKET, read_key),
            );
            assert_eq!(
                answered, expected,
                "case {case:?}: expected the refusal {expected:?}, got {answered:?}"
            );
        }

        let (status, body) = s3_as(
            &store,
            &vended,
            "PUT",
            &format!("/{GRANTED_BUCKET}/{write_key}"),
            b"written under the vended credential",
        )
        .await;
        if row.writes {
            assert_eq!(
                status, 200,
                "case {case:?}: expected a write to be served, got {status} with body {body}"
            );
        } else {
            assert_eq!(
                status, 403,
                "case {case:?}: expected a write to be refused 403, got {status} with body {body}"
            );
            let answered = message(case, &body);
            let expected = expected_refusal(
                &principal,
                "s3:PutObject",
                &object_resource(GRANTED_BUCKET, write_key),
            );
            assert_eq!(
                answered, expected,
                "case {case:?}: expected the refusal {expected:?}, got {answered:?}"
            );
        }
    }
}

/// One row of the listing table: the scope the token carries, and which enumerations of the
/// bucket the credential may make.
struct ListingRow {
    case: &'static str,
    location_scope: &'static str,
    sub_prefix: &'static str,
    target: &'static str,
    /// The `prefix` parameter the admitted listing names, and the keys it must report.
    admitted_prefix: &'static str,
    admitted_keys: &'static [&'static str],
    /// Whether an enumeration of the WHOLE bucket — a listing naming no prefix at all — is
    /// admitted: only a scope that covers every key of the bucket confines nothing.
    whole_bucket: bool,
}

/// Enumerating the bucket under a vended credential is confined to the key space its scope
/// covers: a listing of the granted prefix is served, and one that asks for the whole bucket
/// is refused unless the scope covers the whole bucket.
#[tokio::test]
async fn a_vended_credential_enumerates_only_the_key_space_its_scope_covers() {
    let rows = [
        ListingRow {
            case: "a prefix-scoped token",
            location_scope: "s3://ledger/granted/",
            sub_prefix: "*",
            target: "s3://ledger/granted/*",
            admitted_prefix: "granted/",
            admitted_keys: &[
                "granted/deep/inside.txt",
                "granted/inside.txt",
                "granted/other.txt",
            ],
            whole_bucket: false,
        },
        ListingRow {
            case: "a bucket-scoped token",
            location_scope: "s3://",
            sub_prefix: "ledger/*",
            target: "s3://ledger/*",
            admitted_prefix: "granted/",
            admitted_keys: &[
                "granted/deep/inside.txt",
                "granted/inside.txt",
                "granted/other.txt",
            ],
            whole_bucket: true,
        },
    ];
    for row in rows {
        let case = row.case;
        let (store, _engine, role_arn) = granted_data(case).await;
        create_instance(case, &store).await;
        let location_id = create_location(case, &store, row.location_scope, &role_arn).await;
        create_grant(case, &store, &location_id, row.sub_prefix, PERMISSION_READ).await;
        let vended = vend(case, &store, row.target, PERMISSION_READ, None).await;
        let principal = session_arn(case, &store, &vended).await;

        let (status, body) = s3_as(
            &store,
            &vended,
            "GET",
            &format!(
                "/{GRANTED_BUCKET}?list-type=2&prefix={}",
                query_escape(row.admitted_prefix)
            ),
            b"",
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the listing of {} to be served, got {status} with body \
             {body}",
            row.admitted_prefix
        );
        let keys = elements(&body, "Key");
        assert_eq!(
            keys, row.admitted_keys,
            "case {case:?}: expected the keys {:?} under {}, got {keys:?}",
            row.admitted_keys, row.admitted_prefix
        );

        let (status, body) = s3_as(
            &store,
            &vended,
            "GET",
            &format!("/{GRANTED_BUCKET}?list-type=2"),
            b"",
        )
        .await;
        if row.whole_bucket {
            assert_eq!(
                status, 200,
                "case {case:?}: expected a token covering the whole bucket to enumerate it, \
                 got {status} with body {body}"
            );
        } else {
            assert_eq!(
                status, 403,
                "case {case:?}: expected the enumeration of the whole bucket to be refused \
                 403, got {status} with body {body}"
            );
            let answered = message(case, &body);
            let expected = expected_refusal(
                &principal,
                "s3:ListBucket",
                &format!("arn:aws:s3:::{GRANTED_BUCKET}"),
            );
            assert_eq!(
                answered, expected,
                "case {case:?}: expected the refusal {expected:?}, got {answered:?}"
            );
        }
    }
}

/// Two vends are two sessions: each carries its own credential and its own identity, so a
/// refusal of a request one signed names that session and no other.
#[tokio::test]
async fn each_vend_mints_a_session_of_its_own() {
    let case = "two vends of one grant";
    let (store, _engine, role_arn) = granted_data(case).await;
    create_instance(case, &store).await;
    let location_id = create_location(case, &store, "s3://ledger/granted/", &role_arn).await;
    create_grant(case, &store, &location_id, "*", PERMISSION_READ).await;

    let first = vend(case, &store, "s3://ledger/granted/*", PERMISSION_READ, None).await;
    let second = vend(case, &store, "s3://ledger/granted/*", PERMISSION_READ, None).await;
    assert_ne!(
        first.access_key_id, second.access_key_id,
        "case {case:?}: expected a credential of its own per vend, got {} twice",
        first.access_key_id
    );
    let first_arn = session_arn(case, &store, &first).await;
    let second_arn = session_arn(case, &store, &second).await;
    assert_ne!(
        first_arn, second_arn,
        "case {case:?}: expected a session of its own per vend, got {first_arn} twice"
    );
    // Both are sessions of the location's role, which is the identity whose policies the
    // scope is the intersection with.
    for (label, arn) in [("the first vend", &first_arn), ("the second vend", &second_arn)] {
        assert!(
            arn.contains(&format!("assumed-role/{LOCATION_ROLE_NAME}/")),
            "case {label:?}: expected a session of {LOCATION_ROLE_NAME}, got {arn}"
        );
    }
}
