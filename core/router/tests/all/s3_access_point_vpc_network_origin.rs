// SPDX-License-Identifier: CC0-1.0

//! The NETWORK ORIGIN of an Amazon S3 access point, enforced on the data path — across the
//! two services that have to agree about it.
//!
//! The rule under test: **an access point created with a `VpcConfiguration` is confined to
//! that VPC — "if this field is specified, this access point will only allow connections
//! from the specified VPC ID" (`VpcConfiguration$VpcId`), which is what its `NetworkOrigin`
//! of `VPC` reports — so a request that reaches it from outside that VPC is refused HTTP 403
//! `AccessDenied`, whatever address it was addressed by and whatever action it asked for,
//! and no `Allow` in the access point's own policy lifts the refusal. An access point
//! created WITHOUT one has a network origin of `Internet` and serves the same bucket's
//! objects, and the bucket's own address is untouched by either.**
//!
//! This emulator is reached over one internet-facing data plane and no VPC carries traffic
//! into it, so every request that arrives here arrived from outside every VPC: that is what
//! makes the confinement observable at all.
//!
//! Two crates have to agree for the rule to hold, so the suite lives here: S3 Control
//! creates the access point and records the VPC it binds it to, and S3 resolves the address
//! and takes the decision. The S3 crate's own `access_point_data_plane` suite pins the
//! policy decision on an Internet-origin path; what only this suite can reach is the whole
//! chain — the access point created through its own API with its own `VpcConfiguration`, read
//! back through `GetAccessPoint`, and then addressed on the data path.
//!
//! The rows span what the rule generalises over: the ADDRESS the access point is reached by
//! (its ARN, its alias and its endpoint host label as the path's bucket label, and its own
//! endpoint host), the form of the KEY (a bare name and a nested one), the ACTION (an object
//! read, a HEAD and a listing of the access point itself), the NETWORK ORIGIN (`VPC` against
//! `Internet` over one bucket), what the access point's POLICY says (none, an `Allow` naming
//! the account, an `Allow` for every principal on the object, and an `Allow` of every action
//! on the whole access point) and whether the caller presents a credential at all.

use bytes::Bytes;
use smithy_server_runtime::MultiServiceRouter;

use super::query_wire::{element, form_encode, opt_element};

/// The Region every call here is signed for, and the Region the access point ARNs name.
const REGION: &str = "us-east-1";

/// The account this emulator serves: the `x-amz-account-id` the S3 Control calls carry and
/// the account the access point ARNs name.
const ACCOUNT: &str = ::engine::EMULATOR_ACCOUNT_ID;

/// The XML namespace S3 Control's model declares.
const S3CONTROL_XMLNS: &str = "http://awss3control.amazonaws.com/doc/2018-08-20/";

/// The bucket both access points front, and the bytes every key of it holds.
const BUCKET: &str = "xsvc-access-point-network-origin";
const BODY: &str = "the object behind the access point";

/// The keys the rows read: a bare name and a nested one, so the resource an access point
/// names for the object (`<access-point-arn>/object/<key>`) is driven over both forms.
const KEYS: [&str; 2] = ["private.txt", "reports/2026/private.csv"];

/// The two access points over that one bucket: one confined to a VPC, one not.
const VPC_ACCESS_POINT: &str = "xsvc-vpc-origin-ap";
const INTERNET_ACCESS_POINT: &str = "xsvc-internet-origin-ap";

/// The VPC the first one is bound to. A VPC id is `vpc-` followed by 17 hexadecimal
/// characters (Amazon VPC, resource id formats); which VPC it is does not enter the rule —
/// only that one is named.
const VPC_ID: &str = "vpc-0a1b2c3d4e5f67890";

/// What `NetworkOrigin` reports for each of them — the two values of that enum
/// (`smithy_query(query="shape",shape="NetworkOrigin")`: `Internet`, `VPC`).
const VPC_ORIGIN: &str = "VPC";
const INTERNET_ORIGIN: &str = "Internet";

