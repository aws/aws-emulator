// SPDX-License-Identifier: CC0-1.0

//! WHO a multipart upload reports as its initiator, and WHERE a page of uploads says the
//! listing stopped.
//!
//! Every assertion drives real HTTP requests through a router holding S3 beside IAM and STS
//! over ONE shared engine, so the callers are made with those services' own operations
//! (`CreateUser` + `PutUserPolicy` + `CreateAccessKey`, `CreateRole` + `AssumeRole`) and each
//! S3 request is presented with that caller's access key id — which is where this emulator
//! reads which principal a request speaks for. Nothing reaches into a store.
//!
//! The two rules under test, each over a SET of inputs rather than one request:
//!
//! 1. An upload's `Initiator` is the PRINCIPAL that initiated it, recorded by the initiation
//!    and reported unchanged by every reader of the upload: "if the principal is an Amazon
//!    Web Services account, it provides the Canonical User ID. If the principal is an IAM
//!    User, it provides a user ARN value" (`Initiator$ID`), with that identity's own friendly
//!    name beside it (`Initiator$DisplayName`). The rows differ along WHO calls — the
//!    account's own published credential, an IAM user at the account root, an IAM user under
//!    an entity path several levels deep, and an assumed-role session — so no row's identity
//!    can be fixed text, and each is the ARN the service that minted the principal reported
//!    for it. The `Owner` of the object the upload will become stays the account that holds
//!    the bucket, named by its canonical user id, so the two containers do not collapse into
//!    one.
//! 2. `NextKeyMarker`/`NextUploadIdMarker` are part of EVERY answer and name the position
//!    the page stopped at — the last entry it reported, which a next request resumes after —
//!    whether or not anything was left over, and are empty for a page that reported no entry
//!    at all; `IsTruncated` is what separately reports whether there is a next page ("Set the
//!    value of key-marker to the NextKeyMarker value from the previous response. Similarly,
//!    set the value of upload-id-marker to the NextUploadIdMarker value from the previous
//!    response", `ListMultipartUploads`'s own documentation). The rows differ along the page
//!    size: smaller than, exactly as large as and larger than the number of uploads the
//!    bucket holds, a listing naming no size at all, and a page of no entries.

use std::sync::Arc;

use serde_json::json;
use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::{elements_of, send_bytes_signed, Answer};
use super::identity_authorization::form_encode;
use super::multipart_upload_listing::upload_listing_of;

/// The Region every request in this suite is signed for, and the endpoint it is addressed to.
const REGION: &str = "us-east-1";

/// An access key id neither IAM nor STS holds a record of, which is the position the
/// account's own published credential is in. It creates every fixture here, and it is one of
/// the callers the first table spans.
const ACCOUNT_KEY: &str = "AKIDEMULATOR";

/// An emulator holding S3 beside the two services that hold WHO a credential speaks for: IAM
/// (the account's users and the access keys vended for them) and STS (the sessions a role is
/// assumed into).
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // The suite signs nothing: the `Authorization` header carries only the credential scope,
    // which is where a service reads the Region and the caller's access key id.
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_sts_awssecuritytokenservicev20110615::EmulatedService::new(engine),
    );
    router
}

/// One S3 request signed with `access_key_id` in its SigV4 credential scope.
async fn s3(
    router: &MultiServiceRouter,
    access_key_id: &str,
    method: &str,
    uri: &str,
) -> Answer {
    let host = format!("s3.{REGION}.amazonaws.com");
    let authorization = format!(
        "AWS4-HMAC-SHA256 Credential={access_key_id}/20260101/{REGION}/s3/aws4_request, \
         SignedHeaders=host, Signature=not-verified"
    );
    send_bytes_signed(router, &authorization, method, &host, uri, &[], b"").await
}

