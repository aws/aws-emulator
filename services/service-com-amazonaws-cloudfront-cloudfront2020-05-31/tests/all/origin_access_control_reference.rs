// SPDX-License-Identifier: CC0-1.0

//! The origin access control an origin NAMES, resolved against the account's controls,
//! end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing, restXml
//! (de)serialization, the handler, the persisted state and serialization — the same path a
//! client takes. The rule under test is the one CloudFront states for a reference a
//! distribution configuration makes to an origin access control:
//!
//! * every call that STORES a submitted `DistributionConfig` resolves each
//!   `Origin$OriginAccessControlId` against the account's origin access controls, and a
//!   configuration naming one the account does not hold — never created, or created and
//!   since deleted — is refused with `InvalidOriginAccessControl` (HTTP 400) and stores
//!   nothing: no distribution is created, and the caller reference the refused request
//!   named is not spent either;
//! * an absent or empty `OriginAccessControlId` names no control at all, so it resolves to
//!   nothing and is accepted;
//! * the identical configuration naming a control the account DOES hold is accepted and
//!   reads back with that control on the same origins.
//!
//! The rows differ along everything the rule generalises over: HOW the named control fails
//! to resolve (a control that was deleted, an id in the shape this service mints that no
//! control was created under, a value in no such shape), WHERE in the origin list the
//! reference sits and how many origins carry it, the KIND of origin that carries it (an
//! Amazon S3 origin, which is what an origin access control is for, and a custom origin at
//! a Lambda function URL endpoint), the configuration the live control itself was created
//! with (every row of the origin access control table whose origin type is that origin's
//! kind, which is the pairing a stored configuration has to satisfy), and WHICH call submits
//! the configuration (`CreateDistribution`, `CreateDistributionWithTags` and
//! `UpdateDistribution`).
//!
//! The harness (the router, the request helpers and the control table) is the one the
//! distribution and origin access control suites use, reused rather than copied.

use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::INVALID_ORIGIN_ACCESS_CONTROL_MESSAGE;

use crate::distribution_lifecycle::{
    call, create_distribution, emulator, get_distribution, is_version_token, list_distributions,
    update_distribution, Answer, API_VERSION,
};
use crate::origin_access_control_lifecycle::{create_control, get_control, table, Control};

/// The AWS-managed cache policy `Managed-CachingOptimized`, which the default cache
/// behaviour of every configuration below names — the policy a client attaches to a
/// behaviour that caches what it may. Its id is the one `MANAGED_CACHE_POLICIES`
/// publishes.
const CACHING_OPTIMIZED: &str = "658327ea-f89d-4fab-a63d-7e88639e58f6";

// ---------------------------------------------------------------------------
// Calling the surface under test
// ---------------------------------------------------------------------------

/// `DELETE /2020-05-31/origin-access-control/{Id}` under `if_match`.
async fn delete_control(router: &MultiServiceRouter, id: &str, if_match: &str) -> Answer {
    call(
        router,
        "DELETE",
        &format!("/{API_VERSION}/origin-access-control/{id}"),
        None,
        Some(if_match),
    )
    .await
}

/// `POST /2020-05-31/distribution?WithTags`, whose payload wraps a `DistributionConfig`
/// in a `DistributionConfigWithTags`.
///
/// The body is built here rather than through the tagging suite's helper because that one
/// takes the distribution harness's configuration builder, which carries no
/// `Origin$OriginAccessControlId` — the member under test.
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

/// Create one origin access control through the API and report `(Id, ETag)`.
async fn create(router: &MultiServiceRouter, row: &Control) -> (String, String) {
    let created = create_control(router, &row.xml()).await;
    assert_eq!(
        created.status,
        201,
        "case {}: expected HTTP 201 from CreateOriginAccessControl, got {} with {}",
        row.case,
        created.status,
        created.raw
    );
    let id = created.root().text_at("Id").unwrap_or_default().to_string();
    let etag = created.header("etag").to_string();
    assert!(
        is_version_token(&etag),
        "case {}: expected a minted version token in ETag, got {etag:?}",
        row.case
    );
    (id, etag)
}

// ---------------------------------------------------------------------------
// The configurations under test
// ---------------------------------------------------------------------------

/// What kind of origin carries the reference under test.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OriginKind {
    /// An `S3OriginConfig` origin with an empty `OriginAccessIdentity` — the form the
    /// model documents for a bucket reached through an origin access control ("If you're
    /// using origin access control (OAC) instead of origin access identity, specify an
    /// empty `OriginAccessIdentity` element").
    S3,
    /// A `CustomOriginConfig` origin, which carries no origin access identity at all — here
    /// a Lambda function URL, addressed at the host Lambda publishes one at.
    Custom,
}