/// The code and status S3 refuses an unauthorized request with (Amazon S3 "Error
/// responses": `AccessDenied`, HTTP 403).
const ACCESS_DENIED: &str = "AccessDenied";
const FORBIDDEN: u16 = 403;

/// The IAM actions the rows are refused under: the read of an object's current version, and
/// the listing of the access point's objects.
const GET_OBJECT: &str = "s3:GetObject";
const LIST_BUCKET: &str = "s3:ListBucket";

/// The reason clause IAM publishes for a request a resource-based document explicitly
/// denies, which is the clause the VPC confinement is refused in.
const RESOURCE_DENY: &str = "with an explicit deny in a resource-based policy";

/// What one call answered: the status and the body as text.
struct Answer {
    status: u16,
    body: String,
}

impl Answer {
    /// The text of the first `<name>` element of the body, or the empty string, with the XML
    /// escaping the serializer applies undone — a policy document's quotes travel as
    /// `&quot;` in an XML text node.
    fn element(&self, name: &str) -> String {
        opt_element(&self.body, name)
            .unwrap_or_default()
            .replace("&quot;", "\"")
            .replace("&apos;", "'")
            .replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&amp;", "&")
    }
}

/// The emulator as it is ASSEMBLED, on one engine: S3 Control writes the access point's data
/// path into the store S3 serves it from, so both halves of the rule are the same record.
fn build_router() -> MultiServiceRouter {
    let engine: std::sync::Arc<engine::InMemoryEngine> =
        std::sync::Arc::new(engine::InMemoryEngine::new());
    let mut router = aws_emulator_router::build_router(engine);
    router.set_request_authentication(false);
    router
}

/// A SigV4 `Authorization` header whose credential scope names `service`, which is what
/// routes the request. The signature is never checked: request authentication is off.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=not-verified"
    )
}

/// One request against the `s3` signing name — which is what both S3 and S3 Control sign as;
/// the URI path is what tells them apart — addressed to `host`.
async fn s3_at(
    router: &MultiServiceRouter,
    method: &str,
    host: &str,
    path: &str,
    body: &str,
) -> Answer {
    let request = http::Request::builder()
        .method(method)
        .uri(path)
        .header("host", host)
        .header("authorization", authorization("s3"))
        .header("x-amz-account-id", ACCOUNT)
        .header("content-type", "application/xml")
        .body(Bytes::from(body.to_owned()))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    Answer {
        status: response.status().as_u16(),
        body: String::from_utf8_lossy(response.body()).into_owned(),
    }
}

/// The Region's PATH-STYLE endpoint: the host whose own name carries no address.
fn path_style_host() -> String {
    format!("s3.{REGION}.amazonaws.com")
}

/// One request to the path-style endpoint — every S3 Control call, and the path-style half of
/// the data-plane rows.
async fn s3_wire(router: &MultiServiceRouter, method: &str, path: &str, body: &str) -> Answer {
    s3_at(router, method, &path_style_host(), path, body).await
}

/// One UNSIGNED request to the path-style endpoint: the caller that presents no credential at
/// all ("if you grant READ access to the anonymous user, the GetObject operation returns the
/// object without using an authorization header", `GetObject`'s own documentation).
async fn s3_anonymous(router: &MultiServiceRouter, method: &str, path: &str) -> Answer {
    let host = path_style_host();
    let request = http::Request::builder()
        .method(method)
        .uri(path)
        .header("host", &host)
        .body(Bytes::new())
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    Answer {
        status: response.status().as_u16(),
        body: String::from_utf8_lossy(response.body()).into_owned(),
    }
}

