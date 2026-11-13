// SPDX-License-Identifier: CC0-1.0

//! The TEXT of four Route 53 refusals whose message is built to a RULE, end to
//! end over the router.
//!
//! Each rule is about how the refusal's `Message` is BUILT, so each is asserted
//! over a table spanning what the rule generalises over rather than the one
//! request that exposed it:
//!
//! * a `(hosted zone, VPC)` pair the zone's authorization ledger does not hold
//!   is `VPCAssociationAuthorizationNotFound` (HTTP 404) whose message is
//!   REQUEST-DERIVED and worded for that ledger — it names the zone, the VPC id
//!   AND the region the request asked about, and states that no authorization
//!   exists for them, rather than restating the association path's refusal;
//! * an association that would leave one VPC resolving one name through two
//!   zones is `ConflictingDomainExists` (HTTP 400) whose message names the VPC
//!   id and region from the request and the ID of the zone ALREADY associated
//!   with that VPC under the same name — the actionable half, which the caller
//!   cannot know without another read;
//! * a `NextToken` that identifies no page is `InvalidPaginationToken` (HTTP
//!   400) whose message names the token that arrived and nothing else, from
//!   EVERY paginated operation that refuses one, because what a token encodes
//!   is the service's own business;
//! * a traffic policy document the published format does not define is
//!   `InvalidTrafficPolicyDocument` (HTTP 400) whose message enumerates EVERY
//!   constraint the document violates, one clause per violated constraint,
//!   joined with `;` — so a caller validating a document by submitting it
//!   learns all of its faults from one request.
//!
//! The harness (`router`, `call`, `Reply`) and every request builder come from
//! the suites that own those operations, so these tests drive the service
//! through the same router a socket-served request goes through.
//!
//! The readers every refusal-wording suite shares live here too: [`message_of`], for a
//! refusal whose message is the thing under test, and [`assert_identity_only_at`], for one
//! the service answers by its error class alone.

use service_com_amazonaws_route53_awsdnsv20130401::state::TRAFFIC_POLICY_FORMAT_VERSION;

use crate::hosted_zone_records::{router, Reply};
use crate::hosted_zones_by_vpc::list_by_vpc;
use crate::traffic_policies::{create_policy, document_with_start_endpoint};
use crate::vpc_association_authorizations::{authorize_ok, list_authorizations};
use crate::vpc_associations::{associate, create_private_zone, vpc_block, CREATION_VPC, HOME_REGION};
use crate::vpc_authorization_deletion::{deauthorize, deauthorize_ok};

/// The `Error/Message` of a refusal, or a panic naming the case: a refusal whose
/// message is the thing under test has to carry one.
///
/// `pub(crate)` so a sibling suite asserting another refusal's wording reads it
/// out of the reply the same way rather than growing a second copy of this.
pub(crate) fn message_of(reply: &Reply, case: &str) -> String {
    reply
        .first("Error/Message")
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: the refusal must carry a message; body {}",
                reply.body
            )
        })
        .to_string()
}

/// Assert that `reply` reports a refusal by its IDENTITY ALONE: the given status, the
/// `Error` element carrying `Type` `Sender` and the expected `Code`, and no `Message`
/// element anywhere in the body.
///
/// Which refusals the service answers this way is a fact of the error CLASS, not of the
/// status it carries — so the status is a parameter and this one check serves every class
/// the service reports without a message.
///
/// `pub(crate)` so every suite that drives such a refusal asserts the body shape through
/// the SAME check rather than growing a second copy of it.
pub(crate) fn assert_identity_only_at(reply: &Reply, case: &str, status: u16, code: &str) {
    assert_eq!(
        reply.status, status,
        "case {case:?}: expected HTTP {status}, got {} with body {}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error_code(),
        code,
        "case {case:?}: expected the code {code:?}, got {:?}; body {}",
        reply.error_code(),
        reply.body
    );
    assert_eq!(
        reply.first("Error/Type"),
        Some("Sender"),
        "case {case:?}: expected the fault Type \"Sender\", got {:?}; body {}",
        reply.first("Error/Type"),
        reply.body
    );
    assert_eq!(
        reply.first("Error/Message"),
        None,
        "case {case:?}: the refusal must carry no Message element, got {:?}; body {}",
        reply.first("Error/Message"),
        reply.body
    );
    // Read off the raw body as well, so an EMPTY `<Message/>` — which carries no
    // text and so does not reach the flattened element paths — cannot pass
    // either.
    assert!(
        !reply.body.contains("<Message"),
        "case {case:?}: the body must carry no Message element at all, got {}",
        reply.body
    );
}

