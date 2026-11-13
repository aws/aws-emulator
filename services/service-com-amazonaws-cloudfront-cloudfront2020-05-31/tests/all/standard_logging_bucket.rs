// SPDX-License-Identifier: CC0-1.0

//! STANDARD LOGGING to Amazon S3: which log bucket a `DistributionConfig` may name, and the
//! grant enabling logging leaves on the bucket it names — end to end over HTTP.
//!
//! Amazon S3 is MOUNTED beside CloudFront on the same engine, because both halves of the
//! rule are facts about a BUCKET: the buckets below are created through S3's own API, their
//! Object Ownership setting is the one a client asked for, and the access control list every
//! assertion reads is the one S3 reports.
//!
//! The rule under test: CloudFront delivers a standard access log by writing an object into
//! the bucket the configuration names, as the log-delivery account — so
//!
//! 1. a configuration that ENABLES logging to a bucket whose ACLs are DISABLED (Object
//!    Ownership `BucketOwnerEnforced`) is refused with `InvalidArgument` (HTTP 400) naming
//!    that bucket, and nothing is created;
//! 2. a configuration accepted against a bucket whose ACLs are ENABLED leaves the
//!    log-delivery account holding `FULL_CONTROL` on that bucket, beside the grants the
//!    bucket already carried;
//! 3. the same two answers hold wherever a configuration is SUBMITTED — the create, the
//!    create that carries tags, and the update — because it is the configuration that is
//!    judged, not the operation;
//! 4. a configuration that enables no logging is accepted and leaves the bucket's ACL
//!    untouched.
//!
//! The rows differ along everything the rule generalises over: which Object Ownership
//! setting the bucket carries (every value `ObjectOwnership` admits, plus a bucket whose
//! controls were withdrawn), the FORM of the bucket's endpoint host (the global one and a
//! regional one), whether the bucket's name carries dots, and whether the configuration
//! names a prefix and asks for cookies.

use aws_sdk_s3::types::{ObjectOwnership, Permission, Type};
use smithy_server_runtime::{EmbeddedEmulator, MultiServiceRouter};

use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::LOG_DELIVERY_CANONICAL_USER_ID;

use crate::distribution_lifecycle::{call, Answer, API_VERSION};

/// The Region this suite's buckets are created in, and the Region the global endpoint host
/// below addresses.
const REGION: &str = "us-east-1";

/// A Region OTHER than [`REGION`], for the row whose log bucket is named by a regional
/// endpoint host: one object store holds every bucket, so the host's Region is addressing
/// rather than a second store.
const OTHER_REGION: &str = "us-west-2";

// ---------------------------------------------------------------------------
// The emulator: CloudFront plus the object store its log buckets live in
// ---------------------------------------------------------------------------

/// CloudFront and Amazon S3 over ONE shared engine, with a callback into the same router, so
/// this suite creates buckets and reads their ACLs through S3's own API — the very seam the
/// handler reaches them over.
struct Emulator {
    router: MultiServiceRouter,
    callback: EmbeddedEmulator,
}

impl Emulator {
    fn new() -> Self {
        let engine: std::sync::Arc<engine::InMemoryEngine> =
            std::sync::Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_cloudfront_cloudfront2020_05_31::EmulatedService::new(
                std::sync::Arc::clone(&engine),
            ),
        );
        router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
            std::sync::Arc::clone(&engine),
        ));
        let callback = EmbeddedEmulator::from_router(router.clone());
        Self { router, callback }
    }

    /// The Amazon S3 client, built by the one shared constructor over the in-process
    /// callback, so this suite's buckets are the ones the handler reads.
    fn s3(&self) -> aws_sdk_s3::Client {
        service_common::s3_client::s3_client(&self.callback, REGION)
    }

    /// Create `bucket` with the Object Ownership setting `ownership` asks for.
    async fn bucket(&self, bucket: &str, ownership: Ownership) {
        let mut create = self.s3().create_bucket().bucket(bucket);
        if let Some(setting) = ownership.object_ownership() {
            create = create.object_ownership(setting);
        }
        create
            .send()
            .await
            .unwrap_or_else(|error| panic!("creating {bucket} must succeed, got {error:?}"));
        if ownership == Ownership::ControlsWithdrawn {
            self.s3()
                .delete_bucket_ownership_controls()
                .bucket(bucket)
                .send()
                .await
                .unwrap_or_else(|error| {
                    panic!("withdrawing the controls of {bucket} must succeed, got {error:?}")
                });
        }
    }

    /// Every `(canonical user id, permission)` pair the bucket's access control list grants,
    /// as Amazon S3 reports it.
    async fn acl_grants(&self, bucket: &str) -> Vec<(String, String)> {
        let answered = self
            .s3()
            .get_bucket_acl()
            .bucket(bucket)
            .send()
            .await
            .unwrap_or_else(|error| panic!("reading the ACL of {bucket} must succeed, got {error:?}"));
        answered
            .grants()
            .iter()
            .map(|grant| {
                (
                    grant
                        .grantee()
                        .and_then(|grantee| grantee.id())
                        .unwrap_or_default()
                        .to_string(),
                    grant
                        .permission()
                        .map(|permission| permission.as_str().to_string())
                        .unwrap_or_default(),
                )
            })
            .collect()
    }

    /// How many grants of `bucket`'s ACL are the log-delivery account's `FULL_CONTROL`.
    async fn log_delivery_grants(&self, bucket: &str) -> usize {
        self.acl_grants(bucket)
            .await
            .into_iter()
            .filter(|(id, permission)| {
                id == LOG_DELIVERY_CANONICAL_USER_ID
                    && permission == Permission::FullControl.as_str()
            })
            .count()
    }
}