impl OriginKind {
    /// The `OriginAccessControlOriginType` a control fronting an origin of this kind carries
    /// — the type that names the service this origin belongs to
    /// (`smithy_query(query="shape",shape="OriginAccessControlOriginTypes")`).
    fn origin_type(self) -> &'static str {
        match self {
            OriginKind::S3 => "s3",
            OriginKind::Custom => "lambda",
        }
    }
}

/// The rows of the origin access control table whose origin type fronts an origin of `kind`,
/// so the configuration each case stores pairs the control with the origin it is for.
fn controls_for(kind: OriginKind) -> Vec<Control> {
    let rows: Vec<Control> = table()
        .into_iter()
        .filter(|row| row.origin_type() == kind.origin_type())
        .collect();
    assert!(
        !rows.is_empty(),
        "the control table must carry a row of origin type {:?}",
        kind.origin_type()
    );
    rows
}

/// How the origin access control id a configuration names fails to resolve.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DeadReference {
    /// A control this account created and has since DELETED: the id is one this service
    /// minted, and the store no longer holds it.
    Deleted,
    /// An id in the shape this service mints (`E` + 13 uppercase alphanumerics) that no
    /// control was ever created under.
    NeverCreated,
    /// A value in no shape this service mints a control id in.
    NotAnId,
}

impl DeadReference {
    fn label(self) -> &'static str {
        match self {
            DeadReference::Deleted => "a deleted control",
            DeadReference::NeverCreated => "an unused id in the minted shape",
            DeadReference::NotAnId => "a value in no minted shape",
        }
    }
}

/// One case: an origin list, with the reference under test on the origins the row marks.
#[derive(Debug, Clone)]
struct Case {
    case: String,
    /// `true` for each origin that names the control under test, in origin order.
    attached: Vec<bool>,
    kind: OriginKind,
    dead: DeadReference,
}

/// Where the reference sits in the origin list, which the rule has to hold for wherever
/// it is: on the only origin, on the first or the last of several, and on more than one.
fn attachments() -> Vec<(&'static str, Vec<bool>)> {
    vec![
        ("the only origin", vec![true]),
        ("the first of three origins", vec![true, false, false]),
        ("the last of three origins", vec![false, false, true]),
        ("two of three origins", vec![true, false, true]),
    ]
}

/// Every case: each way of failing to resolve, on each attachment, with the origin kind
/// alternating across the product so both kinds carry each failure.
fn cases() -> Vec<Case> {
    let mut rows = Vec::new();
    for dead in [
        DeadReference::Deleted,
        DeadReference::NeverCreated,
        DeadReference::NotAnId,
    ] {
        for (index, (where_, attached)) in attachments().into_iter().enumerate() {
            let kind = if index % 2 == 0 {
                OriginKind::S3
            } else {
                OriginKind::Custom
            };
            rows.push(Case {
                case: format!("{} on {where_} ({kind:?} origin)", dead.label()),
                attached,
                kind,
                dead,
            });
        }
    }
    rows
}

/// The `DistributionConfig` XML a client sends for `origins` — each `(Id, control)`, with
/// `None` for an origin that names no control at all. Members follow the model's
/// declaration order.
fn config_xml(
    reference: &str,
    comment: &str,
    origins: &[(String, Option<String>)],
    kind: OriginKind,
) -> String {
    let mut out = format!(
        "<DistributionConfig xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
         <CallerReference>{reference}</CallerReference>\
         <Origins><Quantity>{}</Quantity><Items>",
        origins.len()
    );
    for (origin_id, control) in origins {
        out.push_str(&format!("<Origin><Id>{origin_id}</Id>"));
        match kind {
            OriginKind::S3 => out.push_str(&format!(
                "<DomainName>{origin_id}-bucket.s3.us-east-1.amazonaws.com</DomainName>\
                 <S3OriginConfig><OriginAccessIdentity></OriginAccessIdentity>\
                 </S3OriginConfig>"
            )),
            OriginKind::Custom => out.push_str(&format!(
                "<DomainName>{origin_id}.lambda-url.us-east-1.on.aws</DomainName>\
                 <CustomOriginConfig><HTTPPort>80</HTTPPort><HTTPSPort>443</HTTPSPort>\
                 <OriginProtocolPolicy>https-only</OriginProtocolPolicy></CustomOriginConfig>"
            )),
        }
        if let Some(control) = control {
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
            .map(|(id, _)| id.clone())
            .unwrap_or_default()
    ));
    out
}

/// The origin ids of a case, one per entry of its attachment pattern.
fn origin_ids(attached: &[bool]) -> Vec<String> {
    (0..attached.len())
        .map(|index| format!("origin-{index}"))
        .collect()
}