// ---------------------------------------------------------------------------
// The authorization ledger's not-found refusal
// ---------------------------------------------------------------------------

/// One request to remove an authorization the zone does not hold: the zone id as
/// the request spells it, the bare id the message must name, and the VPC the
/// request asks about.
struct LedgerRow {
    case: &'static str,
    /// The `{HostedZoneId}` label as the request writes it (URI-encoded where
    /// needed).
    requested_zone: String,
    /// The id the message must name — always the BARE one.
    bare_zone: String,
    vpc_id: &'static str,
    region: &'static str,
}

/// The form this service words a missing authorization in, with the request's
/// own values filled in.
fn missing_authorization_message(bare_zone: &str, vpc_id: &str, region: &str) -> String {
    format!("No authorization exists for hosted zone: {bare_zone} and vpc: {vpc_id}, {region}")
}

#[tokio::test]
async fn a_missing_authorization_names_the_zone_the_vpc_and_the_requested_region() {
    let router = router();
    let zone = create_private_zone(
        &router,
        "ledger-text.example.com",
        "msg-ledger",
        CREATION_VPC,
    )
    .await;
    let other_zone = create_private_zone(
        &router,
        "ledger-text-other.example.com",
        "msg-ledger-other",
        "vpc-1111222233334444a",
    )
    .await;

    // The authorization that is created and then removed, so one row asks about
    // a VPC whose authorization DID exist.
    let removed = ("vpc-5a6b7c8d", "eu-west-3");
    authorize_ok(&router, &zone, removed.0, removed.1).await;
    deauthorize_ok(&router, &zone, removed.0, removed.1, "the removed one").await;
    // An authorization held by the OTHER zone only.
    let elsewhere = ("vpc-6a7b8c9d", "ap-northeast-1");
    authorize_ok(&router, &other_zone, elsewhere.0, elsewhere.1).await;
    // One authorization on each zone that every refusal must leave alone.
    let kept = ("vpc-11112222333344445", "us-west-2");
    authorize_ok(&router, &zone, kept.0, kept.1).await;

    // The rows differ along the zone the request addresses and the spelling it
    // addresses it with, the VPC id form (the 8- and 17-character forms), the
    // region named (values of the modelled `VPCRegion` enum,
    // smithy_query(query="shape",shape="VPCRegion")), and why the zone does not
    // authorize the VPC: it never did, the authorization was removed, the
    // authorization belongs to another zone, and the VPC is merely ASSOCIATED
    // with the zone.
    let rows = [
        LedgerRow {
            case: "a VPC no authorization ever named, in the 17-character id form",
            requested_zone: zone.clone(),
            bare_zone: zone.clone(),
            vpc_id: "vpc-99998888777766665",
            region: "us-west-2",
        },
        LedgerRow {
            case: "a VPC no authorization ever named, in the 8-character id form",
            requested_zone: zone.clone(),
            bare_zone: zone.clone(),
            vpc_id: "vpc-1a2b3c4d",
            region: "eu-central-1",
        },
        LedgerRow {
            case: "a VPC whose authorization was removed, asked about in its own region",
            requested_zone: zone.clone(),
            bare_zone: zone.clone(),
            vpc_id: removed.0,
            region: removed.1,
        },
        LedgerRow {
            // The region is the REQUEST's, not the one the removed entry
            // carried, so this row's message differs from the one above.
            case: "that same VPC, asked about in another region",
            requested_zone: zone.clone(),
            bare_zone: zone.clone(),
            vpc_id: removed.0,
            region: "ap-south-1",
        },
        LedgerRow {
            case: "a VPC authorized on another zone",
            requested_zone: zone.clone(),
            bare_zone: zone.clone(),
            vpc_id: elsewhere.0,
            region: elsewhere.1,
        },
        LedgerRow {
            case: "the VPC the zone is associated with rather than authorized for",
            requested_zone: zone.clone(),
            bare_zone: zone.clone(),
            vpc_id: CREATION_VPC,
            region: HOME_REGION,
        },
        LedgerRow {
            case: "the /hostedzone/-prefixed spelling of the zone id",
            requested_zone: format!("%2Fhostedzone%2F{zone}"),
            bare_zone: zone.clone(),
            vpc_id: "vpc-7a8b9c0d",
            region: "sa-east-1",
        },
        LedgerRow {
            // Another zone entirely, so a message built from a fixed zone would
            // fail here.
            case: "the other zone, asked about a VPC the first one authorizes",
            requested_zone: other_zone.clone(),
            bare_zone: other_zone.clone(),
            vpc_id: kept.0,
            region: kept.1,
        },
    ];

    for row in &rows {
        let reply = deauthorize(
            &router,
            &row.requested_zone,
            &vpc_block(Some(row.vpc_id), Some(row.region)),
        )
        .await;
        assert_eq!(
            reply.status, 404,
            "case {:?}: expected HTTP 404, got {} with body {}",
            row.case, reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "VPCAssociationAuthorizationNotFound",
            "case {:?}: expected VPCAssociationAuthorizationNotFound, got body {}",
            row.case,
            reply.body
        );

        let got = message_of(&reply, row.case);
        let expected = missing_authorization_message(&row.bare_zone, row.vpc_id, row.region);
        assert_eq!(
            got, expected,
            "case {:?}: expected message {expected:?}, got {got:?}",
            row.case
        );
        // The ledger form, not the association path's sentence: the region a
        // ledger entry is scoped by is part of the refusal.
        assert!(
            got.contains(row.region),
            "case {:?}: the message must name the region {}, got {got:?}",
            row.case,
            row.region
        );
    }
}