/// One awsQuery call to IAM or STS, signed with `access_key_id`, answered with its body.
async fn query(
    router: &MultiServiceRouter,
    access_key_id: &str,
    signing_name: &str,
    version: &str,
    action: &str,
    params: &[(&str, String)],
) -> String {
    let mut body = format!("Action={action}&Version={version}");
    for (name, value) in params {
        body.push('&');
        body.push_str(name);
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let host = format!("{signing_name}.amazonaws.com");
    let request = ::http::Request::builder()
        .method("POST")
        .uri(format!("https://{host}/"))
        .header("host", host)
        .header("content-type", "application/x-www-form-urlencoded")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential={access_key_id}/20260101/{REGION}/{signing_name}/\
                 aws4_request, SignedHeaders=host, Signature=not-verified"
            ),
        )
        .body(bytes::Bytes::from(body))
        .expect("a well-formed awsQuery request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let answered = String::from_utf8_lossy(response.body()).to_string();
    assert_eq!(
        status, 200,
        "case {action:?}: the call that makes this suite's caller must succeed, got {status} \
         with body {answered:?}"
    );
    answered
}

/// The text of the first `<tag>` element of an awsQuery answer.
fn answered(body: &str, tag: &str) -> String {
    elements_of(body, tag)
        .first()
        .cloned()
        .unwrap_or_else(|| panic!("the answer must carry <{tag}>, got {body:?}"))
}

/// One caller a row initiates an upload as.
struct Caller {
    /// What the row is called in a failed assertion.
    case: &'static str,
    /// The access key id its S3 requests present, which is where this emulator reads WHO is
    /// calling.
    access_key_id: String,
    /// The ARN the principal is known by, as the service that minted it reported it — what
    /// `Initiator$ID` owes for an IAM identity.
    arn: String,
    /// The principal's friendly name, which `Initiator$DisplayName` owes.
    display_name: String,
}

/// An identity-based document wide enough to initiate an upload into `bucket`: an initiation
/// "must be allowed to perform the `s3:PutObject` action on an object"
/// (`CreateMultipartUpload`'s own documentation of the permissions it needs).
fn bucket_grant(bucket: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Action": "s3:*",
            "Resource": [
                format!("arn:aws:s3:::{bucket}"),
                format!("arn:aws:s3:::{bucket}/*"),
            ],
        }],
    })
    .to_string()
}

/// The account's own published credential, resolved the way every service resolves it: the
/// principal `sts:GetCallerIdentity` reports for an access key id neither store holds.
async fn account_caller(router: &MultiServiceRouter) -> Caller {
    let reported = query(
        router,
        ACCOUNT_KEY,
        "sts",
        "2011-06-15",
        "GetCallerIdentity",
        &[],
    )
    .await;
    Caller {
        case: "the account's own published credential",
        access_key_id: ACCOUNT_KEY.to_string(),
        arn: answered(&reported, "Arn"),
        display_name: ::service_common::caller_principal::AMBIENT_USER_NAME.to_string(),
    }
}

/// An IAM user of this account sitting under the entity path `path`, carrying the grant an
/// initiation needs and holding an access key of its own.
async fn iam_user(
    router: &MultiServiceRouter,
    case: &'static str,
    name: &str,
    path: &str,
    bucket: &str,
) -> Caller {
    let created = query(
        router,
        ACCOUNT_KEY,
        "iam",
        "2010-05-08",
        "CreateUser",
        &[
            ("UserName", name.to_string()),
            ("Path", path.to_string()),
        ],
    )
    .await;
    let arn = answered(&created, "Arn");
    query(
        router,
        ACCOUNT_KEY,
        "iam",
        "2010-05-08",
        "PutUserPolicy",
        &[
            ("UserName", name.to_string()),
            ("PolicyName", "initiate-uploads".to_string()),
            ("PolicyDocument", bucket_grant(bucket)),
        ],
    )
    .await;
    let vended = query(
        router,
        ACCOUNT_KEY,
        "iam",
        "2010-05-08",
        "CreateAccessKey",
        &[("UserName", name.to_string())],
    )
    .await;
    Caller {
        case,
        access_key_id: answered(&vended, "AccessKeyId"),
        arn,
        display_name: name.to_string(),
    }
}

