// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for what AUTHORIZES a request this service serves for a caller that is
//! an IAM IDENTITY of the account rather than the account's own credential.
//!
//! Every assertion drives real HTTP requests through a router holding S3 beside IAM over one
//! shared engine, so the identity a request speaks for, the policies in force on it and the
//! wire answer are all the real ones: the user, its access key and its policies are created
//! through IAM's own API, and each S3 request is presented with that user's access key id in
//! its `Authorization` header — which is where the emulator reads the signing identity from.
//!
//! The rules under test:
//!
//! * A signed request is authorized PER ACTION against the identity-based documents in force
//!   on the principal that signed it and against the bucket's own policy: an explicit `Deny`
//!   in either refuses it, otherwise it needs an `Allow` from one of them, and with neither
//!   it is refused 403 `AccessDenied` naming the principal, the action and the resource
//!   ("Determining whether a request is allowed or denied within an account", IAM's own
//!   evaluation order).
//! * WHICH action a request asks for is decided by the operation AND by the request's own
//!   shape: a read naming a `versionId` asks for `s3:GetObjectVersion` rather than
//!   `s3:GetObject`, a delete naming one asks for `s3:DeleteObjectVersion` rather than
//!   `s3:DeleteObject`, a listing of versions asks for `s3:ListBucketVersions` rather than
//!   `s3:ListBucket`, and a read of the default-encryption configuration asks for
//!   `s3:GetEncryptionConfiguration`, which the `s3:GetBucket*` family does not cover.
//! * A read of a key the bucket holds nothing at is answered by what the caller may learn
//!   about the bucket's contents at all: 404 `NoSuchKey` for a caller holding `s3:ListBucket`
//!   and 403 `AccessDenied` for one that does not, so existence does not leak.
//! * A bucket policy's `Condition` is decided against the request's own members, so a grant
//!   capped by `s3:max-keys` admits a listing up to the cap and refuses a larger one.
//! * `PutBucketPolicy` resolves the identities its document grants to: a user or role this
//!   account does not hold, and an IAM GROUP whether or not it exists, are refused 400
//!   `MalformedPolicy` / `Invalid principal in policy`, and the stored policy is unchanged.
//! * With `BlockPublicPolicy` on, a wildcard-principal grant whose only restriction is a
//!   wildcard-VALUED condition is still public and is refused; one restricted to fixed values
//!   is accepted.
//!
//! Each table's rows differ along what its rule generalises over: the operation, the shape of
//! the request (naming a version or not, the size of the listing), where the grant comes from
//! (the identity's documents or the bucket policy), which principal spelling it reaches the
//! caller by, and which principal form a stored document names.

use std::sync::Arc;

use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::{create_bucket, get_bucket_policy, put_bucket_policy, Answer};
use super::bucket_public_access_block::allow_public_access;
use super::object_versioning::set_versioning;

/// The bytes every object in this suite holds.
const BODY: &[u8] = b"identity-authorized-subject";

/// The key every row addresses, which each row's setup WRITES twice so the key has an older
/// version beside its current one.
const HELD_KEY: &str = "reports/held.csv";

/// A key no setup writes, so a read of it resolves no version at all.
const ABSENT_KEY: &str = "reports/never-written.csv";

/// The code S3 answers an authorization refusal with.
const ACCESS_DENIED: &str = "AccessDenied";

/// The code and message a read of a key the bucket holds nothing at answers with.
const NO_SUCH_KEY: &str = "NoSuchKey";

/// The code and message `PutBucketPolicy` refuses a document naming an identity it cannot
/// resolve as a grantee with. Written out here rather than imported, so the wording has
/// something to be wrong about.
const MALFORMED_POLICY: &str = "MalformedPolicy";
/// See [`MALFORMED_POLICY`].
const INVALID_PRINCIPAL: &str = "Invalid principal in policy";

/// A router serving S3 beside IAM over ONE shared engine, which is what lets a request
/// signed with a key `iam:CreateAccessKey` vended be judged on the policies
/// `iam:PutUserPolicy` wrote.
///
/// `pub(crate)`, like the three helpers below it ([`user_with_grant`], [`s3`], [`user_arn`]):
/// the suite that pins the identity-OR-resource UNION ([`super::object_read_policy_union`])
/// drives its callers through this one harness rather than standing up a second one.
pub(crate) fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Neither the key id nor the signature is a credential here: the suite turns request
    // authentication off, exactly as the other suites of this crate do, so the access key id
    // on the wire is read as the signing identity and nothing is verified.
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(engine),
    );
    router
}