/// The origin list of a case, with `control` on each origin the row marks.
fn origins_of(ids: &[String], attached: &[bool], control: &str) -> Vec<(String, Option<String>)> {
    ids.iter()
        .zip(attached)
        .map(|(id, holds)| {
            (
                id.clone(),
                if *holds {
                    Some(control.to_string())
                } else {
                    None
                },
            )
        })
        .collect()
}

/// Assert that `answer` is the refusal the rule states: HTTP 400 with
/// `InvalidOriginAccessControl` and the sentence this code is served with, naming nothing
/// out of the request.
fn assert_refused(answer: &Answer, case: &str, operation: &str, dead: &str) {
    assert_eq!(
        answer.status, 400,
        "case {case} ({operation}): expected HTTP 400 for an origin naming a control the \
         account does not hold, got {} with {}",
        answer.status, answer.raw
    );
    assert_eq!(
        answer.error_code(),
        "InvalidOriginAccessControl",
        "case {case} ({operation}): expected InvalidOriginAccessControl, got {:?} with {}",
        answer.error_code(),
        answer.raw
    );
    let message = answer.root().find_text("Message").unwrap_or_default();
    assert_eq!(
        message, INVALID_ORIGIN_ACCESS_CONTROL_MESSAGE,
        "case {case} ({operation}): expected the InvalidOriginAccessControl message, got \
         {message:?}"
    );
    assert!(
        !message.contains(dead),
        "case {case} ({operation}): expected nothing from the request in the message, got \
         {message:?}"
    );
}

/// The id of a control that does not resolve, for a row of the table — created and
/// deleted where the row asks for that, and confirmed gone through
/// `GetOriginAccessControl` either way, exactly as a client would establish it.
async fn dead_id(router: &MultiServiceRouter, case: &Case, row: &Control) -> String {
    let id = match case.dead {
        DeadReference::Deleted => {
            let (id, etag) = create(router, row).await;
            let deleted = delete_control(router, &id, &etag).await;
            assert_eq!(
                deleted.status, 204,
                "case {}: expected HTTP 204 from DeleteOriginAccessControl, got {} with {}",
                case.case, deleted.status, deleted.raw
            );
            id
        }
        // In the shape this service mints a control id in (`E` + 13 uppercase
        // alphanumerics), which the read below confirms names no control.
        DeadReference::NeverCreated => "E2QWRUHAPOMQFV".to_string(),
        DeadReference::NotAnId => "not-an-origin-access-control".to_string(),
    };
    let fetched = get_control(router, &id).await;
    assert_eq!(
        fetched.status, 404,
        "case {}: expected GetOriginAccessControl on {id} to answer 404 before the \
         distribution call, got {} with {}",
        case.case, fetched.status, fetched.raw
    );
    id
}

// ---------------------------------------------------------------------------
// A create naming a control the account does not hold
// ---------------------------------------------------------------------------

/// A configuration whose origin names an origin access control the account does not hold
/// is refused with `InvalidOriginAccessControl` and creates nothing — while the identical
/// configuration naming a control the account DOES hold is accepted and reads back with
/// that control on the same origins.
#[tokio::test]
async fn a_create_naming_a_control_the_account_does_not_hold_is_refused() {
    for (index, case) in cases().into_iter().enumerate() {
        let rows = controls_for(case.kind);
        let router = emulator();
        // The live control this account holds throughout, and the configuration it was
        // created with varies by row, so the rule is not tested against one control.
        let (live, _) = create(&router, &rows[index % rows.len()]).await;
        let dead = dead_id(&router, &case, &rows[(index + 1) % rows.len()]).await;

        let ids = origin_ids(&case.attached);
        let reference = format!("ref-{index}");
        let comment = format!("a configuration refused for {}", case.case);
        let refused_config = config_xml(
            &reference,
            &comment,
            &origins_of(&ids, &case.attached, &dead),
            case.kind,
        );

        let refused = create_distribution(&router, &refused_config).await;
        assert_refused(&refused, &case.case, "CreateDistribution", &dead);

        // The tagging create runs the same create path, so it answers the same refusal.
        let with_tags = create_distribution_with_tags(&router, &refused_config).await;
        assert_refused(&with_tags, &case.case, "CreateDistributionWithTags", &dead);

        // And the refusal created nothing: the account holds no distribution at all, so
        // the refused configuration's comment is in no listing either.
        let listed = list_distributions(&router, "").await;
        assert_eq!(
            listed.root().text_at("Quantity"),
            Some("0"),
            "case {}: expected the refused creates to leave the account with no \
             distributions, got {}",
            case.case,
            listed.raw
        );
        assert!(
            !listed.raw.contains(&comment),
            "case {}: expected the refused configuration's comment to be in no listing, \
             got {}",
            case.case,
            listed.raw
        );

        // The positive control: the SAME configuration, under the SAME caller reference,
        // with the live control in place of the one that does not resolve. It is accepted
        // — so what was refused was the reference, and the refusals above spent neither
        // the caller reference nor anything else.
        let accepted_config = config_xml(
            &reference,
            &comment,
            &origins_of(&ids, &case.attached, &live),
            case.kind,
        );
        let created = create_distribution(&router, &accepted_config).await;
        assert_eq!(
            created.status, 201,
            "case {}: expected HTTP 201 for the same configuration naming a live control, \
             got {} with {}",
            case.case, created.status, created.raw
        );
        let distribution_id = created.root().text_at("Id").unwrap_or_default().to_string();

        // Read back through the implemented reader: the stored origins carry the control
        // the accepted configuration named, and only on the origins that named it.
        let fetched = get_distribution(&router, &distribution_id).await;
        assert_eq!(
            fetched.status, 200,
            "case {}: expected HTTP 200 from GetDistribution, got {} with {}",
            case.case, fetched.status, fetched.raw
        );
        assert_controls(&fetched, &case, &live, "GetDistribution");
    }
}