/// A session of a role of this account, assumed through STS's own `AssumeRole`, whose role
/// carries the grant an initiation needs.
async fn role_session(
    router: &MultiServiceRouter,
    case: &'static str,
    role_name: &str,
    session_name: &str,
    bucket: &str,
) -> Caller {
    let account = ::engine::emulator_account_id();
    let role_arn = format!("arn:aws:iam::{account}:role/{role_name}");
    let trust = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": {"AWS": format!("arn:aws:iam::{account}:root")},
            "Action": "sts:AssumeRole",
        }],
    })
    .to_string();
    query(
        router,
        ACCOUNT_KEY,
        "iam",
        "2010-05-08",
        "CreateRole",
        &[
            ("RoleName", role_name.to_string()),
            ("AssumeRolePolicyDocument", trust),
        ],
    )
    .await;
    query(
        router,
        ACCOUNT_KEY,
        "iam",
        "2010-05-08",
        "PutRolePolicy",
        &[
            ("RoleName", role_name.to_string()),
            ("PolicyName", "initiate-uploads".to_string()),
            ("PolicyDocument", bucket_grant(bucket)),
        ],
    )
    .await;
    let assumed = query(
        router,
        ACCOUNT_KEY,
        "sts",
        "2011-06-15",
        "AssumeRole",
        &[
            ("RoleArn", role_arn),
            ("RoleSessionName", session_name.to_string()),
        ],
    )
    .await;
    Caller {
        case,
        access_key_id: answered(&assumed, "AccessKeyId"),
        arn: answered(&assumed, "Arn"),
        display_name: session_name.to_string(),
    }
}

/// `PUT /<bucket>` as the account — the bucket every upload of this suite is initiated into.
async fn create_bucket(router: &MultiServiceRouter, bucket: &str) {
    let created = s3(router, ACCOUNT_KEY, "PUT", &format!("/{bucket}")).await;
    assert_eq!(
        created.status,
        200,
        "the bucket {bucket:?} must be created first, got {} with body {:?}",
        created.status,
        body_text(&created)
    );
}

/// `POST /<bucket>/<key>?uploads` as `caller` — `CreateMultipartUpload`, answering the id the
/// store minted for the upload.
async fn initiate(
    router: &MultiServiceRouter,
    caller: &Caller,
    bucket: &str,
    key: &str,
) -> String {
    let initiated = s3(
        router,
        &caller.access_key_id,
        "POST",
        &format!("/{bucket}/{key}?uploads"),
    )
    .await;
    assert_eq!(
        initiated.status,
        200,
        "case {:?}: initiating the upload of {key:?} must answer 200, got {} with body {:?}",
        caller.case,
        initiated.status,
        body_text(&initiated)
    );
    initiated.element("UploadId").unwrap_or_else(|| {
        panic!(
            "case {:?}: an initiation must report an UploadId, got {:?}",
            caller.case,
            body_text(&initiated)
        )
    })
}

/// The `Initiator` container of a read of ONE upload, as `(ID, DisplayName)` — the empty
/// string for a member the document leaves out.
fn initiator_of(case: &str, answer: &Answer) -> (String, String) {
    let body = body_text(answer);
    let container = elements_of(&body, "Initiator")
        .first()
        .cloned()
        .unwrap_or_else(|| {
            panic!("case {case:?}: the read must report an Initiator, got {body:?}")
        });
    (
        elements_of(&container, "ID").first().cloned().unwrap_or_default(),
        elements_of(&container, "DisplayName")
            .first()
            .cloned()
            .unwrap_or_default(),
    )
}

/// A response body as text.
fn body_text(answer: &Answer) -> String {
    String::from_utf8_lossy(&answer.body).to_string()
}

/// A canonical user id is "a 64-digit hexadecimal encoded, 256-bit value" (AWS General
/// Reference, *AWS account identifiers*) — the shape an `Owner$ID` carries.
fn is_canonical_user_id(value: &str) -> bool {
    value.len() == 64 && value.chars().all(|digit| digit.is_ascii_hexdigit())
}

