// SPDX-License-Identifier: CC0-1.0

//! The object store beside this service: Amazon S3 and S3 Control mounted on ONE
//! shared engine, the same assembly `core/router` performs, reached over the wire.
//!
//! Several things S3 Control does are facts about, or writes to, Amazon S3's own
//! objects and buckets: a Multi-Region Access Point reports the Region each
//! member BUCKET lives in, and an S3 Batch Operations job reads its manifest and
//! writes its copies, its tags, its generated manifest and its completion report
//! as Amazon S3 objects. Neither can be pinned against this service alone, so the
//! suites that pin them stand both services up here and drive Amazon S3 the way a
//! client drives it — path-style over the wire (`PUT /<bucket>`, `GET
//! /<bucket>/<key>`, `GET /<bucket>?list-type=2`) — so every object a suite reads
//! back is one the object store itself holds.
//!
//! A suite that pins what a job's ROLE decides mounts IAM beside them
//! ([`ObjectStore::with_iam`]), because the role a job runs as exists only if
//! IAM answers for it.

use std::sync::Arc;

use smithy_server_runtime::MultiServiceRouter;

use crate::access_point_lifecycle::{call_in_region, xml_escape, Reply, ACCOUNT, REGION};

/// The XML namespace an Amazon S3 request payload carries.
const S3_XMLNS: &str = "http://s3.amazonaws.com/doc/2006-03-01/";

/// The Region a bucket created with no `LocationConstraint` lives in, and the
/// one Region a create must NOT name a constraint for.
pub(crate) const DEFAULT_BUCKET_REGION: &str = "us-east-1";

/// Amazon S3 and S3 Control on one shared engine.
pub(crate) struct ObjectStore {
    pub(crate) router: MultiServiceRouter,
}