/// The identity this emulator reports for the credential these calls present, which is the
/// principal every refusal below names — read from the service that answers for it rather
/// than written out here.
async fn caller_arn(router: &MultiServiceRouter) -> String {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("sts.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization("sts"))
        .body(Bytes::from("Action=GetCallerIdentity&Version=2011-06-15"))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let body = String::from_utf8_lossy(response.body()).into_owned();
    assert_eq!(
        status, 200,
        "setup: GetCallerIdentity must succeed, got {status} with {body}"
    );
    element(&body, "Arn")
}

/// `PUT /<bucket>` — S3's `CreateBucket`.
async fn create_bucket(router: &MultiServiceRouter, bucket: &str) {
    let answer = s3_wire(router, "PUT", &format!("/{bucket}"), "").await;
    assert_eq!(
        answer.status, 200,
        "setup: CreateBucket({bucket:?}) must succeed, got {} with {}",
        answer.status, answer.body
    );
}

/// `PUT /<bucket>/<key>` — S3's `PutObject`.
async fn put_object(router: &MultiServiceRouter, bucket: &str, key: &str, body: &str) {
    let answer = s3_wire(router, "PUT", &format!("/{bucket}/{key}"), body).await;
    assert_eq!(
        answer.status, 200,
        "setup: PutObject({bucket:?}, {key:?}) must succeed, got {} with {}",
        answer.status, answer.body
    );
}

/// `PUT /<bucket>?publicAccessBlock` — S3's `PutPublicAccessBlock`, with all four switches
/// off, which is what lets a PUBLIC bucket policy be stored at all ("`BlockPublicPolicy` …
/// causes Amazon S3 to reject calls to PUT Bucket policy if the specified bucket policy allows
/// public access").
async fn allow_public_access(router: &MultiServiceRouter, bucket: &str) {
    let answer = s3_wire(
        router,
        "PUT",
        &format!("/{bucket}?publicAccessBlock"),
        "<PublicAccessBlockConfiguration>\
         <BlockPublicAcls>false</BlockPublicAcls>\
         <IgnorePublicAcls>false</IgnorePublicAcls>\
         <BlockPublicPolicy>false</BlockPublicPolicy>\
         <RestrictPublicBuckets>false</RestrictPublicBuckets>\
         </PublicAccessBlockConfiguration>",
    )
    .await;
    assert!(
        (200..300).contains(&answer.status),
        "setup: PutPublicAccessBlock({bucket:?}) must succeed, got {} with {}",
        answer.status,
        answer.body
    );
}

/// `PUT /<bucket>?policy` — S3's `PutBucketPolicy`, carrying a document that grants the read
/// to EVERY principal, which is what an anonymous read needs to be served at all.
async fn put_public_bucket_policy(router: &MultiServiceRouter, bucket: &str) {
    let document = format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Effect\":\"Allow\",\
         \"Principal\":\"*\",\"Action\":\"s3:GetObject\",\
         \"Resource\":\"arn:aws:s3:::{bucket}/*\"}}]}}"
    );
    let answer = s3_wire(router, "PUT", &format!("/{bucket}?policy"), &document).await;
    assert!(
        (200..300).contains(&answer.status),
        "setup: PutBucketPolicy({bucket:?}) must succeed, got {} with {}",
        answer.status,
        answer.body
    );
}

/// One created access point: the ARN and the alias its create answered, and the name and
/// account it was created under — together, every address the data path resolves it by.
struct AccessPoint {
    name: String,
    arn: String,
    alias: String,
}

impl AccessPoint {
    /// The resource the access point names one of its objects by:
    /// `<access-point-arn>/object/<key>` (Amazon S3 User Guide, *Access point policy
    /// examples*), which is the resource a refusal about that object names.
    fn object(&self, key: &str) -> String {
        format!("{}/object/{key}", self.arn)
    }

    /// The first label of its endpoint host, `<name>-<account-id>`.
    fn endpoint_label(&self) -> String {
        format!("{}-{ACCOUNT}", self.name)
    }
}