// ---------------------------------------------------------------------------
// The buckets, and the configurations that name them
// ---------------------------------------------------------------------------

/// Which Object Ownership setting a log bucket carries.
///
/// The three values are `ObjectOwnership`'s own
/// (`smithy_query(query="shape",shape="ObjectOwnership")`: `BucketOwnerEnforced`,
/// `BucketOwnerPreferred`, `ObjectWriter`), plus the two states that are not a value at all:
/// a bucket whose ownership controls were WITHDRAWN, and a bucket the object store does not
/// hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ownership {
    BucketOwnerEnforced,
    BucketOwnerPreferred,
    ObjectWriter,
    ControlsWithdrawn,
    NoSuchBucket,
}

impl Ownership {
    /// The `x-amz-object-ownership` value a create carries for this setting, or `None` where
    /// the create names none.
    fn object_ownership(self) -> Option<ObjectOwnership> {
        match self {
            Self::BucketOwnerEnforced => Some(ObjectOwnership::BucketOwnerEnforced),
            // A bucket whose controls are withdrawn is created with ACLs enabled first:
            // what is under test is the WITHDRAWN state, not the create's own setting.
            Self::BucketOwnerPreferred | Self::ControlsWithdrawn => {
                Some(ObjectOwnership::BucketOwnerPreferred)
            }
            Self::ObjectWriter => Some(ObjectOwnership::ObjectWriter),
            Self::NoSuchBucket => None,
        }
    }

    /// Whether a bucket with this setting has ACLs DISABLED, which is what decides the
    /// refusal.
    fn acls_disabled(self) -> bool {
        self == Self::BucketOwnerEnforced
    }
}

/// One row: a log bucket, how it is named in `LoggingConfig$Bucket`, and what the
/// configuration naming it asks for.
#[derive(Debug, Clone)]
struct Row {
    case: &'static str,
    bucket: &'static str,
    ownership: Ownership,
    /// The Region the bucket's endpoint host addresses, or `None` for the GLOBAL host form
    /// (`<bucket>.s3.amazonaws.com`), which addresses none.
    host_region: Option<&'static str>,
    include_cookies: bool,
    prefix: &'static str,
}

impl Row {
    /// `LoggingConfig$Bucket`: "The Amazon S3 bucket to store the access logs in, for
    /// example, `amzn-s3-demo-bucket.s3.amazonaws.com`" — the bucket's endpoint host.
    fn logging_host(&self) -> String {
        match self.host_region {
            Some(region) => format!("{}.s3.{region}.amazonaws.com", self.bucket),
            None => format!("{}.s3.amazonaws.com", self.bucket),
        }
    }

    /// The `DistributionConfig` XML for this row, with standard logging `enabled` or not.
    fn config(&self, reference: &str, enabled: bool) -> String {
        distribution_config(reference, Some((enabled, &self.logging_host(), self))) 
    }
}