/// One awsQuery call to IAM, answered with the HTTP status and the XML body.
///
/// `pub(crate)`: the suite that pins the replication role a write is copied under
/// ([`super::replication_role_write_paths`]) makes its role and its trust policy through
/// IAM's own API over this same harness rather than standing up a second one.
pub(crate) async fn iam(
    router: &MultiServiceRouter,
    action: &str,
    params: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version=2010-05-08");
    for (name, value) in params {
        body.push('&');
        body.push_str(&form_encode(name));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = ::http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", "iam.amazonaws.com")
        .header("content-type", "application/x-www-form-urlencoded")
        .body(bytes::Bytes::from(body))
        .expect("a well-formed IAM request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let body = String::from_utf8_lossy(response.body()).to_string();
    (status, body)
}

/// Percent-encode one `application/x-www-form-urlencoded` component, through the crate this
/// service already percent-encodes with rather than by hand.
///
/// `pub(crate)`: every suite of this test binary that drives an awsQuery service needs the
/// same encoding — and an access point ARN travelling as ONE path segment needs it too
/// ([`super::access_point_data_plane`]) — so there is one encoder here rather than a copy per
/// suite.
pub(crate) fn form_encode(value: &str) -> String {
    ::percent_encoding::utf8_percent_encode(value, ::percent_encoding::NON_ALPHANUMERIC)
        .to_string()
}

/// The text of the single `<tag>` element of an XML body.
///
/// `pub(crate)`, as [`form_encode`] above and for the same reason: the suite that pins what
/// authorizes the account's own listings ([`super::account_wide_listing_authorization`]) reads
/// a refusal's `<Code>` and `<Message>` through this one reader rather than copying it.
pub(crate) fn element(body: &str, tag: &str) -> String {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let (_, rest) = body
        .split_once(&open)
        .unwrap_or_else(|| panic!("the response must carry <{tag}>, got {body}"));
    let (text, _) = rest
        .split_once(&close)
        .unwrap_or_else(|| panic!("<{tag}> must be closed, got {body}"));
    text.to_string()
}

/// An IAM user carrying exactly the inline policy `actions` on `resources` describes, and the
/// access key id a request speaks for it with. `actions` empty leaves the user with no policy
/// at all, which is the position every unprivileged principal is in.
pub(crate) async fn user_with_grant(
    router: &MultiServiceRouter,
    name: &str,
    actions: &[&str],
    resources: &[String],
) -> String {
    let (status, body) = iam(router, "CreateUser", &[("UserName", name)]).await;
    assert_eq!(status, 200, "the user {name:?} must be created, got {body}");
    if !actions.is_empty() {
        let document = identity_policy(actions, resources);
        let (status, body) = iam(
            router,
            "PutUserPolicy",
            &[
                ("UserName", name),
                ("PolicyName", "granted"),
                ("PolicyDocument", &document),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "the policy of {name:?} must be stored, got {body}"
        );
    }
    let (status, body) = iam(router, "CreateAccessKey", &[("UserName", name)]).await;
    assert_eq!(
        status, 200,
        "an access key for {name:?} must be vended, got {body}"
    );
    element(&body, "AccessKeyId")
}

/// An identity-based document allowing `actions` on `resources` — the policies an IAM
/// identity carries name no principal, because the policy is attached to one.
fn identity_policy(actions: &[&str], resources: &[String]) -> String {
    format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Effect\":\"Allow\",\
         \"Action\":{},\"Resource\":{}}}]}}",
        json_strings(actions),
        json_strings(&resources.iter().map(String::as_str).collect::<Vec<&str>>())
    )
}

/// Store one more inline policy `document` on the user `name`, as `iam:PutUserPolicy` does,
/// answering whether IAM stored it.
///
/// `pub(crate)`: a sibling suite that needs a SECOND document in force on one principal — a
/// `Deny` beside a grant ([`super::account_wide_listing_authorization`]) — writes it through
/// this one harness rather than standing up a second IAM caller.
pub(crate) async fn put_user_policy(
    router: &MultiServiceRouter,
    name: &str,
    policy_name: &str,
    document: &str,
) -> bool {
    let (status, _) = iam(
        router,
        "PutUserPolicy",
        &[
            ("UserName", name),
            ("PolicyName", policy_name),
            ("PolicyDocument", document),
        ],
    )
    .await;
    status == 200
}

/// `values` as a JSON array of strings.
fn json_strings(values: &[&str]) -> String {
    let quoted: Vec<String> = values.iter().map(|value| format!("\"{value}\"")).collect();
    format!("[{}]", quoted.join(","))
}

/// One S3 request, presented as signed by `akid` when one is given.
pub(crate) async fn s3(
    router: &MultiServiceRouter,
    akid: Option<&str>,
    method: &str,
    uri: &str,
    body: &[u8],
) -> Answer {
    let host = "s3.us-east-1.amazonaws.com";
    let mut builder = ::http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{uri}"))
        .header("host", host)
        .header("content-type", "application/octet-stream");
    if let Some(akid) = akid {
        builder = builder.header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential={akid}/20260101/us-east-1/s3/aws4_request, \
                 SignedHeaders=host, Signature=not-verified"
            ),
        );
    }
    let request = builder
        .body(bytes::Bytes::copy_from_slice(body))
        .expect("the test request must build");
    let response = router.dispatch(request).await;
    let (parts, body) = response.into_parts();
    Answer {
        status: parts.status.as_u16(),
        headers: parts.headers,
        body,
    }
}