/// `PUT /v20180820/accesspoint/<name>` — S3 Control's `CreateAccessPoint`, bound to `vpc_id`
/// when one is given, answering the access point it created.
async fn create_access_point(
    router: &MultiServiceRouter,
    name: &str,
    bucket: &str,
    vpc_id: Option<&str>,
    public_access_block: Option<bool>,
) -> AccessPoint {
    let configuration = vpc_id.map_or_else(String::new, |vpc_id| {
        format!("<VpcConfiguration><VpcId>{vpc_id}</VpcId></VpcConfiguration>")
    });
    // The four Block Public Access settings of the access point, all set alike when the row
    // states them (`PublicAccessBlockConfiguration`: `BlockPublicAcls`, `IgnorePublicAcls`,
    // `BlockPublicPolicy`, `RestrictPublicBuckets`), and left to the service's default — all
    // four on — when it states none.
    let blocked = public_access_block.map_or_else(String::new, |setting| {
        format!(
            "<PublicAccessBlockConfiguration><BlockPublicAcls>{setting}</BlockPublicAcls>\
             <IgnorePublicAcls>{setting}</IgnorePublicAcls>\
             <BlockPublicPolicy>{setting}</BlockPublicPolicy>\
             <RestrictPublicBuckets>{setting}</RestrictPublicBuckets>\
             </PublicAccessBlockConfiguration>"
        )
    });
    let answer = s3_wire(
        router,
        "PUT",
        &format!("/v20180820/accesspoint/{name}"),
        &format!(
            "<CreateAccessPointRequest xmlns=\"{S3CONTROL_XMLNS}\"><Bucket>{bucket}</Bucket>\
             {configuration}{blocked}</CreateAccessPointRequest>"
        ),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "setup: CreateAccessPoint({name:?}, vpc {vpc_id:?}) must succeed, got {} with {}",
        answer.status, answer.body
    );
    let access_point = AccessPoint {
        name: name.to_string(),
        arn: answer.element("AccessPointArn"),
        alias: answer.element("Alias"),
    };
    assert!(
        access_point.arn.ends_with(&format!(":accesspoint/{name}"))
            && !access_point.alias.is_empty(),
        "setup: CreateAccessPoint({name:?}) must answer the ARN and the alias, got {}",
        answer.body
    );
    access_point
}

/// `GET /v20180820/accesspoint/<name>` — S3 Control's `GetAccessPoint`.
async fn get_access_point(router: &MultiServiceRouter, name: &str) -> Answer {
    s3_wire(router, "GET", &format!("/v20180820/accesspoint/{name}"), "").await
}

/// `PUT /v20180820/accesspoint/<name>/policy` — S3 Control's `PutAccessPointPolicy`.
async fn put_access_point_policy(router: &MultiServiceRouter, name: &str, document: &str) {
    let answer = s3_wire(
        router,
        "PUT",
        &format!("/v20180820/accesspoint/{name}/policy"),
        &format!(
            "<PutAccessPointPolicyRequest xmlns=\"{S3CONTROL_XMLNS}\">\
             <Policy>{document}</Policy></PutAccessPointPolicyRequest>"
        ),
    )
    .await;
    assert_eq!(
        answer.status, 204,
        "PutAccessPointPolicy({name:?}) must succeed, got {} with {}",
        answer.status, answer.body
    );
}

/// The sentence S3 words an authorization refusal in: who was refused, the action, the
/// resource the decision was taken over, and the clause naming what refused it. Written out
/// here rather than built from either service's own helper so the wording has something to be
/// wrong about.
fn denial_message(principal: &str, action: &str, resource: &str, reason: &str) -> String {
    format!(
        "User: {principal} is not authorized to perform: {action} on resource: \
         \"{resource}\" {reason}"
    )
}

/// One ADDRESS an access point is reached by: the case, the host the request is sent to, the
/// path of a request about one key, and the path of a listing of the access point itself.
struct Address {
    case: &'static str,
    host: String,
    path: Box<dyn Fn(&str) -> String>,
    listing: String,
}