/// The rows: every Object Ownership state a log bucket can be in, across both forms of the
/// endpoint host, with a dotted bucket name among them.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "ACLs disabled, global endpoint host",
            bucket: "cf-logs-enforced",
            ownership: Ownership::BucketOwnerEnforced,
            host_region: None,
            include_cookies: false,
            prefix: "",
        },
        Row {
            case: "ACLs disabled, regional endpoint host, dotted bucket name",
            bucket: "cf.logs.enforced.regional",
            ownership: Ownership::BucketOwnerEnforced,
            host_region: Some(OTHER_REGION),
            include_cookies: true,
            prefix: "cdn/",
        },
        Row {
            case: "ACLs enabled, the bucket owner preferred",
            bucket: "cf-logs-preferred",
            ownership: Ownership::BucketOwnerPreferred,
            host_region: None,
            include_cookies: true,
            prefix: "preferred/",
        },
        Row {
            case: "ACLs enabled, the object writer owns, regional endpoint host",
            bucket: "cf-logs-object-writer",
            ownership: Ownership::ObjectWriter,
            host_region: Some(OTHER_REGION),
            include_cookies: false,
            prefix: "",
        },
        Row {
            case: "ACLs enabled, the bucket's ownership controls withdrawn",
            bucket: "cf.logs.withdrawn",
            ownership: Ownership::ControlsWithdrawn,
            host_region: None,
            include_cookies: false,
            prefix: "withdrawn/",
        },
        Row {
            case: "a bucket the object store does not hold",
            bucket: "cf-logs-absent",
            ownership: Ownership::NoSuchBucket,
            host_region: None,
            include_cookies: false,
            prefix: "",
        },
    ]
}

/// A `DistributionConfig` document carrying one origin, one default cache behaviour and —
/// where `logging` says so — a `Logging` element, in the order `DistributionConfig` declares
/// its members.
fn distribution_config(reference: &str, logging: Option<(bool, &str, &Row)>) -> String {
    let logging = match logging {
        Some((enabled, host, row)) => format!(
            "<Logging><Enabled>{enabled}</Enabled>\
             <IncludeCookies>{}</IncludeCookies>\
             <Bucket>{host}</Bucket><Prefix>{}</Prefix></Logging>",
            row.include_cookies, row.prefix
        ),
        None => String::new(),
    };
    format!(
        "<DistributionConfig xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
         <CallerReference>{reference}</CallerReference>\
         <Origins><Quantity>1</Quantity><Items><Origin><Id>primary</Id>\
         <DomainName>origin.example.com</DomainName>\
         <CustomOriginConfig><HTTPPort>80</HTTPPort><HTTPSPort>443</HTTPSPort>\
         <OriginProtocolPolicy>https-only</OriginProtocolPolicy></CustomOriginConfig>\
         </Origin></Items></Origins>\
         <DefaultCacheBehavior><TargetOriginId>primary</TargetOriginId>\
         <ViewerProtocolPolicy>allow-all</ViewerProtocolPolicy></DefaultCacheBehavior>\
         <Comment>standard logging</Comment>{logging}\
         <PriceClass>PriceClass_All</PriceClass><Enabled>true</Enabled></DistributionConfig>"
    )
}

/// `CreateDistribution` with `config`.
async fn create(router: &MultiServiceRouter, config: &str) -> Answer {
    call(
        router,
        "POST",
        &format!("/{API_VERSION}/distribution"),
        Some(config),
        None,
    )
    .await
}

/// `CreateDistributionWithTags` with `config` and one tag.
async fn create_with_tags(router: &MultiServiceRouter, config: &str) -> Answer {
    let body = format!(
        "<DistributionConfigWithTags \
         xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">{}<Tags><Items>\
         <Tag><Key>suite</Key><Value>standard-logging</Value></Tag></Items></Tags>\
         </DistributionConfigWithTags>",
        config.replacen(
            &format!(" xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\""),
            "",
            1
        )
    );
    call(
        router,
        "POST",
        &format!("/{API_VERSION}/distribution?WithTags"),
        Some(&body),
        None,
    )
    .await
}

/// The sentence CloudFront refuses a log bucket with ACLs disabled with, as this suite
/// expects it: the service's fixed text, naming the bucket the request itself specified.
fn acl_access_not_enabled(specified: &str) -> String {
    format!(
        "The S3 bucket that you specified for CloudFront logs does not enable ACL access: \
         {specified}"
    )
}

// ---------------------------------------------------------------------------
// The rules
// ---------------------------------------------------------------------------