/// The ARNs a grant for one bucket names: the bucket itself, for the bucket-scoped actions,
/// and every key in it, for the object-scoped ones.
fn bucket_resources(bucket: &str) -> Vec<String> {
    vec![
        format!("arn:aws:s3:::{bucket}"),
        format!("arn:aws:s3:::{bucket}/*"),
    ]
}

/// The ARN of the IAM user `name` of this account, the spelling a policy names it by.
pub(crate) fn user_arn(name: &str) -> String {
    format!("arn:aws:iam::{}:user/{name}", engine::emulator_account_id())
}

/// The sentence S3 refuses a request no document in force allows with: the principal that
/// signed it, the action it asked for, the resource it addressed and IAM's reason clause for
/// an absent grant. Written out here rather than built from the service's own helper.
///
/// `pub(crate)`: the suite that pins the identity-OR-resource UNION
/// ([`super::object_read_policy_union`]) asserts the same sentence, and one spelling of it
/// keeps the two suites from drifting on the wording.
pub(crate) fn no_grant_message(principal: &str, action: &str, resource: &str) -> String {
    format!(
        "User: {principal} is not authorized to perform: {action} on resource: \"{resource}\" \
         because no identity-based policy allows the {action} action"
    )
}

/// A bucket with versioning enabled holding [`HELD_KEY`] twice over, and the version id of
/// the OLDER of the two writes — the version a read or a delete names explicitly.
async fn seeded_bucket(router: &MultiServiceRouter, bucket: &str) -> String {
    let created = create_bucket(router, bucket, None, None).await;
    assert_eq!(
        created.status, 200,
        "the bucket {bucket:?} must be created, got {}",
        created.status
    );
    set_versioning(router, bucket, "Enabled").await;
    let first = s3(router, None, "PUT", &format!("/{bucket}/{HELD_KEY}"), BODY).await;
    assert_eq!(
        first.status, 200,
        "the first write must be stored, got {}",
        first.status
    );
    let older = first
        .header("x-amz-version-id")
        .expect("a versioning-enabled bucket mints a version id");
    let second = s3(router, None, "PUT", &format!("/{bucket}/{HELD_KEY}"), BODY).await;
    assert_eq!(
        second.status, 200,
        "the second write must be stored, got {}",
        second.status
    );
    older
}

/// What an operation asks for, as the request's own shape decides it.
#[derive(Debug, Clone, Copy)]
enum Call {
    /// `GetObject` of the key's current version.
    Read,
    /// `GetObject` naming the older version's id.
    ReadVersion,
    /// `GetObject` of a key the bucket holds nothing at.
    ReadAbsent,
    /// `ListObjectsV2`, with the `max-keys` the row names.
    List(Option<u32>),
    /// `ListObjectVersions`.
    ListVersions,
    /// `PutObject`.
    Write,
    /// `DeleteObject` naming no version.
    Delete,
    /// `DeleteObject` naming the older version's id.
    DeleteVersion,
    /// `GetBucketEncryption`.
    ReadEncryption,
    /// `HeadBucket`, whose answer is its status alone.
    HeadBucket,
}

/// What the row's call must answer.
#[derive(Debug, Clone)]
enum Expected {
    /// Served, with this status.
    Served(u16),
    /// Refused 403 `AccessDenied` because no document in force allows `action` on the
    /// resource this row addresses.
    NoGrant { action: &'static str, object: bool },
    /// Refused 403 `AccessDenied` with no body at all, which is a HEAD's whole answer.
    Forbidden,
    /// Served as far as the key, which is not there.
    MissingKey,
}

/// One row of the action-mapping table.
struct Row {
    case: &'static str,
    /// The actions the row's caller is granted by its own identity policy.
    granted: &'static [&'static str],
    call: Call,
    expected: Expected,
}

