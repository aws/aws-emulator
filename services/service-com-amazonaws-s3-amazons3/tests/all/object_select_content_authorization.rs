// SPDX-License-Identifier: CC0-1.0

//! WHO may ask Amazon S3 for a Select query, and what each caller is answered.
//!
//! RULE: a `?select` request is authorized before anything else about it is considered — "you
//! must have the `s3:GetObject` permission for this operation"
//! (`SelectObjectContent`'s own documentation) — so a caller the policies in force do not
//! leave that action on the addressed object to is refused 403 `AccessDenied`, naming the
//! principal, the action and the resource, exactly as a plain read of the same object's bytes
//! is refused; and a caller that MAY perform it meets the feature's own refusal instead, the
//! 405 `MethodNotAllowed` S3 answers a resource that does not serve the method asked of it
//! (the withdrawal of S3 Select from every account that had not already enabled it, which
//! [`super::object_select_content`] pins over every shape a Select request takes).
//!
//! Neither answer is the emulator's 501 `NotImplemented`: a caller that is refused for its
//! permissions and one that is refused the feature are two different facts, and a client
//! branches on both.
//!
//! The rows differ along what the rule generalises over: WHAT the caller holds (no policy at
//! all, a grant of another action, a grant of the same action on another bucket, the grant
//! itself, a wildcard grant that covers it, a grant barred by an explicit `Deny` in the
//! bucket's own policy, and the account's own credential), and WHETHER the addressed key
//! holds an object — because the permission is of the object's resource whatever is stored
//! there, so neither answer may turn on what the key holds.
//!
//! Every assertion drives real HTTP requests through a router holding S3 beside IAM over one
//! shared engine: the user, its access key and its policies are created through IAM's own API,
//! and each request is presented with that user's access key id, which is where the emulator
//! reads the signing identity from.

use service_com_amazonaws_s3_amazons3::state::{
    ACCESS_DENIED_CODE, ACCESS_DENIED_STATUS, METHOD_NOT_ALLOWED_CODE, METHOD_NOT_ALLOWED_MESSAGE,
    METHOD_NOT_ALLOWED_STATUS,
};
use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::{create_bucket, put_bucket_policy, Answer};
use super::identity_authorization::{no_grant_message, router, s3, user_arn, user_with_grant};
use super::object_write_authorization::{policy_document, statement};

/// The IAM action a Select query is authorized under.
const GET_OBJECT_ACTION: &str = "s3:GetObject";

/// The bytes the held key holds — the CSV shape a Select query is written for.
const ROWS_CSV: &[u8] = b"id,name,category\n1,alice,gold\n2,bob,silver\n";

/// The key each row's setup writes.
const HELD_KEY: &str = "reports/rows.csv";

/// A key no setup writes, so the request addresses an object that is not there.
const ABSENT_KEY: &str = "reports/never-written.csv";

/// The Select request document, with the members the model declares required
/// (`smithy_query(query="closure",operation="SelectObjectContent")`).
const SELECT_REQUEST: &[u8] = br#"<SelectObjectContentRequest xmlns="http://s3.amazonaws.com/doc/2006-03-01/"><Expression>SELECT * FROM S3Object</Expression><ExpressionType>SQL</ExpressionType><InputSerialization><CSV><FileHeaderInfo>USE</FileHeaderInfo></CSV></InputSerialization><OutputSerialization><CSV></CSV></OutputSerialization></SelectObjectContentRequest>"#;

/// `POST /<bucket>/<key>?select&select-type=2` — the route the model binds
/// `SelectObjectContent` at — presented as signed by `akid` when one is given.
async fn select_object_content(
    router: &MultiServiceRouter,
    akid: Option<&str>,
    bucket: &str,
    key: &str,
) -> Answer {
    s3(
        router,
        akid,
        "POST",
        &format!("/{bucket}/{key}?select&select-type=2"),
        SELECT_REQUEST,
    )
    .await
}

/// The ARNs a grant for one bucket names: the bucket itself and every key in it.
fn bucket_resources(bucket: &str) -> Vec<String> {
    vec![
        format!("arn:aws:s3:::{bucket}"),
        format!("arn:aws:s3:::{bucket}/*"),
    ]
}

/// The ARN of one object, the resource an object-scoped grant is decided over.
fn object_arn(bucket: &str, key: &str) -> String {
    format!("arn:aws:s3:::{bucket}/{key}")
}

/// The sentence S3 refuses a request an explicit `Deny` in the bucket's own policy barred
/// with: the principal, the action, the resource and IAM's reason clause for that decision.
/// Written out here rather than built from the service's own helper, so the wording has
/// something to be wrong about.
fn resource_deny_message(principal: &str, action: &str, resource: &str) -> String {
    format!(
        "User: {principal} is not authorized to perform: {action} on resource: \"{resource}\" \
         with an explicit deny in a resource-based policy"
    )
}

/// What a row expects: the feature's own refusal, or the authorization refusal carrying a
/// sentence.
enum Expected {
    /// 405 `MethodNotAllowed` with its published message — the caller may perform the action.
    FeatureWithdrawn,
    /// 403 `AccessDenied`, carrying this sentence.
    Denied(fn(&str, &str, &str) -> String),
}

