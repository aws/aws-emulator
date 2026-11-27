// SPDX-License-Identifier: CC0-1.0

//! The KIND an origin states it is, against the origin access control attached to it, end to
//! end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing, restXml
//! (de)serialization, the handler, the persisted state and serialization — the same path a
//! client takes. The rule is the second half of the one
//! [`crate::origin_access_control_origin_type_pairing`] asserts over an origin's ADDRESS: an
//! origin access control signs the origin request CloudFront makes to the ONE service its
//! `OriginAccessControlOriginType` names, so it may front only an origin of that kind — and the
//! configuration member the origin is SPECIFIED with is the model's own statement of that kind:
//!
//! * `S3OriginConfig` is "an origin that is an Amazon S3 bucket that is not configured with
//!   static website hosting", so it is the kind the `s3` control is for;
//! * `CustomOriginConfig` is "an origin that is not an Amazon S3 bucket", with the one exception
//!   of a bucket's static-website surface — which is a different server from the bucket's REST
//!   API and which no `s3` control may front either;
//! * `VpcOriginConfig` is "a VPC origin", which is no Amazon S3 bucket.
//!
//! So a configuration whose origin names an `s3` control while specifying itself as anything
//! but an Amazon S3 bucket — and one whose origin names a control of any OTHER type while
//! specifying itself as a bucket — is refused `IllegalOriginAccessConfiguration` (HTTP 400) and
//! stores nothing, however the origin is ADDRESSED: the two halves of the kind have to agree
//! with the control, not merely one of them.
//!
//! The rows differ along what the rule generalises over: the configuration member the origin is
//! specified with (all three), crossed with the origin type the control carries (`s3` and the
//! other types the model admits), at an address that names the control's own service — so the
//! address can never be what decides a row. The call that submits the configuration is varied
//! too (`CreateDistribution`, `CreateDistributionWithTags`, `UpdateDistribution`), since each
//! stores a configuration its caller submitted.
//!
//! The expected sentence is a LITERAL here rather than the crate's own constant, so a constant
//! edited to some other wording fails this suite. The harness (the router and the request
//! helpers) is the distribution and origin access control suites', reused rather than copied.

use smithy_server_runtime::MultiServiceRouter;

use crate::distribution_lifecycle::{
    call, create_distribution, emulator, get_distribution, list_distributions, update_distribution,
    Answer, API_VERSION,
};
use crate::origin_access_control_lifecycle::create_control;

/// The AWS-managed cache policy `Managed-CachingOptimized`, which the default cache behaviour
/// of every configuration below names.
const CACHING_OPTIMIZED: &str = "658327ea-f89d-4fab-a63d-7e88639e58f6";

/// The sentence `IllegalOriginAccessConfiguration` carries for a control attached to an origin
/// of another kind.
const ILLEGAL_PAIRING: &str = "Illegal configuration: The origin type and OAC origin type differ";

/// The configuration member an origin is specified with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Specified {
    /// `S3OriginConfig`, with the empty `OriginAccessIdentity` the model documents for a bucket
    /// reached through an origin access control.
    S3Bucket,
    /// `CustomOriginConfig`.
    CustomOrigin,
    /// `VpcOriginConfig`, which names a VPC origin resource by id.
    VpcOrigin,
}

impl Specified {
    fn case(self) -> &'static str {
        match self {
            Specified::S3Bucket => "specified as an Amazon S3 bucket",
            Specified::CustomOrigin => "specified as a custom origin",
            Specified::VpcOrigin => "specified as a VPC origin",
        }
    }

    /// The member XML an origin carries for this kind.
    fn xml(self) -> &'static str {
        match self {
            Specified::S3Bucket => {
                "<S3OriginConfig><OriginAccessIdentity></OriginAccessIdentity></S3OriginConfig>"
            }
            Specified::CustomOrigin => {
                "<CustomOriginConfig><HTTPPort>80</HTTPPort><HTTPSPort>443</HTTPSPort>\
                 <OriginProtocolPolicy>https-only</OriginProtocolPolicy></CustomOriginConfig>"
            }
            Specified::VpcOrigin => {
                "<VpcOriginConfig><VpcOriginId>vo_0123456789ABC</VpcOriginId></VpcOriginConfig>"
            }
        }
    }

    /// Whether this member states the origin IS an Amazon S3 bucket, which is the one kind an
    /// `s3` control may front.
    fn is_an_s3_bucket(self) -> bool {
        self == Specified::S3Bucket
    }
}

/// One origin type under test, with an address that names the service it is for — so no row is
/// ever decided by the address.
struct OriginType {
    /// The `OriginAccessControlOriginType` the control carries.
    origin_type: &'static str,
    /// An `Origin$DomainName` at that type's own service endpoint.
    domain_name: &'static str,
}

