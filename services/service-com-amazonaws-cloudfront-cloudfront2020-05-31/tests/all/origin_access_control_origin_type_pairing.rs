// SPDX-License-Identifier: CC0-1.0

//! The PAIRING between an origin access control and the origin it is attached to, end to end
//! over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing, restXml
//! (de)serialization, the handler, the persisted state and serialization — the same path a
//! client takes. The rule under test is the one CloudFront states for an
//! `OriginAccessControlConfig$OriginAccessControlOriginType`, which is "the type of origin that
//! this origin access control is for":
//!
//! * every value of it names ONE service
//!   (`smithy_query(query="shape",shape="OriginAccessControlOriginTypes")`: `s3`,
//!   `mediastore`, `mediapackagev2`, `lambda`), and the control signs the origin
//!   request CloudFront makes to THAT service — so every call that stores a
//!   `DistributionConfig` admits a control on an origin only while the origin is of the kind
//!   that service's is: specified with the configuration member that names that kind
//!   (`S3OriginConfig` for the one `s3` control, `CustomOriginConfig` for every other type's
//!   origin) AND addressed at that service's endpoint. Every other pairing is refused with
//!   `IllegalOriginAccessConfiguration` (HTTP 400), storing nothing;
//! * the refusal is read on EVERY origin of the configuration, wherever the mismatched one
//!   sits;
//! * an origin that names no control at all is fronted by nothing, so it is accepted whatever
//!   it addresses;
//! * an accepted pairing is STORED: the distribution reads back with that control on that
//!   origin.
//!
//! The rows differ along everything the rule generalises over: every `OriginAccessControlOriginType`
//! the model admits, crossed with every ADDRESS form this emulator has to separate — the Amazon S3
//! endpoint spellings (the two REST forms, the website, dual-stack and FIPS forms and the S3
//! Express endpoint of a directory bucket), an Amazon API Gateway endpoint, both spellings a
//! Lambda function URL is published at (the host form AWS documents and the local addressing this
//! emulator's own Lambda announces), an Elemental MediaStore container, an Elemental MediaPackage
//! v2 endpoint and a customer domain that is nobody's service endpoint — and the call that submits
//! the configuration (`CreateDistribution`, `CreateDistributionWithTags`,
//! `UpdateDistribution`).
//!
//! The harness (the router and the request helpers) is the distribution and origin access
//! control suites', reused rather than copied.

use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_cloudfront_cloudfront2020_05_31::ops::types::ORIGIN_ACCESS_CONTROL_ORIGIN_TYPES_VALUES;
use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::ILLEGAL_ORIGIN_ACCESS_CONTROL_ORIGIN_TYPE_MESSAGE;

use crate::distribution_lifecycle::{
    call, create_distribution, emulator, get_distribution, list_distributions, update_distribution,
    Answer, API_VERSION,
};
use crate::origin_access_control_lifecycle::create_control;

/// The AWS-managed cache policy `Managed-CachingOptimized`, which the default cache behaviour
/// of every configuration below names.
const CACHING_OPTIMIZED: &str = "658327ea-f89d-4fab-a63d-7e88639e58f6";

// ---------------------------------------------------------------------------
// The addresses an origin is reached at
// ---------------------------------------------------------------------------

/// One `Origin$DomainName` form, and the `OriginAccessControlOriginType` whose service it
/// addresses — `None` for an origin no control may front at all, either because the address is
/// nobody's service endpoint or because the configuration member the origin is specified with
/// names a kind no type's service answers for.
///
/// The forms come from the endpoints the services the origin types name publish: the Amazon S3
/// endpoint spellings of a bucket (Amazon S3 User Guide, *Amazon S3 endpoints*),
/// `<api-id>.execute-api.<region>.amazonaws.com`, the Lambda function URL host
/// `<url-id>.lambda-url.<region>.on.aws`, the Elemental MediaStore container endpoint and the
/// Elemental MediaPackage v2 egress endpoint. The LOCAL row is the address this emulator's own
/// Lambda announces a function URL at for a client that resolves no AWS host — the same plane,
/// reached where that client can reach it.
struct Address {
    case: &'static str,
    domain_name: &'static str,
    /// The origin configuration member this origin is specified with, as the model lays the
    /// kinds out: `S3OriginConfig` for a bucket that is not a website, `CustomOriginConfig`
    /// for every other origin.
    s3_origin_config: bool,
    /// The one origin type whose control may front this origin.
    fronted_by: Option<&'static str>,
}