// ---------------------------------------------------------------------------
// The conflicting-domain refusal
// ---------------------------------------------------------------------------

/// One domain conflict: two private zones carrying the same name, the VPC the
/// first one holds, and the region the refused request names it in.
struct ConflictRow {
    case: &'static str,
    zone_name: &'static str,
    /// The VPC the first zone is created with — the one the second zone's
    /// association then asks for.
    held_vpc: &'static str,
    /// The VPC the second zone is created with.
    other_vpc: &'static str,
    /// The `VPCRegion` the refused request carries.
    request_region: &'static str,
}

#[tokio::test]
async fn a_domain_conflict_names_the_vpc_its_region_and_the_zone_holding_it() {
    // The rows differ along the shared DNS name, both VPC id forms, and the
    // region the REQUEST names — including a region other than the one the
    // holding zone stored, so a message built from stored state rather than from
    // the request would fail. Every region is a value of the modelled
    // `VPCRegion` enum (smithy_query(query="shape",shape="VPCRegion")).
    let rows = [
        ConflictRow {
            case: "the region the holding zone was created in",
            zone_name: "conflict-one.example.com",
            held_vpc: "vpc-1a2b3c4d",
            other_vpc: "vpc-5e6f7a8b",
            request_region: HOME_REGION,
        },
        ConflictRow {
            case: "a region other than the one the holding zone stored",
            zone_name: "conflict-two.example.org",
            held_vpc: "vpc-0a1b2c3d4e5f60718",
            other_vpc: "vpc-00112233445566778",
            request_region: "eu-west-1",
        },
        ConflictRow {
            case: "a name given with a trailing dot, in another partition's region",
            zone_name: "conflict-three.example.net.",
            held_vpc: "vpc-deadbeef",
            other_vpc: "vpc-feedface",
            request_region: "us-gov-west-1",
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        // A router of its own per row, so one row's zones cannot be read as
        // another's.
        let router = router();
        let holding_zone = create_private_zone(
            &router,
            row.zone_name,
            &format!("msg-conflict-{index}-holder"),
            row.held_vpc,
        )
        .await;
        let newcomer_zone = create_private_zone(
            &router,
            row.zone_name,
            &format!("msg-conflict-{index}-newcomer"),
            row.other_vpc,
        )
        .await;

        let reply = associate(
            &router,
            &newcomer_zone,
            &vpc_block(Some(row.held_vpc), Some(row.request_region)),
            None,
        )
        .await;
        assert_eq!(
            reply.status, 400,
            "case {:?}: expected HTTP 400, got {} with body {}",
            row.case, reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "ConflictingDomainExists",
            "case {:?}: expected ConflictingDomainExists, got body {}",
            row.case,
            reply.body
        );

        let got = message_of(&reply, row.case);
        let expected = format!(
            "The VPC {} in region {} has already been associated with the hosted zone \
             {holding_zone} with the same domain name.",
            row.held_vpc, row.request_region
        );
        assert_eq!(
            got, expected,
            "case {:?}: expected message {expected:?}, got {got:?}",
            row.case
        );
        // The actionable content is the zone holding the VPC, not the name the
        // caller already asked for.
        assert!(
            !got.contains(row.zone_name) && !got.contains(&newcomer_zone),
            "case {:?}: the message must name the holding zone {holding_zone} rather than the \
             name {} or the zone asked about, got {got:?}",
            row.case,
            row.zone_name
        );
    }
}

// ---------------------------------------------------------------------------
// The invalid pagination token
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_invalid_pagination_token_is_reported_as_the_token_alone() {
    let router = router();
    let zone = create_private_zone(
        &router,
        "token-text.example.com",
        "msg-token",
        CREATION_VPC,
    )
    .await;
    // One authorization, so the listing has a page to resume and the token is
    // the only thing wrong with the request.
    authorize_ok(&router, &zone, "vpc-11112222333344445", "us-west-2").await;

    // The tokens differ along length, character class and how closely they
    // resemble a token one of these listings could have minted (a VPC id, a
    // zone id, a bare number) — none of them identifies a page in either
    // listing. Each row is `(the token as the query string sends it, the token
    // the service receives)`; the last row carries XML metacharacters, so the
    // refusal has to report the token AND leave its own envelope well-formed.
    let long_token = "Z".repeat(40);
    let rows = [
        (
            "not-a-real-pagination-token-0123456789",
            "not-a-real-pagination-token-0123456789",
        ),
        ("0", "0"),
        ("vpc-deadbeef", "vpc-deadbeef"),
        ("Zone-1", "Zone-1"),
        ("token", "token"),
        (long_token.as_str(), long_token.as_str()),
        ("%3Ctoken%3E", "<token>"),
    ];

    for (sent, token) in rows {
        let case = format!("the token {token:?}");
        let expected = format!("Invalid pagination token: {token}");

        // The zone-scoped listing of authorizations.
        let authorizations = list_authorizations(&router, &zone, &format!("?nexttoken={sent}"))
            .await;
        assert_eq!(
            authorizations.status, 400,
            "case {case:?}: expected HTTP 400 from ListVPCAssociationAuthorizations, got {} with \
             body {}",
            authorizations.status, authorizations.body
        );
        assert_eq!(
            authorizations.error_code(),
            "InvalidPaginationToken",
            "case {case:?}: expected InvalidPaginationToken, got body {}",
            authorizations.body
        );
        let from_authorizations = message_of(&authorizations, &case);
        assert_eq!(
            from_authorizations, expected,
            "case {case:?}: expected message {expected:?}, got {from_authorizations:?}"
        );
        // The refusal is about the TOKEN, so it claims nothing about the zone or
        // the VPCs the listing would have reported.
        assert!(
            !from_authorizations.contains(&zone),
            "case {case:?}: the message must not name the zone {zone}, got \
             {from_authorizations:?}"
        );

        // The other paginated listing that refuses a token reads the SAME
        // sentence: the wording belongs to the fact, not to the operation.
        let by_vpc = list_by_vpc(&router, CREATION_VPC, HOME_REGION, None, Some(sent)).await;
        assert_eq!(
            by_vpc.status, 400,
            "case {case:?}: expected HTTP 400 from ListHostedZonesByVPC, got {} with body {}",
            by_vpc.status, by_vpc.body
        );
        assert_eq!(
            by_vpc.error_code(),
            "InvalidPaginationToken",
            "case {case:?}: expected InvalidPaginationToken, got body {}",
            by_vpc.body
        );
        let from_by_vpc = message_of(&by_vpc, &case);
        assert_eq!(
            from_by_vpc, expected,
            "case {case:?}: expected message {expected:?}, got {from_by_vpc:?}"
        );

        // A caller-influenced value is output-encoded into the envelope rather
        // than written into it raw, so the body stays well-formed XML whatever
        // the token carried.
        for (operation, body) in [
            ("ListVPCAssociationAuthorizations", &authorizations.body),
            ("ListHostedZonesByVPC", &by_vpc.body),
        ] {
            assert!(
                !body.contains("<token>"),
                "case {case:?}: {operation} must encode the token into its envelope, got body \
                 {body}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// The traffic policy document's violated constraints
// ---------------------------------------------------------------------------

/// One document to submit, and every constraint clause its refusal must report,
/// in order.
struct DocumentRow {
    case: &'static str,
    document: String,
    clauses: Vec<String>,
}

/// The clause a document that declares no endpoint is refused with.
///
/// `pub(crate)` so the sibling suite asserting the record-type clause's
/// POSITION reads the clauses it is positioned against from here rather than
/// growing a second copy of them.
pub(crate) const NO_ENDPOINT: &str = "At least one endpoint must be declared.";

/// The clause a document without exactly one entry point is refused with.
pub(crate) const ENTRY_POINT: &str =
    "Either StartRule or StartEndpoint must be declared, but not both";

/// The ONE clause a document that declares no record type the service supports
/// is refused with, reported after every other clause (`crate::state`'s
/// `RECORD_TYPE_CLAUSE`; the suite `refusal_message_forms` is where that rule's
/// own table lives, and it reads the clause from here).
pub(crate) const RECORD_TYPE: &str = "Policy RecordType is missing or not supported.";

#[tokio::test]
async fn a_faulty_document_reports_every_constraint_it_violates() {
    // The rows differ along HOW MANY constraints one document breaks (one, two,
    // four, six) and along which ones, so a refusal that stopped at the first
    // violation would fail every row with more than one clause. Each clause
    // states one constraint of the published traffic policy document format.
    let rows = vec![
        DocumentRow {
            case: "only the format version and the record type",
            document: format!(
                "{{\"AWSPolicyFormatVersion\":\"{TRAFFIC_POLICY_FORMAT_VERSION}\",\
                 \"RecordType\":\"A\"}}"
            ),
            clauses: vec![NO_ENDPOINT.to_string(), ENTRY_POINT.to_string()],
        },
        DocumentRow {
            case: "an empty JSON object, which declares nothing at all",
            document: "{}".to_string(),
            clauses: vec![
                "AWSPolicyFormatVersion must be declared.".to_string(),
                NO_ENDPOINT.to_string(),
                ENTRY_POINT.to_string(),
                RECORD_TYPE.to_string(),
            ],
        },
        DocumentRow {
            case: "an unsupported version, an unknown record type, an empty Endpoints block \
                   and both entry points",
            document: "{\"AWSPolicyFormatVersion\":\"1999-01-01\",\"RecordType\":\"NOTATYPE\",\
                       \"Endpoints\":{},\"StartEndpoint\":\"first\",\"StartRule\":\"second\"}"
                .to_string(),
            clauses: vec![
                format!(
                    "AWSPolicyFormatVersion 1999-01-01 is not a supported traffic policy \
                     document format version, the supported version is \
                     {TRAFFIC_POLICY_FORMAT_VERSION}."
                ),
                NO_ENDPOINT.to_string(),
                ENTRY_POINT.to_string(),
                "StartEndpoint first does not reference a declared endpoint.".to_string(),
                "StartRule second does not reference a declared rule.".to_string(),
                RECORD_TYPE.to_string(),
            ],
        },
        DocumentRow {
            // ONE violated constraint, so the message is one clause and carries
            // no separator at all.
            case: "a well-formed document with no RecordType",
            document: format!(
                "{{\"AWSPolicyFormatVersion\":\"{TRAFFIC_POLICY_FORMAT_VERSION}\",\
                 \"StartEndpoint\":\"only\",\
                 \"Endpoints\":{{\"only\":{{\"Type\":\"value\",\"Value\":\"192.0.2.1\"}}}}}}"
            ),
            clauses: vec![RECORD_TYPE.to_string()],
        },
        DocumentRow {
            case: "an endpoint that is not a JSON object",
            document: format!(
                "{{\"AWSPolicyFormatVersion\":\"{TRAFFIC_POLICY_FORMAT_VERSION}\",\
                 \"RecordType\":\"A\",\"StartEndpoint\":\"only\",\
                 \"Endpoints\":{{\"only\":\"192.0.2.1\"}}}}"
            ),
            clauses: vec!["The endpoint only must be declared as a JSON object.".to_string()],
        },
        DocumentRow {
            case: "a StartRule naming no declared rule, with no Rules block",
            document: format!(
                "{{\"AWSPolicyFormatVersion\":\"{TRAFFIC_POLICY_FORMAT_VERSION}\",\
                 \"RecordType\":\"AAAA\",\"StartRule\":\"switch\",\
                 \"Endpoints\":{{\"only\":{{\"Type\":\"value\",\"Value\":\"2001:db8::1\"}}}}}}"
            ),
            clauses: vec!["StartRule switch does not reference a declared rule.".to_string()],
        },
        DocumentRow {
            // A document nothing can be checked inside is the whole refusal on
            // its own.
            case: "a body that is not JSON",
            document: "this is not a policy".to_string(),
            clauses: vec![
                "The traffic policy document must be declared as valid JSON.".to_string(),
            ],
        },
        DocumentRow {
            case: "JSON that is not an object",
            document: "[\"not\",\"an\",\"object\"]".to_string(),
            clauses: vec![
                "The traffic policy document must be declared as a JSON object.".to_string(),
            ],
        },
    ];

    let router = router();
    for row in &rows {
        let reply = create_policy(&router, row.case, &row.document, None).await;
        assert_eq!(
            reply.status, 400,
            "case {:?}: expected HTTP 400, got {} with body {}",
            row.case, reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "InvalidTrafficPolicyDocument",
            "case {:?}: expected InvalidTrafficPolicyDocument, got body {}",
            row.case,
            reply.body
        );

        let got = message_of(&reply, row.case);
        let expected = row.clauses.join(";");
        assert_eq!(
            got, expected,
            "case {:?}: expected message {expected:?}, got {got:?}",
            row.case
        );
        // One clause per violated constraint: the semicolons are separators, so
        // the clause count is the violation count.
        let reported: Vec<&str> = got.split(';').collect();
        assert_eq!(
            reported.len(),
            row.clauses.len(),
            "case {:?}: expected {} clauses, got {reported:?}",
            row.case,
            row.clauses.len()
        );
    }

    // The positive control: a document the format DOES define breaks no
    // constraint, so it is created rather than refused.
    let accepted = create_policy(
        &router,
        "well-formed-document",
        &document_with_start_endpoint("A", "192.0.2.1"),
        None,
    )
    .await;
    assert_eq!(
        accepted.status, 201,
        "a well-formed document must be created, got {} with body {}",
        accepted.status, accepted.body
    );
}