/// Every address `access_point` is reached by, each with the request form it arrives in (see
/// the module docs for the set).
fn addresses(access_point: &AccessPoint) -> Vec<Address> {
    let arn = access_point.arn.clone();
    let encoded_arn = form_encode(&access_point.arn);
    let alias = access_point.alias.clone();
    let label = access_point.endpoint_label();
    vec![
        Address {
            case: "the access point ARN as the path's bucket label",
            host: path_style_host(),
            path: Box::new(move |key| format!("/{}/{key}", form_encode(&arn))),
            listing: format!("/{encoded_arn}?list-type=2"),
        },
        Address {
            case: "its alias as the path's bucket label",
            host: path_style_host(),
            path: Box::new({
                let alias = alias.clone();
                move |key| format!("/{alias}/{key}")
            }),
            listing: format!("/{alias}?list-type=2"),
        },
        Address {
            case: "its endpoint host label as the path's bucket label",
            host: path_style_host(),
            path: Box::new({
                let label = label.clone();
                move |key| format!("/{label}/{key}")
            }),
            listing: format!("/{label}?list-type=2"),
        },
        Address {
            case: "its own endpoint host",
            host: format!("{label}.s3.{REGION}.amazonaws.com"),
            path: Box::new(|key| format!("/{key}")),
            listing: "/?list-type=2".to_string(),
        },
    ]
}

/// Rule, first half: a VPC-confined access point refuses the internet-borne request through
/// it — every address, every key and every action — while an Internet-origin access point
/// over the SAME bucket serves the object's bytes and the bucket's own address is untouched.
#[tokio::test]
async fn a_vpc_origin_access_point_refuses_every_internet_borne_request_through_it() {
    let router = build_router();
    create_bucket(&router, BUCKET).await;
    for key in KEYS {
        put_object(&router, BUCKET, key, BODY).await;
    }
    let confined = create_access_point(&router, VPC_ACCESS_POINT, BUCKET, Some(VPC_ID), None).await;
    let open = create_access_point(&router, INTERNET_ACCESS_POINT, BUCKET, None, None).await;
    let caller = caller_arn(&router).await;

    // What the two access points REPORT, read back through the operation that reports it:
    // the network origin each was created with, and the VPC the first one is bound to.
    let described = get_access_point(&router, VPC_ACCESS_POINT).await;
    assert_eq!(
        (
            described.status,
            described.element("NetworkOrigin"),
            described.element("VpcId")
        ),
        (200, VPC_ORIGIN.to_string(), VPC_ID.to_string()),
        "the VPC-origin access point must report {VPC_ORIGIN} and the VPC it is bound to, \
         got {} with {}",
        described.status,
        described.body
    );
    let described_open = get_access_point(&router, INTERNET_ACCESS_POINT).await;
    assert_eq!(
        (described_open.status, described_open.element("NetworkOrigin")),
        (200, INTERNET_ORIGIN.to_string()),
        "the access point created with no VpcConfiguration must report {INTERNET_ORIGIN}, got \
         {} with {}",
        described_open.status,
        described_open.body
    );

    for address in addresses(&confined) {
        let case = address.case;
        for key in KEYS {
            let path = (address.path)(key);
            let refused = s3_at(&router, "GET", &address.host, &path, "").await;
            assert_eq!(
                (refused.status, refused.element("Code")),
                (FORBIDDEN, ACCESS_DENIED.to_string()),
                "case {case}, key {key:?}: a read through the VPC-origin access point must be \
                 refused {FORBIDDEN} {ACCESS_DENIED}, got {} with {}",
                refused.status,
                refused.body
            );
            let expected =
                denial_message(&caller, GET_OBJECT, &confined.object(key), RESOURCE_DENY);
            assert_eq!(
                refused.element("Message"),
                expected,
                "case {case}, key {key:?}: expected {expected:?}, got {:?}",
                refused.element("Message")
            );
            // The HEAD of the same object is the same decision, answered with no body at all.
            let headed = s3_at(&router, "HEAD", &address.host, &path, "").await;
            assert_eq!(
                headed.status, FORBIDDEN,
                "case {case}, key {key:?}: a HEAD through the VPC-origin access point must be \
                 refused {FORBIDDEN}, got {} with {}",
                headed.status, headed.body
            );
        }
        // And so is a listing of the access point itself, over the resource the access point
        // names for the path it is.
        let listed = s3_at(&router, "GET", &address.host, &address.listing, "").await;
        assert_eq!(
            (listed.status, listed.element("Code")),
            (FORBIDDEN, ACCESS_DENIED.to_string()),
            "case {case}: a listing through the VPC-origin access point must be refused \
             {FORBIDDEN} {ACCESS_DENIED}, got {} with {}",
            listed.status,
            listed.body
        );
        let expected = denial_message(&caller, LIST_BUCKET, &confined.arn, RESOURCE_DENY);
        assert_eq!(
            listed.element("Message"),
            expected,
            "case {case}: expected {expected:?}, got {:?}",
            listed.element("Message")
        );
    }

    // The positive control: the Internet-origin access point over the same bucket serves the
    // object's exact bytes through every one of its own addresses.
    for address in addresses(&open) {
        let case = address.case;
        for key in KEYS {
            let served = s3_at(&router, "GET", &address.host, &(address.path)(key), "").await;
            assert_eq!(
                (served.status, served.body.as_str()),
                (200, BODY),
                "case {case}, key {key:?}: the Internet-origin access point must serve the \
                 object, got {} with {:?}",
                served.status,
                served.body
            );
        }
    }

    // And the bucket's own address is untouched by either access point.
    for key in KEYS {
        let served = s3_wire(&router, "GET", &format!("/{BUCKET}/{key}"), "").await;
        assert_eq!(
            (served.status, served.body.as_str()),
            (200, BODY),
            "case the bucket's own address, key {key:?}: the object must be served, got {} \
             with {:?}",
            served.status,
            served.body
        );
    }
}