/// The table: one row per (grant, request shape) pair the rule distinguishes.
const ROWS: &[Row] = &[
    Row {
        case: "a read with no document in force at all",
        granted: &[],
        call: Call::Read,
        expected: Expected::NoGrant {
            action: "s3:GetObject",
            object: true,
        },
    },
    Row {
        case: "a read granted the read",
        granted: &["s3:GetObject"],
        call: Call::Read,
        expected: Expected::Served(200),
    },
    Row {
        case: "a read granted every S3 action",
        granted: &["s3:*"],
        call: Call::Read,
        expected: Expected::Served(200),
    },
    Row {
        case: "a read granted only the write",
        granted: &["s3:PutObject"],
        call: Call::Read,
        expected: Expected::NoGrant {
            action: "s3:GetObject",
            object: true,
        },
    },
    Row {
        case: "a read naming a version, granted only the unversioned read",
        granted: &["s3:GetObject"],
        call: Call::ReadVersion,
        expected: Expected::NoGrant {
            action: "s3:GetObjectVersion",
            object: true,
        },
    },
    Row {
        case: "a read naming a version, granted the version-scoped read",
        granted: &["s3:GetObjectVersion"],
        call: Call::ReadVersion,
        expected: Expected::Served(200),
    },
    Row {
        case: "an absent key, granted the read but not the listing",
        granted: &["s3:GetObject"],
        call: Call::ReadAbsent,
        expected: Expected::NoGrant {
            action: "s3:ListBucket",
            object: false,
        },
    },
    Row {
        case: "an absent key, granted the listing too",
        granted: &["s3:GetObject", "s3:ListBucket"],
        call: Call::ReadAbsent,
        expected: Expected::MissingKey,
    },
    Row {
        case: "a listing granted only the read",
        granted: &["s3:GetObject"],
        call: Call::List(None),
        expected: Expected::NoGrant {
            action: "s3:ListBucket",
            object: false,
        },
    },
    Row {
        case: "a listing granted the listing",
        granted: &["s3:ListBucket"],
        call: Call::List(None),
        expected: Expected::Served(200),
    },
    Row {
        case: "a version listing granted only the object listing",
        granted: &["s3:ListBucket"],
        call: Call::ListVersions,
        expected: Expected::NoGrant {
            action: "s3:ListBucketVersions",
            object: false,
        },
    },
    Row {
        case: "a version listing granted the version listing",
        granted: &["s3:ListBucketVersions"],
        call: Call::ListVersions,
        expected: Expected::Served(200),
    },
    Row {
        case: "a write granted only the read",
        granted: &["s3:GetObject"],
        call: Call::Write,
        expected: Expected::NoGrant {
            action: "s3:PutObject",
            object: true,
        },
    },
    Row {
        case: "a write granted the write",
        granted: &["s3:PutObject"],
        call: Call::Write,
        expected: Expected::Served(200),
    },
    Row {
        case: "a delete naming no version, granted the plain delete",
        granted: &["s3:DeleteObject"],
        call: Call::Delete,
        expected: Expected::Served(204),
    },
    Row {
        case: "a delete naming no version, with no grant",
        granted: &[],
        call: Call::Delete,
        expected: Expected::NoGrant {
            action: "s3:DeleteObject",
            object: true,
        },
    },
    Row {
        case: "a delete naming a version, granted only the plain delete",
        granted: &["s3:DeleteObject"],
        call: Call::DeleteVersion,
        expected: Expected::NoGrant {
            action: "s3:DeleteObjectVersion",
            object: true,
        },
    },
    Row {
        case: "a delete naming a version, granted the version-scoped delete",
        granted: &["s3:DeleteObjectVersion"],
        call: Call::DeleteVersion,
        expected: Expected::Served(204),
    },
    Row {
        case: "the encryption configuration, granted the s3:GetBucket* family",
        granted: &["s3:GetBucket*"],
        call: Call::ReadEncryption,
        expected: Expected::NoGrant {
            action: "s3:GetEncryptionConfiguration",
            object: false,
        },
    },
    Row {
        case: "the encryption configuration, granted its own action",
        granted: &["s3:GetEncryptionConfiguration"],
        call: Call::ReadEncryption,
        expected: Expected::Served(200),
    },
    Row {
        case: "a bucket HEAD with no grant",
        granted: &[],
        call: Call::HeadBucket,
        expected: Expected::Forbidden,
    },
    Row {
        case: "a bucket HEAD granted the listing",
        granted: &["s3:ListBucket"],
        call: Call::HeadBucket,
        expected: Expected::Served(200),
    },
];