/// One row of the table: the case, the actions the caller's identity policy grants, the bucket
/// those grants name, the bucket policy the addressed bucket carries, whether the request is
/// signed by that user at all, and what it must be answered.
struct Row {
    case: &'static str,
    actions: &'static [&'static str],
    /// `true` when the grant names the bucket the request addresses, `false` when it names
    /// another bucket entirely.
    grants_this_bucket: bool,
    /// The bucket policy the addressed bucket carries, as a statement.
    bucket_policy: Option<(&'static str, &'static str)>,
    /// `false` for the row that presents no credential of a user's at all — the account's own.
    signed: bool,
    expected: Expected,
}

#[tokio::test]
async fn a_select_request_is_refused_for_its_permissions_before_it_is_refused_the_feature() {
    let router = router();
    let other = "select-authorization-elsewhere";
    let created = create_bucket(&router, other, None, None).await;
    assert_eq!(
        created.status, 200,
        "the bucket a grant can name instead must be created, got {}",
        created.status
    );
    // A caller that MAY read every object of the account, so each row can check that its
    // refusal left the stored object exactly as it was.
    let reader = user_with_grant(
        &router,
        "select-authorization-reader",
        &["s3:GetObject"],
        &["arn:aws:s3:::*/*".to_string()],
    )
    .await;

    // The rows differ along WHAT the caller holds; the two keys below then show that neither
    // answer depends on what the addressed key holds.
    let rows: [Row; 7] = [
        Row {
            case: "a caller carrying no policy at all",
            actions: &[],
            grants_this_bucket: true,
            bucket_policy: None,
            signed: true,
            expected: Expected::Denied(no_grant_message),
        },
        Row {
            case: "a caller granted another action on the same object",
            actions: &["s3:PutObject"],
            grants_this_bucket: true,
            bucket_policy: None,
            signed: true,
            expected: Expected::Denied(no_grant_message),
        },
        Row {
            case: "a caller granted the read of another bucket's objects",
            actions: &["s3:GetObject"],
            grants_this_bucket: false,
            bucket_policy: None,
            signed: true,
            expected: Expected::Denied(no_grant_message),
        },
        Row {
            case: "a caller granted the read of this object",
            actions: &["s3:GetObject"],
            grants_this_bucket: true,
            bucket_policy: None,
            signed: true,
            expected: Expected::FeatureWithdrawn,
        },
        Row {
            case: "a caller granted every action of this service",
            actions: &["s3:*"],
            grants_this_bucket: true,
            bucket_policy: None,
            signed: true,
            expected: Expected::FeatureWithdrawn,
        },
        Row {
            case: "a caller granted the read and barred by the bucket's own policy",
            actions: &["s3:GetObject"],
            grants_this_bucket: true,
            bucket_policy: Some(("Deny", "s3:GetObject")),
            signed: true,
            expected: Expected::Denied(resource_deny_message),
        },
        Row {
            case: "the account's own credential",
            actions: &[],
            grants_this_bucket: true,
            bucket_policy: None,
            signed: false,
            expected: Expected::FeatureWithdrawn,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let bucket = format!("select-authorization-{index}");
        let created = create_bucket(&router, &bucket, None, None).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: the bucket must be created, got {}",
            created.status
        );
        // The account's own credential writes the object, so the row's own grant decides
        // nothing but the Select request below.
        let written = s3(
            &router,
            None,
            "PUT",
            &format!("/{bucket}/{HELD_KEY}"),
            ROWS_CSV,
        )
        .await;
        assert_eq!(
            written.status, 200,
            "case {case:?}: the object must be stored first, got {}",
            written.status
        );
        if let Some((effect, action)) = row.bucket_policy {
            let document = policy_document(&statement(
                effect,
                action,
                &object_arn(&bucket, "*"),
                None,
            ));
            let stored = put_bucket_policy(&router, &bucket, &document).await;
            assert_eq!(
                stored.status, 204,
                "case {case:?}: the bucket policy must be stored, got {}",
                stored.status
            );
        }
        let granted = match row.grants_this_bucket {
            true => bucket_resources(&bucket),
            false => bucket_resources(other),
        };
        let user = format!("select-caller-{index}");
        let akid = user_with_grant(&router, &user, row.actions, &granted).await;
        let presented = row.signed.then_some(akid.as_str());
        let principal = user_arn(&user);

        // Neither answer may turn on what the addressed key holds: the permission is of the
        // object's resource, and the withdrawal is account-wide.
        for key in [HELD_KEY, ABSENT_KEY] {
            let answer = select_object_content(&router, presented, &bucket, key).await;
            match row.expected {
                Expected::FeatureWithdrawn => {
                    assert_eq!(
                        answer.status, METHOD_NOT_ALLOWED_STATUS,
                        "case {case:?}/{key}: expected HTTP {METHOD_NOT_ALLOWED_STATUS}, got {} \
                         with body {:?}",
                        answer.status,
                        String::from_utf8_lossy(&answer.body)
                    );
                    assert_eq!(
                        answer.error_code(),
                        METHOD_NOT_ALLOWED_CODE,
                        "case {case:?}/{key}: expected the code {METHOD_NOT_ALLOWED_CODE:?}, got \
                         {:?}",
                        answer.error_code()
                    );
                    assert_eq!(
                        answer.error_message(),
                        METHOD_NOT_ALLOWED_MESSAGE,
                        "case {case:?}/{key}: expected the message \
                         {METHOD_NOT_ALLOWED_MESSAGE:?}, got {:?}",
                        answer.error_message()
                    );
                }
                Expected::Denied(sentence) => {
                    assert_eq!(
                        answer.status, ACCESS_DENIED_STATUS,
                        "case {case:?}/{key}: expected HTTP {ACCESS_DENIED_STATUS}, got {} with \
                         body {:?}",
                        answer.status,
                        String::from_utf8_lossy(&answer.body)
                    );
                    assert_eq!(
                        answer.error_code(),
                        ACCESS_DENIED_CODE,
                        "case {case:?}/{key}: expected the code {ACCESS_DENIED_CODE:?}, got {:?}",
                        answer.error_code()
                    );
                    let expected =
                        sentence(&principal, GET_OBJECT_ACTION, &object_arn(&bucket, key));
                    assert_eq!(
                        answer.error_message(),
                        expected,
                        "case {case:?}/{key}: expected the message {expected:?}, got {:?}",
                        answer.error_message()
                    );
                }
            }
        }

        // The refusal REPORTS: the object the setup stored is still there, byte for byte, as a
        // caller that may read it sees — except for the row whose bucket policy denies that
        // read to every principal, which is the point of that row.
        if row.bucket_policy.is_none() {
            let read = s3(
                &router,
                Some(reader.as_str()),
                "GET",
                &format!("/{bucket}/{HELD_KEY}"),
                b"",
            )
            .await;
            assert_eq!(
                read.status, 200,
                "case {case:?}: the stored object must still be readable, got {} with body {:?}",
                read.status,
                String::from_utf8_lossy(&read.body)
            );
            assert_eq!(
                read.body.as_ref(),
                ROWS_CSV,
                "case {case:?}: the stored bytes must come back unchanged"
            );
        }
    }
}