/// Every origin type the model admits
/// (`smithy_query(query="shape",shape="OriginAccessControlOriginTypes")`: `s3`, `mediastore`,
/// `mediapackagev2`, `lambda`), each at an address its own service publishes.
const ORIGIN_TYPES: [OriginType; 4] = [
    OriginType {
        origin_type: "s3",
        domain_name: "reports.s3.us-east-1.amazonaws.com",
    },
    OriginType {
        origin_type: "mediastore",
        domain_name: "a1b2c3.data.mediastore.us-east-1.amazonaws.com",
    },
    OriginType {
        origin_type: "mediapackagev2",
        domain_name: "a1b2c3.egress.d4e5f6.mediapackagev2.us-east-1.amazonaws.com",
    },
    OriginType {
        origin_type: "lambda",
        domain_name: "0123456789abcdef0123456789abcdef.lambda-url.us-east-1.on.aws",
    },
];

/// One origin access control of `origin_type`, created through the API; answers its `Id`.
async fn control(router: &MultiServiceRouter, case: &str, name: &str, origin_type: &str) -> String {
    let body = format!(
        "<OriginAccessControlConfig \
         xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
         <Name>{name}</Name><Description>the origin kind pairing suite</Description>\
         <SigningProtocol>sigv4</SigningProtocol><SigningBehavior>always</SigningBehavior>\
         <OriginAccessControlOriginType>{origin_type}</OriginAccessControlOriginType>\
         </OriginAccessControlConfig>"
    );
    let created = create_control(router, &body).await;
    assert_eq!(
        created.status, 201,
        "case {case}: expected HTTP 201 from CreateOriginAccessControl, got {} with {}",
        created.status, created.raw
    );
    created
        .root()
        .find_text("Id")
        .unwrap_or_else(|| panic!("case {case}: the created control reports no Id"))
        .to_string()
}

/// The `DistributionConfig` XML for one origin at `domain_name`, specified with `specified` and
/// naming `control`. Members follow the model's declaration order for `Origin`.
fn config_xml(
    reference: &str,
    comment: &str,
    domain_name: &str,
    specified: Specified,
    control: Option<&str>,
) -> String {
    let attached = control
        .map(|id| format!("<OriginAccessControlId>{id}</OriginAccessControlId>"))
        .unwrap_or_default();
    format!(
        "<DistributionConfig xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
         <CallerReference>{reference}</CallerReference>\
         <Origins><Quantity>1</Quantity><Items><Origin>\
         <Id>primary</Id><DomainName>{domain_name}</DomainName>{}{attached}\
         </Origin></Items></Origins>\
         <DefaultCacheBehavior><TargetOriginId>primary</TargetOriginId>\
         <ViewerProtocolPolicy>https-only</ViewerProtocolPolicy>\
         <CachePolicyId>{CACHING_OPTIMIZED}</CachePolicyId></DefaultCacheBehavior>\
         <Comment>{comment}</Comment>\
         <PriceClass>PriceClass_All</PriceClass>\
         <Enabled>false</Enabled></DistributionConfig>",
        specified.xml()
    )
}