/// Drive one row's call against `bucket`, as the caller `akid` speaks for.
async fn drive(
    router: &MultiServiceRouter,
    akid: &str,
    bucket: &str,
    older_version: &str,
    call: Call,
) -> Answer {
    let akid = Some(akid);
    match call {
        Call::Read => s3(router, akid, "GET", &format!("/{bucket}/{HELD_KEY}"), b"").await,
        Call::ReadVersion => {
            s3(
                router,
                akid,
                "GET",
                &format!("/{bucket}/{HELD_KEY}?versionId={older_version}"),
                b"",
            )
            .await
        }
        Call::ReadAbsent => s3(router, akid, "GET", &format!("/{bucket}/{ABSENT_KEY}"), b"").await,
        Call::List(max_keys) => {
            let query = match max_keys {
                Some(max_keys) => format!("&max-keys={max_keys}"),
                None => String::new(),
            };
            s3(
                router,
                akid,
                "GET",
                &format!("/{bucket}?list-type=2{query}"),
                b"",
            )
            .await
        }
        Call::ListVersions => s3(router, akid, "GET", &format!("/{bucket}?versions"), b"").await,
        Call::Write => {
            s3(
                router,
                akid,
                "PUT",
                &format!("/{bucket}/{HELD_KEY}"),
                b"written-by-the-row",
            )
            .await
        }
        Call::Delete => s3(router, akid, "DELETE", &format!("/{bucket}/{HELD_KEY}"), b"").await,
        Call::DeleteVersion => {
            s3(
                router,
                akid,
                "DELETE",
                &format!("/{bucket}/{HELD_KEY}?versionId={older_version}"),
                b"",
            )
            .await
        }
        Call::ReadEncryption => s3(router, akid, "GET", &format!("/{bucket}?encryption"), b"").await,
        Call::HeadBucket => s3(router, akid, "HEAD", &format!("/{bucket}"), b"").await,
    }
}

/// RULE: a signed request needs a grant for the action THAT request asks for, and which
/// action that is follows the operation and the request's own shape.
#[tokio::test]
async fn the_action_a_request_asks_for_decides_the_grant_it_needs() {
    let router = router();
    for (index, row) in ROWS.iter().enumerate() {
        // Each row gets its own bucket and its own caller, so no row's grant or mutation can
        // decide another's answer.
        let bucket = format!("identity-authz-{index}");
        let older_version = seeded_bucket(&router, &bucket).await;
        let name = format!("identity-authz-user-{index}");
        let akid = user_with_grant(&router, &name, row.granted, &bucket_resources(&bucket)).await;

        let answer = drive(&router, &akid, &bucket, &older_version, row.call).await;
        let body = String::from_utf8_lossy(&answer.body).to_string();
        match &row.expected {
            Expected::Served(status) => assert_eq!(
                answer.status, *status,
                "case {:?}: expected the call to be served with {status}, got {} and body {body:?}",
                row.case, answer.status
            ),
            Expected::Forbidden => assert_eq!(
                answer.status, 403,
                "case {:?}: expected 403, got {} and body {body:?}",
                row.case, answer.status
            ),
            Expected::MissingKey => {
                assert_eq!(
                    (answer.status, answer.error_code().as_str()),
                    (404, NO_SUCH_KEY),
                    "case {:?}: expected the absent key to be reported, got {} and body {body:?}",
                    row.case,
                    answer.status
                );
            }
            Expected::NoGrant { action, object } => {
                let resource = match object {
                    true => format!("arn:aws:s3:::{bucket}/{}", match row.call {
                        Call::ReadAbsent => ABSENT_KEY,
                        _ => HELD_KEY,
                    }),
                    false => format!("arn:aws:s3:::{bucket}"),
                };
                assert_eq!(
                    (
                        answer.status,
                        answer.error_code(),
                        answer.error_message()
                    ),
                    (
                        403,
                        ACCESS_DENIED.to_string(),
                        no_grant_message(&user_arn(&name), action, &resource)
                    ),
                    "case {:?}: expected the refusal to name the principal, {action} and \
                     {resource}, got {} and body {body:?}",
                    row.case,
                    answer.status
                );
            }
        }
    }
}

/// One row of the bucket-policy table: the case, the actions the caller's own policy grants,
/// the statement the bucket policy carries (built from the caller's ARN and the resource) and
/// whether the read must be served.
type PolicyRow<'a> = (
    &'a str,
    &'a [&'a str],
    &'a dyn Fn(&str, &str) -> String,
    bool,
);