/// Rule, second half: the confinement is not a document, so no `Allow` in the access point's
/// own policy lifts it — the refusal stays the same code, status and sentence after each
/// grant, while the Internet-origin path over the same bucket keeps serving the object.
#[tokio::test]
async fn an_allow_on_a_vpc_origin_access_point_does_not_lift_its_network_refusal() {
    let router = build_router();
    let bucket = "xsvc-access-point-vpc-policy";
    create_bucket(&router, bucket).await;
    put_object(&router, bucket, KEYS[0], BODY).await;
    // Both access points are created with Block Public Access OFF, so the documents the rows
    // put — one of which grants EVERY principal — are stored rather than refused by that
    // other rule (`BlockPublicPolicy` "causes Amazon S3 to reject calls to PUT Bucket policy
    // if the specified bucket policy allows public access").
    let confined = create_access_point(
        &router,
        "xsvc-vpc-policy-ap",
        bucket,
        Some(VPC_ID),
        Some(false),
    )
    .await;
    let open = create_access_point(
        &router,
        "xsvc-internet-policy-ap",
        bucket,
        None,
        Some(false),
    )
    .await;
    let caller = caller_arn(&router).await;
    let expected = denial_message(
        &caller,
        GET_OBJECT,
        &confined.object(KEYS[0]),
        RESOURCE_DENY,
    );
    let read = format!("/{}/{}", form_encode(&confined.arn), KEYS[0]);

    // (case, the document put on the access point) — `None` is the access point as created,
    // carrying none at all.
    let rows: [(&str, Option<String>); 4] = [
        ("no access point policy", None),
        (
            "an explicit Allow naming the calling account",
            Some(format!(
                "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Effect\":\"Allow\",\
                 \"Principal\":{{\"AWS\":\"arn:aws:iam::{ACCOUNT}:root\"}},\
                 \"Action\":\"s3:GetObject\",\"Resource\":\"{}/object/*\"}}]}}",
                confined.arn
            )),
        ),
        (
            "an explicit Allow for every principal on the object",
            Some(format!(
                "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Effect\":\"Allow\",\
                 \"Principal\":{{\"AWS\":\"*\"}},\"Action\":\"s3:GetObject\",\
                 \"Resource\":\"{}/object/*\"}}]}}",
                confined.arn
            )),
        ),
        (
            "an explicit Allow of every action on the whole access point",
            Some(format!(
                "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Effect\":\"Allow\",\
                 \"Principal\":\"*\",\"Action\":\"s3:*\",\"Resource\":[\"{arn}\",\
                 \"{arn}/object/*\"]}}]}}",
                arn = confined.arn
            )),
        ),
    ];
    for (case, document) in rows {
        if let Some(document) = &document {
            put_access_point_policy(&router, &confined.name, document).await;
        }
        let refused = s3_wire(&router, "GET", &read, "").await;
        assert_eq!(
            (refused.status, refused.element("Code")),
            (FORBIDDEN, ACCESS_DENIED.to_string()),
            "case {case}: the read must stay refused {FORBIDDEN} {ACCESS_DENIED}, got {} with \
             {}",
            refused.status,
            refused.body
        );
        assert_eq!(
            refused.element("Message"),
            expected,
            "case {case}: expected {expected:?}, got {:?}",
            refused.element("Message")
        );
    }

    // The grants are live on the path they govern: the Internet-origin access point over the
    // same bucket serves the object, so what refuses the rows above is the VPC confinement
    // and not a data plane that serves nothing.
    let served = s3_wire(
        &router,
        "GET",
        &format!("/{}/{}", form_encode(&open.arn), KEYS[0]),
        "",
    )
    .await;
    assert_eq!(
        (served.status, served.body.as_str()),
        (200, BODY),
        "the Internet-origin access point over the same bucket must serve the object, got {} \
         with {:?}",
        served.status,
        served.body
    );
}