/// Assert that the `Distribution` in `answer` reports the control `live` on exactly the
/// origins `case` attaches it to, and no control on the others.
fn assert_controls(answer: &Answer, case: &Case, live: &str, operation: &str) {
    let origins = answer
        .root()
        .at("DistributionConfig/Origins/Items")
        .unwrap_or_else(|| {
            panic!(
                "case {} ({operation}): no origin list in {}",
                case.case, answer.raw
            )
        })
        .children_named("Origin");
    assert_eq!(
        origins.len(),
        case.attached.len(),
        "case {} ({operation}): expected {} origins, got {}",
        case.case,
        case.attached.len(),
        answer.raw
    );
    for (origin, holds) in origins.iter().zip(&case.attached) {
        let actual = origin.text_at("OriginAccessControlId").unwrap_or_default();
        // An origin that named no control carries the empty id the service resolves for
        // one that names none.
        let expected = if *holds { live } else { "" };
        assert_eq!(
            actual, expected,
            "case {} ({operation}): expected OriginAccessControlId {expected:?} on origin \
             {:?}, got {actual:?} in {}",
            case.case,
            origin.text_at("Id").unwrap_or_default(),
            answer.raw
        );
    }
}

// ---------------------------------------------------------------------------
// An update naming a control the account does not hold
// ---------------------------------------------------------------------------

/// `UpdateDistribution` resolves the reference the same way: an update that would leave an
/// origin naming a control the account does not hold is refused, the stored configuration
/// is untouched at its own version, and the same update naming the live control lands.
#[tokio::test]
async fn an_update_naming_a_control_the_account_does_not_hold_is_refused() {
    for (index, case) in cases().into_iter().enumerate() {
        let rows = controls_for(case.kind);
        let router = emulator();
        let (live, _) = create(&router, &rows[index % rows.len()]).await;

        let ids = origin_ids(&case.attached);
        let reference = format!("ref-{index}");
        let comment = format!("a configuration updated for {}", case.case);
        let stored_config = config_xml(
            &reference,
            &comment,
            &origins_of(&ids, &case.attached, &live),
            case.kind,
        );
        let created = create_distribution(&router, &stored_config).await;
        assert_eq!(
            created.status, 201,
            "case {}: expected HTTP 201 from CreateDistribution, got {} with {}",
            case.case, created.status, created.raw
        );
        let distribution_id = created.root().text_at("Id").unwrap_or_default().to_string();
        let etag = created.header("etag").to_string();

        // The reference the update would leave behind, established exactly as a client
        // establishes it — including a control deleted while no origin named it.
        let dead = dead_id(&router, &case, &rows[(index + 1) % rows.len()]).await;
        let refused_config = config_xml(
            &reference,
            &comment,
            &origins_of(&ids, &case.attached, &dead),
            case.kind,
        );
        let refused =
            update_distribution(&router, &distribution_id, &refused_config, Some(&etag)).await;
        assert_refused(&refused, &case.case, "UpdateDistribution", &dead);

        // The refused update changed nothing: the stored configuration still names the
        // live control, at the version the create minted.
        let fetched = get_distribution(&router, &distribution_id).await;
        assert_eq!(
            fetched.header("etag"),
            etag,
            "case {}: expected the refused update to leave the version at {etag}, got {:?}",
            case.case,
            fetched.header("etag")
        );
        assert_controls(&fetched, &case, &live, "GetDistribution after a refused update");

        // The same update naming the live control lands, so what was refused was the
        // reference rather than the update.
        let updated =
            update_distribution(&router, &distribution_id, &stored_config, Some(&etag)).await;
        assert_eq!(
            updated.status, 200,
            "case {}: expected HTTP 200 from UpdateDistribution naming a live control, got \
             {} with {}",
            case.case, updated.status, updated.raw
        );
        assert_controls(&updated, &case, &live, "UpdateDistribution");
    }
}