/// Every address form the rule is asserted over.
fn addresses() -> Vec<Address> {
    vec![
        Address {
            case: "an Amazon S3 bucket at its REST endpoint",
            domain_name: "reports.s3.us-east-1.amazonaws.com",
            s3_origin_config: true,
            fronted_by: Some("s3"),
        },
        Address {
            case: "an Amazon S3 bucket at the global REST endpoint",
            domain_name: "reports.s3.amazonaws.com",
            s3_origin_config: true,
            fronted_by: Some("s3"),
        },
        // An Amazon S3 bucket's STATIC WEBSITE surface is specified as a CUSTOM origin ("if
        // the Amazon S3 bucket is configured with static website hosting, use this type"), so
        // the configuration states an origin that is not the bucket's REST API: what answers
        // there is the website endpoint, and no origin type's service does, so no control may
        // front it — the `s3` control included.
        Address {
            case: "an Amazon S3 bucket at its website endpoint",
            domain_name: "reports.s3-website-us-east-1.amazonaws.com",
            s3_origin_config: false,
            fronted_by: None,
        },
        // The other endpoint spellings Amazon S3 publishes a bucket at (Amazon S3 User Guide,
        // *Amazon S3 endpoints*: the dual-stack and FIPS forms, and the S3 Express One Zone
        // endpoint of a directory bucket) — each an Amazon S3 origin however the label is
        // spelled.
        Address {
            case: "an Amazon S3 bucket at its dual-stack endpoint",
            domain_name: "reports.s3.dualstack.us-east-1.amazonaws.com",
            s3_origin_config: true,
            fronted_by: Some("s3"),
        },
        Address {
            case: "an Amazon S3 bucket at a FIPS endpoint",
            domain_name: "reports.s3-fips.us-east-1.amazonaws.com",
            s3_origin_config: true,
            fronted_by: Some("s3"),
        },
        Address {
            case: "an Amazon S3 directory bucket at its S3 Express endpoint",
            domain_name: "reports--use1-az4--x-s3.s3express-use1-az4.us-east-1.amazonaws.com",
            s3_origin_config: true,
            fronted_by: Some("s3"),
        },
        // An Amazon API Gateway API is addressed at its own `execute-api` endpoint, which is
        // the endpoint of a service no `OriginAccessControlOriginType` names
        // (`smithy_query(query="shape",shape="OriginAccessControlOriginTypes")`: s3, mediastore,
        // mediapackagev2, lambda) — so no control may front one, exactly as none may front a
        // customer domain.
        Address {
            case: "an Amazon API Gateway API at its execute-api endpoint",
            domain_name: "a1b2c3d4e5.execute-api.us-east-1.amazonaws.com",
            s3_origin_config: false,
            fronted_by: None,
        },
        Address {
            case: "a Lambda function URL at the host AWS publishes",
            domain_name: "0123456789abcdef0123456789abcdef.lambda-url.us-east-1.on.aws",
            s3_origin_config: false,
            fronted_by: Some("lambda"),
        },
        Address {
            case: "a Lambda function URL at the address this emulator announces",
            domain_name: "127.0.0.1:9297/_aws/lambda-url/0123456789abcdef0123456789abcdef",
            s3_origin_config: false,
            fronted_by: Some("lambda"),
        },
        Address {
            case: "an Elemental MediaStore container endpoint",
            domain_name: "a1b2c3.data.mediastore.us-east-1.amazonaws.com",
            s3_origin_config: false,
            fronted_by: Some("mediastore"),
        },
        Address {
            case: "an Elemental MediaPackage v2 egress endpoint",
            domain_name: "a1b2c3.egress.d4e5f6.mediapackagev2.us-east-1.amazonaws.com",
            s3_origin_config: false,
            fronted_by: Some("mediapackagev2"),
        },
        Address {
            case: "a customer domain that is nobody's service endpoint",
            domain_name: "content.example.com",
            s3_origin_config: false,
            fronted_by: None,
        },
    ]
}

// ---------------------------------------------------------------------------
// Calling the surface under test
// ---------------------------------------------------------------------------

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