impl ObjectStore {
    /// Both services, mounted on one engine.
    pub(crate) fn start() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // These requests carry an unsigned `Authorization` header, which is what
        // names the service and the Region they route by; verifying the
        // signature is the transport's concern, not these suites'.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_s3control_awss3controlservicev20180820::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
            engine,
        ));
        Self { router }
    }

    /// Amazon S3, S3 Control and IAM on one shared engine, plus an IAM SDK
    /// client over the router's own in-process callback.
    ///
    /// A batch job's work is carried out AS the role the create named, so a
    /// suite that pins what the job's ROLE decides needs the account's IAM
    /// records to be real: the roles it names are created through IAM's own
    /// operations, over this client, and the resolution under test then runs
    /// over the same in-process callback as in production.
    pub(crate) fn with_iam() -> (Self, ::aws_sdk_iam::Client) {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_s3control_awss3controlservicev20180820::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
            Arc::clone(&engine),
        ));
        router.register_service(
            service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(engine),
        );
        let callback = ::smithy_server_runtime::EmbeddedEmulator::from_router(router.clone());
        let iam = ::aws_sdk_iam::Client::from_conf(
            ::aws_sdk_iam::Config::builder()
                .behavior_version(::aws_sdk_iam::config::BehaviorVersion::latest())
                .region(::aws_sdk_iam::config::Region::new(REGION.to_string()))
                .endpoint_url(format!("https://iam.{REGION}.amazonaws.com"))
                .credentials_provider(callback.aws_credentials_provider())
                .http_client(callback.http_client())
                .retry_config(::aws_sdk_iam::config::retry::RetryConfig::disabled())
                .timeout_config(::aws_sdk_iam::config::timeout::TimeoutConfig::disabled())
                .identity_cache(::aws_sdk_iam::config::IdentityCache::no_cache())
                .stalled_stream_protection(
                    ::aws_sdk_iam::config::StalledStreamProtectionConfig::disabled(),
                )
                .build(),
        );
        (Self { router }, iam)
    }

    /// Amazon S3, S3 Control and the TOKEN SERVICE on one shared engine, and that
    /// engine itself.
    ///
    /// A suite that pins what the temporary CREDENTIAL an S3 Access Grants vend hands
    /// out may do needs both: the token service, because the identity a credential
    /// speaks for is the question it owns (`sts:GetCallerIdentity`), and the engine,
    /// because the role that session is of is resolved against the account's IAM
    /// records in the same engine.
    pub(crate) fn with_sts() -> (Self, Arc<engine::InMemoryEngine>) {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_s3control_awss3controlservicev20180820::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
            Arc::clone(&engine),
        ));
        router.register_service(
            service_com_amazonaws_sts_awssecuritytokenservicev20110615::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        (Self { router }, engine)
    }

    /// One S3 Control call, signed for `region` and made for the emulator's own
    /// account.
    pub(crate) async fn s3control(
        &self,
        region: &str,
        method: &str,
        uri: &str,
        body: &str,
    ) -> Reply {
        call_in_region(&self.router, region, method, uri, ACCOUNT, body).await
    }

    /// One path-style Amazon S3 request, signed for `region`: its status and the
    /// body bytes it answered.
    pub(crate) async fn s3(
        &self,
        region: &str,
        method: &str,
        path: &str,
        body: &[u8],
    ) -> (u16, Vec<u8>) {
        let host = format!("s3.{region}.amazonaws.com");
        let request = ::http::Request::builder()
            .method(method)
            .uri(format!("https://{host}{path}"))
            .header("host", &host)
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/s3/aws4_request, \
                     SignedHeaders=host, Signature=0123456789abcdef"
                ),
            )
            .body(::bytes::Bytes::copy_from_slice(body))
            .expect("the request builds");
        let response = self.router.dispatch(request).await;
        (response.status().as_u16(), response.into_body().to_vec())
    }

    /// Create `bucket` in `region` — `PUT /<bucket>`, naming the Region in the
    /// create's `LocationConstraint` for every Region but the default one, which
    /// a create must not name.
    pub(crate) async fn create_bucket(&self, case: &str, region: &str, bucket: &str) {
        let body = if region == DEFAULT_BUCKET_REGION {
            String::new()
        } else {
            format!(
                "<CreateBucketConfiguration xmlns=\"{S3_XMLNS}\">\
                 <LocationConstraint>{region}</LocationConstraint>\
                 </CreateBucketConfiguration>"
            )
        };
        let (status, answered) = self
            .s3(region, "PUT", &format!("/{bucket}"), body.as_bytes())
            .await;
        assert_eq!(
            status,
            200,
            "case {case:?}: the bucket {bucket} must be created in {region} first, got {status} \
             with body {:?}",
            String::from_utf8_lossy(&answered)
        );
    }

    /// Write `body` at `key` in `bucket`, and answer the `ETag` the write
    /// reported.
    pub(crate) async fn put_object(
        &self,
        case: &str,
        region: &str,
        bucket: &str,
        key: &str,
        body: &[u8],
    ) -> String {
        let host = format!("s3.{region}.amazonaws.com");
        let request = ::http::Request::builder()
            .method("PUT")
            .uri(format!("https://{host}/{bucket}/{key}"))
            .header("host", &host)
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/s3/aws4_request, \
                     SignedHeaders=host, Signature=0123456789abcdef"
                ),
            )
            .body(::bytes::Bytes::copy_from_slice(body))
            .expect("the request builds");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let e_tag = response
            .headers()
            .get("etag")
            .map(|value| String::from_utf8_lossy(value.as_bytes()).to_string())
            .unwrap_or_default();
        assert_eq!(
            status, 200,
            "case {case:?}: the object {bucket}/{key} must be written first, got {status}"
        );
        e_tag
    }

    /// The status and bytes `bucket` answers at `key`.
    pub(crate) async fn object(&self, region: &str, bucket: &str, key: &str) -> (u16, Vec<u8>) {
        self.s3(region, "GET", &format!("/{bucket}/{key}"), b"").await
    }

    /// Every key `bucket` holds under `prefix`, read through `ListObjectsV2`.
    pub(crate) async fn keys_under(&self, region: &str, bucket: &str, prefix: &str) -> Vec<String> {
        let (status, body) = self
            .s3(
                region,
                "GET",
                &format!("/{bucket}?list-type=2&prefix={prefix}"),
                b"",
            )
            .await;
        assert_eq!(
            status,
            200,
            "listing {bucket}/{prefix} must answer 200, got {status} with body {:?}",
            String::from_utf8_lossy(&body)
        );
        elements(&String::from_utf8_lossy(&body), "Key")
    }

    /// Put `tags` on `key` through Amazon S3's own `PutObjectTagging`, so a
    /// suite that pins what a job's action does to an object's tag set can start
    /// from an object that already carries one.
    pub(crate) async fn put_object_tags(
        &self,
        case: &str,
        region: &str,
        bucket: &str,
        key: &str,
        tags: &[(&str, &str)],
    ) {
        let mut body = format!("<Tagging xmlns=\"{S3_XMLNS}\"><TagSet>");
        for (tag_key, value) in tags {
            body.push_str(&format!(
                "<Tag><Key>{}</Key><Value>{}</Value></Tag>",
                xml_escape(tag_key),
                xml_escape(value)
            ));
        }
        body.push_str("</TagSet></Tagging>");
        let (status, answered) = self
            .s3(
                region,
                "PUT",
                &format!("/{bucket}/{key}?tagging"),
                body.as_bytes(),
            )
            .await;
        assert_eq!(
            status,
            200,
            "case {case:?}: the tags of {bucket}/{key} must be written first, got {status} with \
             body {:?}",
            String::from_utf8_lossy(&answered)
        );
    }

    /// The tag set `bucket` holds on `key`, as (key, value) pairs in the order
    /// the answer carries them.
    pub(crate) async fn object_tags(
        &self,
        region: &str,
        bucket: &str,
        key: &str,
    ) -> Vec<(String, String)> {
        let (status, body) = self
            .s3(region, "GET", &format!("/{bucket}/{key}?tagging"), b"")
            .await;
        assert_eq!(
            status,
            200,
            "reading the tags of {bucket}/{key} must answer 200, got {status} with body {:?}",
            String::from_utf8_lossy(&body)
        );
        let document = String::from_utf8_lossy(&body).into_owned();
        elements(&document, "Key")
            .into_iter()
            .zip(elements(&document, "Value"))
            .collect()
    }
}

/// The text of every `<name>` element of `document`, in document order.
pub(crate) fn elements(document: &str, name: &str) -> Vec<String> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    document
        .split(&open)
        .skip(1)
        .filter_map(|tail| tail.split(&close).next())
        .map(str::to_string)
        .collect()
}