/// RULE: a grant in the BUCKET's own policy that names the caller admits the request on its
/// own, a grant naming another principal does not, and an explicit deny in either kind of
/// document refuses whatever the other grants.
#[tokio::test]
async fn a_bucket_policy_grant_names_the_caller_and_a_deny_overrides_every_grant() {
    let router = router();
    let account_root = format!("arn:aws:iam::{}:root", engine::emulator_account_id());
    // The other principal one row's grant names: a user the account really holds, since a
    // document naming one it does not hold is refused before it is ever stored.
    let (status, body) = iam(
        &router,
        "CreateUser",
        &[("UserName", "identity-authz-somebody-else")],
    )
    .await;
    assert_eq!(status, 200, "the other user must be created, got {body}");
    // (case, the actions the caller's own policy grants, the bucket-policy statements, served?)
    let rows: [PolicyRow<'_>; 6] = [
        (
            "a bucket-policy grant naming the caller, with no identity policy",
            &[],
            &|principal, bucket| statement("Allow", principal, "s3:GetObject", bucket),
            true,
        ),
        (
            "a bucket-policy grant naming another principal",
            &[],
            &|_principal, bucket| {
                statement(
                    "Allow",
                    &user_arn("identity-authz-somebody-else"),
                    "s3:GetObject",
                    bucket,
                )
            },
            false,
        ),
        (
            "a bucket-policy grant for another action",
            &[],
            &|principal, bucket| statement("Allow", principal, "s3:PutObject", bucket),
            false,
        ),
        (
            "an identity grant with a bucket-policy deny naming the caller",
            &["s3:*"],
            &|principal, bucket| statement("Deny", principal, "s3:GetObject", bucket),
            false,
        ),
        (
            "an identity grant with a bucket-policy deny naming the account",
            &["s3:*"],
            &|_principal, bucket| statement("Deny", &account_root, "s3:GetObject", bucket),
            false,
        ),
        (
            "an identity grant with a bucket-policy grant beside it",
            &["s3:GetObject"],
            &|principal, bucket| statement("Allow", principal, "s3:GetObject", bucket),
            true,
        ),
    ];
    for (index, (case, granted, policy, served)) in rows.into_iter().enumerate() {
        let bucket = format!("identity-authz-policy-{index}");
        seeded_bucket(&router, &bucket).await;
        let name = format!("identity-authz-policy-user-{index}");
        let akid = user_with_grant(&router, &name, granted, &bucket_resources(&bucket)).await;
        let document = format!(
            "{{\"Version\":\"2012-10-17\",\"Statement\":[{}]}}",
            policy(&user_arn(&name), &format!("arn:aws:s3:::{bucket}/*"))
        );
        let put = put_bucket_policy(&router, &bucket, &document).await;
        assert_eq!(
            put.status,
            204,
            "case {case:?}: the bucket policy must be stored, got {} and body {:?}",
            put.status,
            String::from_utf8_lossy(&put.body)
        );

        let answer = s3(
            &router,
            Some(&akid),
            "GET",
            &format!("/{bucket}/{HELD_KEY}"),
            b"",
        )
        .await;
        assert_eq!(
            answer.status == 200,
            served,
            "case {case:?}: expected served={served}, got {} and body {:?}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        if !served {
            assert_eq!(
                answer.error_code(),
                ACCESS_DENIED,
                "case {case:?}: expected the authorization code, got body {:?}",
                String::from_utf8_lossy(&answer.body)
            );
        }
    }
}

/// One statement of a resource policy, naming one principal.
///
/// `pub(crate)`, like the harness above: the union suite
/// ([`super::object_read_policy_union`]) writes its bucket policies through this one builder.
pub(crate) fn statement(effect: &str, principal: &str, action: &str, resource: &str) -> String {
    format!(
        "{{\"Effect\":\"{effect}\",\"Principal\":{{\"AWS\":\"{principal}\"}},\
         \"Action\":\"{action}\",\"Resource\":\"{resource}\"}}"
    )
}

/// RULE: a bucket-policy `Condition` is decided against the request's own members, so a grant
/// capped by `s3:max-keys` admits a listing up to the cap and refuses every larger one —
/// including one that names no `max-keys` at all, which returns up to 1,000 keys.
#[tokio::test]
async fn a_max_keys_condition_admits_a_capped_listing_and_refuses_a_larger_one() {
    let router = router();
    let bucket = "identity-authz-max-keys";
    seeded_bucket(&router, bucket).await;
    let akid = user_with_grant(
        &router,
        "identity-authz-max-keys-user",
        &[],
        &bucket_resources(bucket),
    )
    .await;
    let cap = 10;
    let document = format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Effect\":\"Allow\",\
         \"Principal\":{{\"AWS\":\"{}\"}},\"Action\":\"s3:ListBucket\",\
         \"Resource\":\"arn:aws:s3:::{bucket}\",\"Condition\":\
         {{\"NumericLessThanEquals\":{{\"s3:max-keys\":\"{cap}\"}}}}}}]}}",
        user_arn("identity-authz-max-keys-user")
    );
    let put = put_bucket_policy(&router, bucket, &document).await;
    assert_eq!(
        put.status,
        204,
        "the capped grant must be stored, got {} and body {:?}",
        put.status,
        String::from_utf8_lossy(&put.body)
    );

    // (case, the max-keys the listing asks for, served?)
    let rows: [(&str, Option<u32>, bool); 5] = [
        ("one key, far below the cap", Some(1), true),
        ("exactly the cap", Some(cap), true),
        ("one above the cap", Some(cap + 1), false),
        ("far above the cap", Some(100), false),
        ("no max-keys at all, which lists up to 1,000", None, false),
    ];
    for (case, max_keys, served) in rows {
        let answer = drive(&router, &akid, bucket, "", Call::List(max_keys)).await;
        assert_eq!(
            answer.status == 200,
            served,
            "case {case:?}: expected served={served}, got {} and body {:?}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        if !served {
            assert_eq!(
                (answer.status, answer.error_code()),
                (403, ACCESS_DENIED.to_string()),
                "case {case:?}: expected the authorization refusal, got body {:?}",
                String::from_utf8_lossy(&answer.body)
            );
        }
    }
}