/// RULE 1 and 2, over the create: a configuration enabling standard logging to a bucket with
/// ACLs disabled is refused and creates nothing, and one accepted against a bucket with ACLs
/// enabled leaves the log-delivery account holding `FULL_CONTROL` on it — beside the owner's
/// own grant, which is not taken away.
#[tokio::test]
async fn a_create_enabling_standard_logging_is_judged_against_the_log_bucket() {
    let emulator = Emulator::new();
    for row in rows() {
        let case = row.case;
        if row.ownership != Ownership::NoSuchBucket {
            emulator.bucket(row.bucket, row.ownership).await;
        }
        let owner_grants = if row.ownership == Ownership::NoSuchBucket {
            Vec::new()
        } else {
            emulator.acl_grants(row.bucket).await
        };

        let answered = create(
            &emulator.router,
            &row.config(&format!("logging-create-{}", row.bucket), true),
        )
        .await;

        if row.ownership.acls_disabled() {
            assert_eq!(
                answered.status, 400,
                "case {case:?}: expected the create to be refused 400, got {} and {}",
                answered.status, answered.raw
            );
            assert_eq!(
                answered.error_code(),
                "InvalidArgument",
                "case {case:?}: expected InvalidArgument, got {:?} in {}",
                answered.error_code(),
                answered.raw
            );
            let message = answered.root().find_text("Message").unwrap_or_default();
            assert_eq!(
                message,
                acl_access_not_enabled(&row.logging_host()),
                "case {case:?}: expected the refusal to name the bucket, got {message:?}"
            );
            assert!(
                answered.root().find_text("Id").is_none(),
                "case {case:?}: expected no distribution to be reported, got {}",
                answered.raw
            );
            let after = emulator.acl_grants(row.bucket).await;
            assert_eq!(
                after, owner_grants,
                "case {case:?}: expected a refused create to leave the ACL alone, got {after:?}"
            );
            continue;
        }

        assert_eq!(
            answered.status, 201,
            "case {case:?}: expected the create to be accepted 201, got {} and {}",
            answered.status, answered.raw
        );
        if row.ownership == Ownership::NoSuchBucket {
            // Nothing was established about a bucket the store does not hold, so the create
            // is neither refused nor followed by a grant on a bucket that is not there.
            continue;
        }
        let granted = emulator.log_delivery_grants(row.bucket).await;
        assert_eq!(
            granted, 1,
            "case {case:?}: expected the log-delivery account to hold FULL_CONTROL once, got \
             {granted} in {:?}",
            emulator.acl_grants(row.bucket).await
        );
        let after = emulator.acl_grants(row.bucket).await;
        for grant in &owner_grants {
            assert!(
                after.contains(grant),
                "case {case:?}: expected the grant {grant:?} to be kept, got {after:?}"
            );
        }
    }
}

/// RULE 4: a configuration that enables NO logging names no log bucket to judge, so it is
/// accepted even against a bucket whose ACLs are disabled — and the bucket's ACL is left
/// exactly as it was.
#[tokio::test]
async fn a_configuration_that_enables_no_logging_touches_no_bucket() {
    let emulator = Emulator::new();
    for row in rows()
        .into_iter()
        .filter(|row| row.ownership != Ownership::NoSuchBucket)
    {
        let case = row.case;
        emulator.bucket(row.bucket, row.ownership).await;
        let before = emulator.acl_grants(row.bucket).await;

        let answered = create(
            &emulator.router,
            &row.config(&format!("logging-off-{}", row.bucket), false),
        )
        .await;

        assert_eq!(
            answered.status, 201,
            "case {case:?}: expected a configuration with logging off to be accepted, got {} \
             and {}",
            answered.status, answered.raw
        );
        let after = emulator.acl_grants(row.bucket).await;
        assert_eq!(
            after, before,
            "case {case:?}: expected the bucket's ACL to be untouched, got {after:?}"
        );
    }
}

/// RULE 3: the rule is the CONFIGURATION's, not one operation's — so the create that carries
/// tags answers exactly as the plain create does, refusal and grant alike.
#[tokio::test]
async fn the_create_that_carries_tags_judges_the_log_bucket_the_same_way() {
    let emulator = Emulator::new();
    for row in rows()
        .into_iter()
        .filter(|row| row.ownership != Ownership::NoSuchBucket)
    {
        let case = row.case;
        emulator.bucket(row.bucket, row.ownership).await;

        let answered = create_with_tags(
            &emulator.router,
            &row.config(&format!("logging-tags-{}", row.bucket), true),
        )
        .await;

        if row.ownership.acls_disabled() {
            assert_eq!(
                answered.status, 400,
                "case {case:?}: expected the tagged create to be refused 400, got {} and {}",
                answered.status, answered.raw
            );
            assert_eq!(
                answered.error_code(),
                "InvalidArgument",
                "case {case:?}: expected InvalidArgument, got {:?} in {}",
                answered.error_code(),
                answered.raw
            );
            continue;
        }
        assert_eq!(
            answered.status, 201,
            "case {case:?}: expected the tagged create to be accepted 201, got {} and {}",
            answered.status, answered.raw
        );
        let granted = emulator.log_delivery_grants(row.bucket).await;
        assert_eq!(
            granted, 1,
            "case {case:?}: expected the log-delivery account to hold FULL_CONTROL once, got \
             {granted}"
        );
    }
}