#[tokio::test]
async fn the_authorization_of_a_select_request_reads_the_same_address_in_either_form() {
    // A virtual-hosted-style Select request carries its bucket in the host and the key alone
    // in the path ("both the virtual-hosted-style requests and the path-style requests are
    // supported", `GetObject`'s own documentation), so the decision is taken over the same
    // object resource and the refusal names it the same way.
    let router = router();
    let bucket = "select-authorization-host-style";
    let created = create_bucket(&router, bucket, None, None).await;
    assert_eq!(
        created.status, 200,
        "the bucket must be created, got {}",
        created.status
    );
    let written = s3(
        &router,
        None,
        "PUT",
        &format!("/{bucket}/{HELD_KEY}"),
        ROWS_CSV,
    )
    .await;
    assert_eq!(
        written.status, 200,
        "the object must be stored first, got {}",
        written.status
    );
    let akid = user_with_grant(&router, "select-host-style-caller", &[], &[]).await;
    let expected = no_grant_message(
        &user_arn("select-host-style-caller"),
        GET_OBJECT_ACTION,
        &object_arn(bucket, HELD_KEY),
    );

    for (case, uri, host) in [
        (
            "the bucket in the path",
            format!("/{bucket}/{HELD_KEY}?select&select-type=2"),
            "s3.us-east-1.amazonaws.com".to_string(),
        ),
        (
            "the bucket in the host",
            format!("/{HELD_KEY}?select&select-type=2"),
            format!("{bucket}.s3.us-east-1.amazonaws.com"),
        ),
    ] {
        let request = ::http::Request::builder()
            .method("POST")
            .uri(format!("https://{host}{uri}"))
            .header("host", &host)
            .header("content-type", "application/xml")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential={akid}/20260101/us-east-1/s3/aws4_request, \
                     SignedHeaders=host, Signature=not-verified"
                ),
            )
            .body(bytes::Bytes::from_static(SELECT_REQUEST))
            .expect("the test request must build");
        let response = router.dispatch(request).await;
        let (parts, body) = response.into_parts();
        let answer = Answer {
            status: parts.status.as_u16(),
            headers: parts.headers,
            body,
        };
        assert_eq!(
            answer.status, ACCESS_DENIED_STATUS,
            "case {case:?}: expected HTTP {ACCESS_DENIED_STATUS}, got {} with body {:?}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            answer.error_code(),
            ACCESS_DENIED_CODE,
            "case {case:?}: expected the code {ACCESS_DENIED_CODE:?}, got {:?}",
            answer.error_code()
        );
        assert_eq!(
            answer.error_message(),
            expected,
            "case {case:?}: expected the message {expected:?}, got {:?}",
            answer.error_message()
        );
    }
}