/// RULE: `PutBucketPolicy` resolves every identity its document grants to — a user or role
/// this account does not hold is refused, and an IAM group is refused whether or not the
/// account holds it — and a refused write leaves the stored policy exactly as it was.
#[tokio::test]
async fn a_document_naming_an_unresolvable_principal_is_refused_whole() {
    let router = router();
    let bucket = "identity-authz-principals";
    let created = create_bucket(&router, bucket, None, None).await;
    assert_eq!(created.status, 200, "the bucket must be created");
    // Some rows name the wildcard, which Block Public Access would refuse for being public;
    // this test is about whether the PRINCIPAL resolves.
    allow_public_access(&router, bucket).await;
    // The identities the account really holds, created through IAM's own API.
    let (status, body) = iam(&router, "CreateUser", &[("UserName", "held-reader")]).await;
    assert_eq!(status, 200, "the user must be created, got {body}");
    let (status, body) = iam(&router, "CreateGroup", &[("GroupName", "held-readers")]).await;
    assert_eq!(status, 200, "the group must be created, got {body}");
    let (status, body) = iam(
        &router,
        "CreateRole",
        &[
            ("RoleName", "held-role"),
            (
                "AssumeRolePolicyDocument",
                "{\"Version\":\"2012-10-17\",\"Statement\":[{\"Effect\":\"Allow\",\
                 \"Principal\":{\"Service\":\"ec2.amazonaws.com\"},\
                 \"Action\":\"sts:AssumeRole\"}]}",
            ),
        ],
    )
    .await;
    assert_eq!(status, 200, "the role must be created, got {body}");

    let account = engine::emulator_account_id();
    let resource = format!("arn:aws:s3:::{bucket}/*");
    // (case, the `Principal` element as JSON, accepted?)
    let rows: [(&str, String, bool); 10] = [
        ("the wildcard", "\"*\"".to_string(), true),
        (
            "a user the account holds",
            format!("{{\"AWS\":\"{}\"}}", user_arn("held-reader")),
            true,
        ),
        (
            "a role the account holds",
            format!("{{\"AWS\":\"arn:aws:iam::{account}:role/held-role\"}}"),
            true,
        ),
        (
            "the account's own root",
            format!("{{\"AWS\":\"arn:aws:iam::{account}:root\"}}"),
            true,
        ),
        (
            "a service principal",
            "{\"Service\":\"logs.amazonaws.com\"}".to_string(),
            true,
        ),
        (
            "a user of ANOTHER account, whose IAM is not this one",
            "{\"AWS\":\"arn:aws:iam::111122223333:user/outsider\"}".to_string(),
            true,
        ),
        (
            "a user the account does not hold",
            format!("{{\"AWS\":\"{}\"}}", user_arn("never-created")),
            false,
        ),
        (
            "a role the account does not hold",
            format!("{{\"AWS\":\"arn:aws:iam::{account}:role/never-created\"}}"),
            false,
        ),
        (
            "a GROUP the account does hold",
            format!("{{\"AWS\":\"arn:aws:iam::{account}:group/held-readers\"}}"),
            false,
        ),
        (
            "a list in which one entry does not resolve",
            format!(
                "{{\"AWS\":[\"{}\",\"{}\"]}}",
                user_arn("held-reader"),
                user_arn("never-created")
            ),
            false,
        ),
    ];
    // The policy the bucket carries before the table runs, which every refused row must
    // leave standing.
    let accepted = format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[{}]}}",
        statement("Allow", &user_arn("held-reader"), "s3:GetObject", &resource)
    );
    let stored = put_bucket_policy(&router, bucket, &accepted).await;
    assert_eq!(stored.status, 204, "the first policy must be stored");

    for (case, principal, acceptable) in rows {
        let document = format!(
            "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Effect\":\"Allow\",\
             \"Principal\":{principal},\"Action\":\"s3:GetObject\",\
             \"Resource\":\"{resource}\"}}]}}"
        );
        let put = put_bucket_policy(&router, bucket, &document).await;
        let body = String::from_utf8_lossy(&put.body).to_string();
        if acceptable {
            assert_eq!(
                put.status, 204,
                "case {case:?}: the document must be stored, got {} and body {body:?}",
                put.status
            );
            // Put the baseline back, so the next refused row has the same document to leave
            // standing.
            let restored = put_bucket_policy(&router, bucket, &accepted).await;
            assert_eq!(
                restored.status, 204,
                "case {case:?}: the baseline policy must be restorable"
            );
            continue;
        }
        assert_eq!(
            (put.status, put.error_code(), put.error_message()),
            (
                400,
                MALFORMED_POLICY.to_string(),
                INVALID_PRINCIPAL.to_string()
            ),
            "case {case:?}: expected the refusal of an unresolvable principal, got {} and \
             body {body:?}",
            put.status
        );
        let read = get_bucket_policy(&router, bucket).await;
        assert_eq!(
            String::from_utf8_lossy(&read.body).to_string(),
            accepted,
            "case {case:?}: a refused write must leave the stored policy as it was"
        );
    }
}