#[tokio::test]
async fn every_reader_of_an_upload_reports_the_principal_that_initiated_it() {
    let router = emulator();
    let bucket = "mpu-initiator-identity";
    create_bucket(&router, bucket).await;

    // The rows: four kinds of principal, each reaching S3 with a credential of its own, so
    // what the readers report has to be read off the caller rather than fixed anywhere.
    let callers = [
        account_caller(&router).await,
        iam_user(
            &router,
            "an IAM user at the account root",
            "mpu-root-initiator",
            "/",
            bucket,
        )
        .await,
        iam_user(
            &router,
            "an IAM user under an entity path several levels deep",
            "mpu-nested-initiator",
            "/team/platform/",
            bucket,
        )
        .await,
        role_session(
            &router,
            "an assumed-role session",
            "mpu-initiator-role",
            "mpu-initiator-session",
            bucket,
        )
        .await,
    ];

    // Each caller initiates one upload, under a key of its own, so ONE listing of the bucket
    // reports every one of them beside each other.
    let mut initiated: Vec<(&Caller, String, String)> = Vec::new();
    for (position, caller) in callers.iter().enumerate() {
        let key = format!("initiated-by-{position}.bin");
        let upload_id = initiate(&router, caller, bucket, &key).await;
        initiated.push((caller, key, upload_id));
    }

    let answer = s3(&router, ACCOUNT_KEY, "GET", &format!("/{bucket}?uploads")).await;
    assert_eq!(
        answer.status,
        200,
        "the listing of {bucket:?} must answer 200, got {} with body {:?}",
        answer.status,
        body_text(&answer)
    );
    let listing = upload_listing_of(&answer);
    let entries = listing.entries();
    let initiator_ids = listing.reported("Initiator.ID");
    let initiator_names = listing.reported("Initiator.DisplayName");
    let owner_ids = listing.reported("Owner.ID");

    for (caller, key, upload_id) in &initiated {
        let at = entries
            .iter()
            .position(|(_, reported)| reported == upload_id)
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: the listing must report the upload {upload_id:?} of {key:?}, got \
                     {entries:?}",
                    caller.case
                )
            });
        // The bucket-wide reader reports the principal that initiated THIS upload…
        assert_eq!(
            (
                initiator_ids[at].as_str(),
                initiator_names[at].as_str()
            ),
            (caller.arn.as_str(), caller.display_name.as_str()),
            "case {:?}: the listing must report the initiating principal of {key:?}, got {:?}",
            caller.case,
            (&initiator_ids[at], &initiator_names[at])
        );
        // …while the OWNER of the object it will become is the account holding the bucket,
        // named by its canonical user id, which is a different value from the principal.
        assert!(
            is_canonical_user_id(&owner_ids[at]),
            "case {:?}: the owner of {key:?} must be the account's canonical user id, got {:?}",
            caller.case,
            owner_ids[at]
        );
        assert_ne!(
            owner_ids[at], initiator_ids[at],
            "case {:?}: the owner and the initiating principal of {key:?} are reported by \
             different identifiers, got {:?}",
            caller.case, owner_ids[at]
        );

        // And the per-upload reader reports the same principal: the initiation RECORDED it,
        // so it does not depend on which reader asks or on who is asking.
        let read = s3(
            &router,
            ACCOUNT_KEY,
            "GET",
            &format!("/{bucket}/{key}?uploadId={upload_id}&x-id=ListParts"),
        )
        .await;
        assert_eq!(
            read.status,
            200,
            "case {:?}: the read of the upload of {key:?} must answer 200, got {} with body {:?}",
            caller.case,
            read.status,
            body_text(&read)
        );
        assert_eq!(
            initiator_of(caller.case, &read),
            (caller.arn.clone(), caller.display_name.clone()),
            "case {:?}: the read of one upload must report the same initiating principal as \
             the listing, got {:?}",
            caller.case,
            initiator_of(caller.case, &read)
        );
    }
}