/// One origin access control of `origin_type`, created through the API; answers its `Id`.
async fn control(router: &MultiServiceRouter, case: &str, name: &str, origin_type: &str) -> String {
    let body = format!(
        "<OriginAccessControlConfig \
         xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
         <Name>{name}</Name><Description>the origin access control pairing suite</Description>\
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

// ---------------------------------------------------------------------------
// The configurations under test
// ---------------------------------------------------------------------------

/// One origin of a submitted configuration: where it is addressed, and the control it names.
struct OriginRow<'row> {
    id: String,
    address: &'row Address,
    control: Option<&'row str>,
}

/// The `DistributionConfig` XML a client sends for `origins`. Members follow the model's
/// declaration order for every shape they sit in.
fn config_xml(reference: &str, comment: &str, origins: &[OriginRow<'_>]) -> String {
    let mut out = format!(
        "<DistributionConfig xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
         <CallerReference>{reference}</CallerReference>\
         <Origins><Quantity>{}</Quantity><Items>",
        origins.len()
    );
    for origin in origins {
        out.push_str(&format!(
            "<Origin><Id>{}</Id><DomainName>{}</DomainName>",
            origin.id, origin.address.domain_name
        ));
        if origin.address.s3_origin_config {
            // The form the model documents for a bucket reached through an origin access
            // control: "specify an empty OriginAccessIdentity element".
            out.push_str(
                "<S3OriginConfig><OriginAccessIdentity></OriginAccessIdentity></S3OriginConfig>",
            );
        } else {
            out.push_str(
                "<CustomOriginConfig><HTTPPort>80</HTTPPort><HTTPSPort>443</HTTPSPort>\
                 <OriginProtocolPolicy>https-only</OriginProtocolPolicy></CustomOriginConfig>",
            );
        }
        if let Some(control) = origin.control {
            out.push_str(&format!(
                "<OriginAccessControlId>{control}</OriginAccessControlId>"
            ));
        }
        out.push_str("</Origin>");
    }
    out.push_str(&format!(
        "</Items></Origins>\
         <DefaultCacheBehavior><TargetOriginId>{}</TargetOriginId>\
         <ViewerProtocolPolicy>https-only</ViewerProtocolPolicy>\
         <CachePolicyId>{CACHING_OPTIMIZED}</CachePolicyId></DefaultCacheBehavior>\
         <Comment>{comment}</Comment>\
         <PriceClass>PriceClass_All</PriceClass>\
         <Enabled>false</Enabled></DistributionConfig>",
        origins
            .first()
            .map(|origin| origin.id.clone())
            .unwrap_or_default()
    ));
    out
}

/// A one-origin configuration at `address`, naming `control`.
fn one_origin<'row>(
    reference: &str,
    comment: &str,
    address: &'row Address,
    control: Option<&'row str>,
) -> String {
    config_xml(
        reference,
        comment,
        &[OriginRow {
            id: "primary".to_string(),
            address,
            control,
        }],
    )
}

// ---------------------------------------------------------------------------
// The assertions each outcome is held to
// ---------------------------------------------------------------------------

/// Assert that `answer` is the refusal the rule states: HTTP 400 with
/// `IllegalOriginAccessConfiguration` and the sentence this code is served with, naming nothing
/// out of the request.
fn assert_refused(answer: &Answer, case: &str, operation: &str, named: &[&str]) {
    assert!(
        !named.is_empty(),
        "case {case} ({operation}): every refusal names what may not appear in its message"
    );
    assert_eq!(
        answer.status, 400,
        "case {case} ({operation}): expected HTTP 400 for a control fronting an origin of \
         another kind, got {} with {}",
        answer.status, answer.raw
    );
    assert_eq!(
        answer.error_code(),
        "IllegalOriginAccessConfiguration",
        "case {case} ({operation}): expected IllegalOriginAccessConfiguration, got {:?} with {}",
        answer.error_code(),
        answer.raw
    );
    let message = answer.root().find_text("Message").unwrap_or_default();
    assert_eq!(
        message, ILLEGAL_ORIGIN_ACCESS_CONTROL_ORIGIN_TYPE_MESSAGE,
        "case {case} ({operation}): expected the sentence this code is served with, got \
         {message:?}"
    );
    for value in named {
        assert!(
            !message.contains(value),
            "case {case} ({operation}): expected nothing from the request in the message, got \
             {message:?}"
        );
    }
}

/// The `OriginAccessControlId` the stored configuration reports for each origin, in order.
fn stored_controls(answer: &Answer, case: &str) -> Vec<String> {
    answer
        .root()
        .at("DistributionConfig/Origins/Items")
        .unwrap_or_else(|| panic!("case {case}: no origin list in {}", answer.raw))
        .children_named("Origin")
        .iter()
        .map(|origin| {
            origin
                .text_at("OriginAccessControlId")
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// Assert the account holds no distribution at all, and that nothing carries `comment`.
async fn assert_nothing_stored(router: &MultiServiceRouter, case: &str, comment: &str) {
    let listed = list_distributions(router, "").await;
    assert_eq!(
        listed.root().text_at("Quantity"),
        Some("0"),
        "case {case}: expected the refusal to create nothing, got {}",
        listed.raw
    );
    assert!(
        !listed.raw.contains(comment),
        "case {case}: expected the refused configuration to be in no listing, got {}",
        listed.raw
    );
}

// ---------------------------------------------------------------------------
// A create pairs the control's origin type with the origin
// ---------------------------------------------------------------------------

/// `CreateDistribution` admits a control on an origin exactly when the origin is addressed at
/// the service the control's `OriginAccessControlOriginType` names, and refuses every other
/// pairing with `IllegalOriginAccessConfiguration` — over the whole cross product of the
/// model's origin types and the address forms an origin takes.
#[tokio::test]
async fn a_create_admits_a_control_only_on_an_origin_of_the_kind_its_type_names() {
    for origin_type in ORIGIN_ACCESS_CONTROL_ORIGIN_TYPES_VALUES {
        for address in addresses() {
            let case = format!("a {origin_type} control on {}", address.case);
            let router = emulator();
            let control_id = control(&router, &case, "pairing-control", origin_type).await;
            let reference = format!("pairing-{origin_type}");
            let comment = format!("the configuration of {case}");
            let config = one_origin(&reference, &comment, &address, Some(&control_id));

            let answered = create_distribution(&router, &config).await;
            if address.fronted_by == Some(*origin_type) {
                assert_eq!(
                    answered.status, 201,
                    "case {case}: expected HTTP 201 for a control on an origin of its own \
                     kind, got {} with {}",
                    answered.status, answered.raw
                );
                // The accepted pairing is STORED: the distribution reads back with that
                // control on that origin.
                let id = answered.root().text_at("Id").unwrap_or_default().to_string();
                let fetched = get_distribution(&router, &id).await;
                assert_eq!(
                    fetched.status, 200,
                    "case {case}: expected HTTP 200 from GetDistribution, got {} with {}",
                    fetched.status, fetched.raw
                );
                assert_eq!(
                    stored_controls(&fetched, &case),
                    vec![control_id.clone()],
                    "case {case}: expected the stored origin to carry the control, got {}",
                    fetched.raw
                );
                continue;
            }

            let named = [control_id.as_str(), *origin_type, address.domain_name];
            assert_refused(&answered, &case, "CreateDistribution", &named);
            // The tagging create runs the same create path, so it answers the same refusal.
            let with_tags = create_distribution_with_tags(&router, &config).await;
            assert_refused(&with_tags, &case, "CreateDistributionWithTags", &named);
            assert_nothing_stored(&router, &case, &comment).await;

            // The positive control: the same origin with no control named at all is accepted,
            // so what was refused was the pairing and nothing else about the configuration.
            let unfronted = one_origin(&reference, &comment, &address, None);
            let created = create_distribution(&router, &unfronted).await;
            assert_eq!(
                created.status, 201,
                "case {case}: expected the same origin naming no control to be accepted, got \
                 {} with {}",
                created.status, created.raw
            );
        }
    }
}

// ---------------------------------------------------------------------------
// The pairing is read on every origin
// ---------------------------------------------------------------------------

/// The rule holds for whichever origin of the list carries the mismatched control: a
/// configuration whose FIRST, middle or LAST origin pairs a control with an origin of another
/// kind is refused, and the same list with every pairing right is accepted.
#[tokio::test]
async fn the_pairing_is_read_on_every_origin_of_the_configuration() {
    let forms = addresses();
    let s3 = forms
        .iter()
        .find(|address| address.fronted_by == Some("s3"))
        .expect("the address table carries an Amazon S3 origin");
    let other = forms
        .iter()
        .find(|address| address.fronted_by == Some("lambda"))
        .expect("the address table carries a Lambda function URL origin");

    for mismatch_at in 0..3 {
        let case = format!("the control on origin {mismatch_at} of three");
        let router = emulator();
        let s3_control = control(&router, &case, "pairing-s3", "s3").await;
        let lambda_control = control(&router, &case, "pairing-lambda", "lambda").await;

        // Three origins, each addressed at a different service, each fronted by the control
        // its own kind admits — except the one this row moves the mismatch to, which names
        // the control of the other kind.
        let origins: Vec<OriginRow<'_>> = (0..3)
            .map(|index| {
                let s3_origin = index % 2 == 0;
                let address = if s3_origin { s3 } else { other };
                let matched = if s3_origin {
                    s3_control.as_str()
                } else {
                    lambda_control.as_str()
                };
                let mismatched = if s3_origin {
                    lambda_control.as_str()
                } else {
                    s3_control.as_str()
                };
                OriginRow {
                    id: format!("origin-{index}"),
                    address,
                    control: Some(if index == mismatch_at { mismatched } else { matched }),
                }
            })
            .collect();

        let reference = format!("pairing-list-{mismatch_at}");
        let comment = format!("the configuration of {case}");
        let refused =
            create_distribution(&router, &config_xml(&reference, &comment, &origins)).await;
        assert_refused(
            &refused,
            &case,
            "CreateDistribution",
            &[s3_control.as_str(), lambda_control.as_str()],
        );
        assert_nothing_stored(&router, &case, &comment).await;

        // The same list with the mismatch put right is accepted, and stores each control on
        // the origin it was named on.
        let paired: Vec<OriginRow<'_>> = origins
            .iter()
            .map(|origin| {
                let matched = if origin.address.fronted_by == Some("s3") {
                    s3_control.as_str()
                } else {
                    lambda_control.as_str()
                };
                OriginRow {
                    id: origin.id.clone(),
                    address: origin.address,
                    control: Some(matched),
                }
            })
            .collect();
        let created = create_distribution(&router, &config_xml(&reference, &comment, &paired))
            .await;
        assert_eq!(
            created.status, 201,
            "case {case}: expected the paired list to be accepted, got {} with {}",
            created.status, created.raw
        );
        let id = created.root().text_at("Id").unwrap_or_default().to_string();
        let fetched = get_distribution(&router, &id).await;
        assert_eq!(
            stored_controls(&fetched, &case),
            vec![
                s3_control.clone(),
                lambda_control.clone(),
                s3_control.clone()
            ],
            "case {case}: expected each control stored on the origin it was named on, got {}",
            fetched.raw
        );
    }
}

// ---------------------------------------------------------------------------
// An update is held to the same pairing
// ---------------------------------------------------------------------------

/// `UpdateDistribution` applies the same rule: an update that would leave a control fronting an
/// origin of another kind is refused, the stored configuration is untouched at its own version,
/// and an update that keeps the pairing right lands.
#[tokio::test]
async fn an_update_is_refused_when_it_would_leave_a_control_on_another_kind_of_origin() {
    let forms = addresses();
    for address in forms.iter().filter(|address| address.fronted_by.is_some()) {
        let fronted_by = address.fronted_by.expect("the row names an origin type");
        let case = format!("an update over {}", address.case);
        let router = emulator();
        let matched = control(&router, &case, "pairing-matched", fronted_by).await;
        // A control of a type this origin is NOT: the first origin type the catalog lists
        // that is not the one fronting this address.
        let other_type = ORIGIN_ACCESS_CONTROL_ORIGIN_TYPES_VALUES
            .iter()
            .find(|origin_type| **origin_type != fronted_by)
            .expect("the catalog lists more than one origin type");
        let mismatched = control(&router, &case, "pairing-mismatched", other_type).await;

        let reference = format!("pairing-update-{fronted_by}");
        let comment = format!("the configuration of {case}");
        let stored = one_origin(&reference, &comment, address, Some(&matched));
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
            &one_origin(&reference, &comment, address, Some(&mismatched)),
            Some(&etag),
        )
        .await;
        assert_refused(
            &refused,
            &case,
            "UpdateDistribution",
            &[mismatched.as_str(), *other_type],
        );

        // The refused update changed nothing: the stored configuration still names the
        // control it was created with, at the version the create minted.
        let fetched = get_distribution(&router, &id).await;
        assert_eq!(
            fetched.header("etag"),
            etag,
            "case {case}: expected the refused update to leave the version at {etag}, got {:?}",
            fetched.header("etag")
        );
        assert_eq!(
            stored_controls(&fetched, &case),
            vec![matched.clone()],
            "case {case}: expected the stored origin to keep its own control, got {}",
            fetched.raw
        );

        // The same update with the pairing kept right lands.
        let updated = update_distribution(&router, &id, &stored, Some(&etag)).await;
        assert_eq!(
            updated.status, 200,
            "case {case}: expected HTTP 200 from an update that keeps the pairing, got {} \
             with {}",
            updated.status, updated.raw
        );
        assert_eq!(
            stored_controls(&updated, &case),
            vec![matched.clone()],
            "case {case}: expected the update to report the control it stored, got {}",
            updated.raw
        );
    }
}