/// RULE 3, over the UPDATE: a distribution that logs nowhere is judged the same way when an
/// update turns logging on — refused against a bucket with ACLs disabled, and granted against
/// one with ACLs enabled. The grant is applied ONCE however many distributions log there.
#[tokio::test]
async fn an_update_that_turns_standard_logging_on_is_judged_the_same_way() {
    let emulator = Emulator::new();
    for row in rows()
        .into_iter()
        .filter(|row| row.ownership != Ownership::NoSuchBucket)
    {
        let case = row.case;
        emulator.bucket(row.bucket, row.ownership).await;
        let reference = format!("logging-update-{}", row.bucket);

        let created = create(&emulator.router, &row.config(&reference, false)).await;
        assert_eq!(
            created.status, 201,
            "case {case:?}: expected the create to be accepted, got {} and {}",
            created.status, created.raw
        );
        let id = created
            .root()
            .find_text("Id")
            .expect("a created distribution reports its id")
            .to_string();
        let etag = created.header("etag").to_string();

        let answered = call(
            &emulator.router,
            "PUT",
            &format!("/{API_VERSION}/distribution/{id}/config"),
            Some(&row.config(&reference, true)),
            Some(&etag),
        )
        .await;

        if row.ownership.acls_disabled() {
            assert_eq!(
                answered.status, 400,
                "case {case:?}: expected the update to be refused 400, got {} and {}",
                answered.status, answered.raw
            );
            assert_eq!(
                answered.error_code(),
                "InvalidArgument",
                "case {case:?}: expected InvalidArgument, got {:?} in {}",
                answered.error_code(),
                answered.raw
            );
            let granted = emulator.log_delivery_grants(row.bucket).await;
            assert_eq!(
                granted, 0,
                "case {case:?}: expected a refused update to grant nothing, got {granted}"
            );
            continue;
        }

        assert_eq!(
            answered.status, 200,
            "case {case:?}: expected the update to be accepted 200, got {} and {}",
            answered.status, answered.raw
        );
        let granted = emulator.log_delivery_grants(row.bucket).await;
        assert_eq!(
            granted, 1,
            "case {case:?}: expected the log-delivery account to hold FULL_CONTROL once, got \
             {granted}"
        );

        // A SECOND distribution logging to the same bucket needs the same one grant: the
        // ACL is read and added to, never duplicated.
        let second = create(
            &emulator.router,
            &row.config(&format!("{reference}-second"), true),
        )
        .await;
        assert_eq!(
            second.status, 201,
            "case {case:?}: expected a second logging distribution to be accepted, got {} and {}",
            second.status, second.raw
        );
        let granted = emulator.log_delivery_grants(row.bucket).await;
        assert_eq!(
            granted, 1,
            "case {case:?}: expected exactly one log-delivery grant after a second \
             distribution, got {granted}"
        );
    }
}

/// The grantee the log-delivery grant names is a CANONICAL USER, which is how an account is
/// named in an ACL — so a client reading the bucket's ACL finds the account by its canonical
/// user id.
#[tokio::test]
async fn the_log_delivery_grant_names_a_canonical_user() {
    let emulator = Emulator::new();
    let row = Row {
        case: "the canonical user the grant names",
        bucket: "cf-logs-grantee-type",
        ownership: Ownership::BucketOwnerPreferred,
        host_region: None,
        include_cookies: false,
        prefix: "",
    };
    emulator.bucket(row.bucket, row.ownership).await;
    let answered = create(&emulator.router, &row.config("logging-grantee-type", true)).await;
    assert_eq!(
        answered.status, 201,
        "case {:?}: expected the create to be accepted, got {} and {}",
        row.case, answered.status, answered.raw
    );
    let grantee_types: Vec<String> = emulator
        .s3()
        .get_bucket_acl()
        .bucket(row.bucket)
        .send()
        .await
        .expect("reading the ACL must succeed")
        .grants()
        .iter()
        .filter(|grant| {
            grant
                .grantee()
                .and_then(|grantee| grantee.id())
                .is_some_and(|id| id == LOG_DELIVERY_CANONICAL_USER_ID)
        })
        .filter_map(|grant| grant.grantee().map(|grantee| grantee.r#type().as_str().to_string()))
        .collect();
    assert_eq!(
        grantee_types,
        vec![Type::CanonicalUser.as_str().to_string()],
        "case {:?}: expected one CanonicalUser grantee, got {grantee_types:?}",
        row.case
    );
}