#[tokio::test]
async fn every_page_reports_the_position_it_stopped_at_beside_whether_more_is_left() {
    let router = emulator();
    let bucket = "mpu-page-stop-position";
    create_bucket(&router, bucket).await;
    let account = account_caller(&router).await;

    // Three uploads under keys that sort in this order, so the entry each page ends on is
    // known without reading the listing for it.
    let mut held: Vec<(String, String)> = Vec::new();
    for key in ["stop-a.bin", "stop-b.bin", "stop-c.bin"] {
        let upload_id = initiate(&router, &account, bucket, key).await;
        held.push((key.to_string(), upload_id));
    }

    // (case, the page size the request names, how many entries the page carries, whether
    // anything was left over for a next page).
    let cases: [(&str, &str, usize, bool); 6] = [
        (
            "a page smaller than the uploads held",
            "&max-uploads=1",
            1,
            true,
        ),
        (
            "a page that stops one short of the uploads held",
            "&max-uploads=2",
            2,
            true,
        ),
        (
            "a page exactly as large as the uploads held",
            "&max-uploads=3",
            3,
            false,
        ),
        (
            "a page larger than the uploads held",
            "&max-uploads=4",
            3,
            false,
        ),
        ("a listing naming no page size at all", "", 3, false),
        ("a page of no entries at all", "&max-uploads=0", 0, false),
    ];

    for (case, size, carried, more_left) in cases {
        let answer = s3(
            &router,
            ACCOUNT_KEY,
            "GET",
            &format!("/{bucket}?uploads{size}"),
        )
        .await;
        assert_eq!(
            answer.status,
            200,
            "case {case:?}: the listing must answer 200, got {} with body {:?}",
            answer.status,
            body_text(&answer)
        );
        let page = upload_listing_of(&answer);
        assert_eq!(
            page.entries(),
            held[..carried].to_vec(),
            "case {case:?}: expected the first {carried} uploads the bucket holds, got {:?}",
            page.entries()
        );
        assert_eq!(
            page.field("IsTruncated"),
            more_left.to_string(),
            "case {case:?}: IsTruncated reports whether anything was left over, expected \
             {more_left}, got {:?}",
            page.field("IsTruncated")
        );
        // The position the page stopped at: the last entry it reported, whether or not
        // anything was left over — and nothing at all for a page that reported no entry,
        // which stopped at no position.
        let wanted = match carried {
            0 => (String::new(), String::new()),
            carried => held[carried - 1].clone(),
        };
        assert_eq!(
            (
                page.field("NextKeyMarker"),
                page.field("NextUploadIdMarker")
            ),
            wanted,
            "case {case:?}: expected the next markers {wanted:?}, got {:?}",
            (
                page.field("NextKeyMarker"),
                page.field("NextUploadIdMarker")
            )
        );
    }

    // The markers are a usable cursor, which is what they are for: resuming from the
    // position the first page of one stopped at reports the uploads after it, and resuming
    // from the position a page that left NOTHING over stopped at reports no upload at all —
    // so a client that keeps paging loses nothing and repeats nothing.
    for (case, size, from, expected) in [
        ("resuming a page of 1", 1_usize, 0_usize, &held[1..]),
        ("resuming a page of 2", 2, 1, &held[2..]),
        ("resuming the page that held everything", 3, 2, &held[3..]),
    ] {
        let (key_marker, id_marker) = held[from].clone();
        let resumed = format!(
            "/{bucket}?uploads&max-uploads={size}&key-marker={key_marker}\
             &upload-id-marker={id_marker}"
        );
        let answer = s3(&router, ACCOUNT_KEY, "GET", &resumed).await;
        assert_eq!(
            answer.status,
            200,
            "case {case:?}: the resumed listing must answer 200, got {} with body {:?}",
            answer.status,
            body_text(&answer)
        );
        let page = upload_listing_of(&answer);
        let wanted: Vec<(String, String)> = expected.iter().take(size).cloned().collect();
        assert_eq!(
            page.entries(),
            wanted,
            "case {case:?}: resuming after {key_marker:?} must report {wanted:?}, got {:?}",
            page.entries()
        );
    }
}