/// Rule, third half: the confinement holds for the caller that presents NO credential too —
/// the one caller a bucket's own public policy is what admits — so an anonymous read a public
/// bucket serves through an Internet-origin access point is refused through a VPC-origin one.
#[tokio::test]
async fn a_vpc_origin_access_point_refuses_the_anonymous_read_a_public_bucket_serves() {
    let router = build_router();
    let bucket = "xsvc-access-point-vpc-anonymous";
    create_bucket(&router, bucket).await;
    put_object(&router, bucket, KEYS[0], BODY).await;
    // The bucket grants the read to EVERY principal, which is what makes an anonymous read
    // servable at all; without it both arms would be refused for want of a grant and the
    // contrast would say nothing.
    allow_public_access(&router, bucket).await;
    put_public_bucket_policy(&router, bucket).await;
    let confined = create_access_point(
        &router,
        "xsvc-vpc-anonymous-ap",
        bucket,
        Some(VPC_ID),
        Some(false),
    )
    .await;
    let open = create_access_point(
        &router,
        "xsvc-internet-anonymous-ap",
        bucket,
        None,
        Some(false),
    )
    .await;

    // (case, the access point the unsigned read is addressed through, whether it is served)
    let rows: [(&str, &AccessPoint, bool); 2] = [
        ("the Internet-origin access point", &open, true),
        ("the VPC-origin access point", &confined, false),
    ];
    for (case, access_point, served) in rows {
        let answer = s3_anonymous(
            &router,
            "GET",
            &format!("/{}/{}", access_point.alias, KEYS[0]),
        )
        .await;
        if served {
            assert_eq!(
                (answer.status, answer.body.as_str()),
                (200, BODY),
                "case {case}: the unsigned read must be served, got {} with {:?}",
                answer.status,
                answer.body
            );
        } else {
            assert_eq!(
                (answer.status, answer.element("Code")),
                (FORBIDDEN, ACCESS_DENIED.to_string()),
                "case {case}: the unsigned read must be refused {FORBIDDEN} {ACCESS_DENIED}, \
                 got {} with {}",
                answer.status,
                answer.body
            );
        }
    }

    // And the bucket's own address still serves the same unsigned read, so what refused the
    // row above is the access point's network origin and not the bucket's own configuration.
    let direct = s3_anonymous(&router, "GET", &format!("/{bucket}/{}", KEYS[0])).await;
    assert_eq!(
        (direct.status, direct.body.as_str()),
        (200, BODY),
        "the bucket's own address must serve the unsigned read, got {} with {:?}",
        direct.status,
        direct.body
    );
}