/// `POST /2020-05-31/distribution?WithTags`, whose payload wraps a `DistributionConfig` in a
/// `DistributionConfigWithTags`.
async fn create_distribution_with_tags(router: &MultiServiceRouter, config: &str) -> Answer {
    let body = format!(
        "<DistributionConfigWithTags \
         xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">{config}<Tags/>\
         </DistributionConfigWithTags>"
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

/// Assert `answer` is the refusal the rule states: HTTP 400, `IllegalOriginAccessConfiguration`
/// and the sentence this code is served with, naming nothing out of the request.
fn assert_refused(answer: &Answer, case: &str, operation: &str, named: &[&str]) {
    assert_eq!(
        answer.status, 400,
        "case {case} ({operation}): expected HTTP 400 for a control fronting another kind of \
         origin, got {} with {}",
        answer.status, answer.raw
    );
    assert_eq!(
        answer.error_code(),
        "IllegalOriginAccessConfiguration",
        "case {case} ({operation}): expected IllegalOriginAccessConfiguration, got {:?} with {}",
        answer.error_code(),
        answer.raw
    );
    let stated = answer.root().find_text("Message").unwrap_or_default();
    assert_eq!(
        stated, ILLEGAL_PAIRING,
        "case {case} ({operation}): expected the sentence {ILLEGAL_PAIRING:?}, got {stated:?}"
    );
    for value in named {
        assert!(
            !stated.contains(value),
            "case {case} ({operation}): expected nothing of the request in the sentence, got \
             {stated:?}"
        );
    }
}

/// RULE: a create admits a control on an origin only while the member the origin is SPECIFIED
/// with states the kind that control's type is for, at an address that names that type's own
/// service — so the configuration member alone decides every row here.
#[tokio::test]
async fn a_create_reads_the_kind_the_origin_specifies_itself_as() {
    for kind in [
        Specified::S3Bucket,
        Specified::CustomOrigin,
        Specified::VpcOrigin,
    ] {
        for subject in &ORIGIN_TYPES {
            let case = format!("a {} control on an origin {}", subject.origin_type, kind.case());
            let router = emulator();
            let control_id = control(&router, &case, "kind-control", subject.origin_type).await;
            let reference = format!("kind-{}-{:?}", subject.origin_type, kind);
            let comment = format!("the configuration of {case}");
            let config = config_xml(
                &reference,
                &comment,
                subject.domain_name,
                kind,
                Some(&control_id),
            );

            // The `s3` control is for the bucket kind and every other type is for a kind that
            // is not a bucket, so the pairing holds exactly when the two agree.
            let pairs = kind.is_an_s3_bucket() == (subject.origin_type == "s3");
            let answered = create_distribution(&router, &config).await;
            if pairs {
                assert_eq!(
                    answered.status, 201,
                    "case {case}: expected HTTP 201 for a control on an origin of its own kind, \
                     got {} with {}",
                    answered.status, answered.raw
                );
                // The accepted pairing is STORED: the distribution reads back with that control
                // on that origin.
                let id = answered.root().text_at("Id").unwrap_or_default().to_string();
                let fetched = get_distribution(&router, &id).await;
                let stored = fetched
                    .root()
                    .at("DistributionConfig/Origins/Items")
                    .and_then(|items| {
                        items
                            .children_named("Origin")
                            .first()
                            .and_then(|origin| origin.text_at("OriginAccessControlId"))
                    })
                    .unwrap_or_default()
                    .to_string();
                assert_eq!(
                    stored, control_id,
                    "case {case}: expected the stored origin to carry the control, got {}",
                    fetched.raw
                );
                continue;
            }

            let named = [control_id.as_str(), subject.origin_type, subject.domain_name];
            assert_refused(&answered, &case, "CreateDistribution", &named);
            // The tagging create runs the same create path, so it answers the same refusal.
            let with_tags = create_distribution_with_tags(&router, &config).await;
            assert_refused(&with_tags, &case, "CreateDistributionWithTags", &named);
            // And nothing was stored by either.
            let listed = list_distributions(&router, "").await;
            assert_eq!(
                listed.root().text_at("Quantity"),
                Some("0"),
                "case {case}: expected the refusals to create nothing, got {}",
                listed.raw
            );
        }
    }
}

/// RULE: an update is held to the same reading — an update that would leave an `s3` control on
/// an origin that specifies itself as a custom one is refused and changes nothing, and the
/// update that specifies the origin as the bucket it is lands.
///
/// The ONE pairing the recorded divergence is about, driven at the address that makes the other
/// half of the rule agree: a bucket's own REST endpoint.
#[tokio::test]
async fn an_update_cannot_move_an_origin_onto_a_kind_its_control_is_not_for() {
    let router = emulator();
    let case = "an update over an Amazon S3 bucket origin";
    let control_id = control(&router, case, "kind-update-control", "s3").await;
    let bucket = ORIGIN_TYPES[0].domain_name;
    let reference = "kind-update";
    let comment = "the configuration of an updated bucket origin";

    let stored = config_xml(
        reference,
        comment,
        bucket,
        Specified::S3Bucket,
        Some(&control_id),
    );
    let created = create_distribution(&router, &stored).await;
    assert_eq!(
        created.status, 201,
        "case {case}: expected HTTP 201 from CreateDistribution, got {} with {}",
        created.status, created.raw
    );
    let id = created.root().text_at("Id").unwrap_or_default().to_string();
    let etag = created.header("etag").to_string();

    let refused = update_distribution(
        &router,
        &id,
        &config_xml(
            reference,
            comment,
            bucket,
            Specified::CustomOrigin,
            Some(&control_id),
        ),
        Some(&etag),
    )
    .await;
    assert_refused(
        &refused,
        case,
        "UpdateDistribution",
        &[control_id.as_str(), bucket],
    );

    // The refused update changed nothing: the stored configuration still specifies the origin
    // as the bucket it is, at the version the create minted.
    let fetched = get_distribution(&router, &id).await;
    assert_eq!(
        fetched.header("etag"),
        etag,
        "case {case}: expected the refused update to leave the version at {etag}, got {:?}",
        fetched.header("etag")
    );
    assert!(
        fetched.raw.contains("S3OriginConfig"),
        "case {case}: expected the stored origin to keep its own kind, got {}",
        fetched.raw
    );

    // The same update, with the origin specified as what it is, lands.
    let updated = update_distribution(&router, &id, &stored, Some(&etag)).await;
    assert_eq!(
        updated.status, 200,
        "case {case}: expected HTTP 200 from an update that keeps the pairing, got {} with {}",
        updated.status, updated.raw
    );
}