/// RULE: with `BlockPublicPolicy` on, a wildcard-principal grant counts as public unless its
/// `Condition` restricts it to FIXED values — a wildcard-VALUED condition restricts nothing —
/// and a public document is refused 403 `AccessDenied` naming the setting that prevented it.
#[tokio::test]
async fn block_public_policy_refuses_a_grant_whose_condition_fixes_nothing() {
    let router = router();
    // (case, the whole Condition clause, accepted?)
    let rows: [(&str, &str, bool); 7] = [
        (
            "one fixed VPC",
            "{\"StringEquals\":{\"aws:SourceVpc\":\"vpc-0123456789abcdef0\"}}",
            true,
        ),
        (
            "every VPC whose id starts as every VPC id does",
            "{\"StringLike\":{\"aws:SourceVpc\":\"vpc-*\"}}",
            false,
        ),
        (
            "the bare wildcard on a VPC",
            "{\"StringLike\":{\"aws:SourceVpc\":\"*\"}}",
            false,
        ),
        (
            "one fixed VPC endpoint",
            "{\"StringEquals\":{\"aws:SourceVpce\":\"vpce-0123456789abcdef0\"}}",
            true,
        ),
        (
            "every VPC endpoint",
            "{\"StringLike\":{\"aws:SourceVpce\":\"vpce-*\"}}",
            false,
        ),
        (
            "the paths under one organization root",
            "{\"ForAnyValue:StringLike\":{\"aws:PrincipalOrgPaths\":\
             \"o-exampleorgid/r-examplerootid/*\"}}",
            true,
        ),
        (
            "every organization there is",
            "{\"StringLike\":{\"aws:PrincipalOrgID\":\"o-*\"}}",
            false,
        ),
    ];
    for (index, (case, condition, acceptable)) in rows.into_iter().enumerate() {
        // Every bucket is created with all four Block Public Access switches ON, which is the
        // configuration this rule is about, so no row lifts them.
        let bucket = format!("identity-authz-public-{index}");
        let created = create_bucket(&router, &bucket, None, None).await;
        assert_eq!(created.status, 200, "case {case:?}: the bucket must exist");
        let document = format!(
            "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Effect\":\"Allow\",\
             \"Principal\":\"*\",\"Action\":\"s3:GetObject\",\
             \"Resource\":\"arn:aws:s3:::{bucket}/*\",\"Condition\":{condition}}}]}}"
        );
        let put = put_bucket_policy(&router, &bucket, &document).await;
        let body = String::from_utf8_lossy(&put.body).to_string();
        if acceptable {
            assert_eq!(
                put.status, 204,
                "case {case:?}: a grant restricted to fixed values must be stored, got {} and \
                 body {body:?}",
                put.status
            );
            continue;
        }
        assert_eq!(
            (put.status, put.error_code(), put.error_message()),
            (
                403,
                ACCESS_DENIED.to_string(),
                format!(
                    "User: {} is not authorized to perform: s3:PutBucketPolicy on resource: \
                     \"arn:aws:s3:::{bucket}\" because public policies are prevented by the \
                     BlockPublicPolicy setting in S3 Block Public Access.",
                    service_com_amazonaws_s3_amazons3::state::caller_principal_arn()
                )
            ),
            "case {case:?}: expected the guardrail's refusal, got {} and body {body:?}",
            put.status
        );
    }
}
