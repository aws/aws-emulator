// SPDX-License-Identifier: CC0-1.0

//! An identifier that could have been issued, and the store holds nothing for, is an ABSENT
//! resource — never an argument CloudFront cannot read, end to end over HTTP.
//!
//! The rule: a path-bound identifier of a namespaced CloudFront resource is opaque — the
//! members that carry one (`AssociateDistributionTenantWebACL$Id`, `GetAnycastIpList$Id`,
//! `DeleteDistributionTenant$Id`, …) are plain `string` in the model, with no `@pattern` and
//! no `@length` — so the only thing the REQUEST alone decides is whether the value could be
//! such an identifier at all: a lowercase namespace label, a separator and a body. Which
//! identifiers were ever ISSUED, and to what, is a property of the STORE. So a value of that
//! form that nothing answers to is the resource reported ABSENT (404 `EntityNotFound` with
//! that resource's fixed sentence) whatever namespace label, body width or body alphabet it
//! carries, while a value of no such form is the identifier that cannot be read at all (400
//! `InvalidArgument`, "Id format is invalid", for the operations whose model serves one).
//!
//! The rows span what the rule generalises over. The identifiers the store holds nothing for
//! vary the parts nothing constrains — the namespace label (the ones this service mints, and
//! others), the separator (`_` and `-`), the body's width (1 to 32 characters) and the body's
//! alphabet (upper case, lower case, mixed, digits, embedded separators) — and the values of
//! no identifier form vary how the form is broken (no separator at all, an empty or over-long
//! or upper-case label, an empty body, a body outside the identifier alphabet, a grossly
//! malformed value, a lone separator, a UUID). Both tables are driven against two resource
//! kinds and every operation that addresses one by a path identifier, and the live resources
//! are read back afterwards to show the refusals touched nothing.
//!
//! How WIDE an operation reads its identifier is its own recorded contract:
//! `GetInvalidationForDistributionTenant` answers the identifier-format sentence for a tenant
//! identifier at a width this service does not mint, where its siblings answer the tenant
//! absent, so it carries its own table at the end of this suite rather than riding the
//! shared one.
//!
//! The wire harness and the create helpers are the crate's own.

use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::{
    id_format_invalid_message, ANYCAST_IP_LIST_NOT_FOUND_MESSAGE, NO_SUCH_INVALIDATION_MESSAGE,
};

use crate::anycast_ip_list_lifecycle::{create_anycast_ip_list, get_anycast_ip_list, Create};
use crate::distribution_lifecycle::{call, emulator, Answer, API_VERSION};
use crate::distribution_tenant_lifecycle::multi_tenant_distribution;
use crate::tenant_web_acl_association::{associate, create_tenant, disassociate, STARTS};

/// The sentence every read of a distribution tenant the store does not hold answers with.
const TENANT_NOT_FOUND: &str = "The specified distribution tenant doesn't exist.";

/// A WAF web ACL ARN an association names. Nothing resolves it, so it only has to be a
/// non-blank value the association could store.
const WEB_ACL_ARN: &str =
    "arn:aws:wafv2:us-east-1:000000000000:global/webacl/AbsentId/9f1c0d6e-0a7b-4c1d-9a2f-3e4d5c6b7a89";

/// An invalidation id the tenant-scoped read addresses — never minted, so what that read
/// answers is decided by the TENANT identifier the row varies.
const INVALIDATION_ID: &str = "I1111111111111";

// ---------------------------------------------------------------------------
// The resource kinds and the operations that address them
// ---------------------------------------------------------------------------

/// The resource kinds whose ids are opaque and namespaced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    AnycastIpList,
    DistributionTenant,
}

/// One operation that addresses its resource by a path identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    GetAnycastIpList,
    GetDistributionTenant,
    DeleteDistributionTenant,
    UpdateDistributionTenant,
    AssociateDistributionTenantWebAcl,
    DisassociateDistributionTenantWebAcl,
    CreateInvalidationForDistributionTenant,
    ListInvalidationsForDistributionTenant,
    GetInvalidationForDistributionTenant,
}

/// Every operation whose recorded contract resolves any identifier of the namespace form.
const OPS: &[Op] = &[
    Op::GetAnycastIpList,
    Op::GetDistributionTenant,
    Op::DeleteDistributionTenant,
    Op::UpdateDistributionTenant,
    Op::AssociateDistributionTenantWebAcl,
    Op::DisassociateDistributionTenantWebAcl,
    Op::CreateInvalidationForDistributionTenant,
    Op::ListInvalidationsForDistributionTenant,
];

impl Op {
    /// The resource kind this operation's path identifier names.
    fn kind(self) -> Kind {
        match self {
            Self::GetAnycastIpList => Kind::AnycastIpList,
            _ => Kind::DistributionTenant,
        }
    }

    /// What the operation answers an identifier the store holds nothing for:
    /// `(status, code, message)`.
    fn absent_resource(self) -> (u16, &'static str, &'static str) {
        match self.kind() {
            Kind::AnycastIpList => (404, "EntityNotFound", ANYCAST_IP_LIST_NOT_FOUND_MESSAGE),
            Kind::DistributionTenant => (404, "EntityNotFound", TENANT_NOT_FOUND),
        }
    }

    /// What the operation answers a value that could be no identifier of its resource at
    /// all: `(status, code, message)`.
    ///
    /// `InvalidArgument` (400) where the operation's model serves one.
    /// `GetDistributionTenant` is the one operation that reads no form at all — its
    /// `Identifier` is "the ARN, ID, or name of the distribution tenant", so every value is
    /// one of those namespaces to resolve, and its model serves nothing but `AccessDenied`
    /// and `EntityNotFound`.
    fn unreadable_identifier(self) -> (u16, &'static str, String) {
        match self {
            Self::GetDistributionTenant => (404, "EntityNotFound", TENANT_NOT_FOUND.to_string()),
            Self::GetInvalidationForDistributionTenant => (
                404,
                "EntityNotFound",
                id_format_invalid_message("DistributionTenantId"),
            ),
            _ => (400, "InvalidArgument", id_format_invalid_message("Id")),
        }
    }
}

/// Call `op` with `identifier` as its path identifier, over the operation's own wire
/// binding. `version` is the `If-Match` the conditional mutations run under — the live
/// resource's current version, so only the identifier can decide the answer.
async fn call_op(
    router: &MultiServiceRouter,
    op: Op,
    identifier: &str,
    version: &str,
) -> Answer {
    match op {
        Op::GetAnycastIpList => get_anycast_ip_list(router, identifier).await,
        Op::GetDistributionTenant => {
            call(
                router,
                "GET",
                &format!("/{API_VERSION}/distribution-tenant/{identifier}"),
                None,
                None,
            )
            .await
        }
        Op::DeleteDistributionTenant => {
            call(
                router,
                "DELETE",
                &format!("/{API_VERSION}/distribution-tenant/{identifier}"),
                None,
                Some(version),
            )
            .await
        }
        Op::UpdateDistributionTenant => {
            let body = format!(
                "<UpdateDistributionTenantRequest \
                 xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
                 <Enabled>false</Enabled></UpdateDistributionTenantRequest>"
            );
            call(
                router,
                "PUT",
                &format!("/{API_VERSION}/distribution-tenant/{identifier}"),
                Some(&body),
                Some(version),
            )
            .await
        }
        Op::AssociateDistributionTenantWebAcl => {
            associate(router, identifier, WEB_ACL_ARN, Some(version)).await
        }
        Op::DisassociateDistributionTenantWebAcl => {
            disassociate(router, identifier, Some(version)).await
        }
        Op::CreateInvalidationForDistributionTenant => {
            let body = format!(
                "<InvalidationBatch \
                 xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
                 <Paths><Quantity>1</Quantity><Items><Path>/*</Path></Items></Paths>\
                 <CallerReference>{version}</CallerReference></InvalidationBatch>"
            );
            call(
                router,
                "POST",
                &format!("/{API_VERSION}/distribution-tenant/{identifier}/invalidation"),
                Some(&body),
                None,
            )
            .await
        }
        Op::ListInvalidationsForDistributionTenant => {
            call(
                router,
                "GET",
                &format!("/{API_VERSION}/distribution-tenant/{identifier}/invalidation"),
                None,
                None,
            )
            .await
        }
        Op::GetInvalidationForDistributionTenant => {
            call(
                router,
                "GET",
                &format!(
                    "/{API_VERSION}/distribution-tenant/{identifier}/invalidation/{INVALIDATION_ID}"
                ),
                None,
                None,
            )
            .await
        }
    }
}

// ---------------------------------------------------------------------------
// The identifiers under test
// ---------------------------------------------------------------------------

/// Identifiers that COULD have been issued for a namespaced resource and that no create in
/// this suite handed out: `(case, value)`.
///
/// The rows vary every part of such an identifier that nothing in the model constrains — the
/// namespace label, the separator, and the body's width and alphabet — because none of them
/// can refuse an identifier: only the store knows which were issued. Two of them carry the
/// labels this service MINTS under (`dt_`, `ipl-`), the rest carry labels it never mints, so
/// the rule is asserted over the namespace class rather than over one emulator's mint.
fn could_have_been_issued() -> Vec<(&'static str, String)> {
    vec![
        (
            "the label and body this service mints a tenant id under",
            "dt_NEVERMINTED12".to_string(),
        ),
        (
            "a tenant id at the width and mixed-case alphabet AWS writes one in",
            "dt_3JO1OQa51Jb3BZh7XQh6pbvF3Wa".to_string(),
        ),
        (
            "the label and body this service mints a list id under",
            "ipl-00000000000000000".to_string(),
        ),
        (
            "a list id under another label and the other separator",
            "aip_1A2B3C4D5E6F7G8H9J0K1L".to_string(),
        ),
        (
            "a label this service never mints, over a minted body",
            "zz_NEVERMINTED12".to_string(),
        ),
        (
            "a one-letter label and a one-character body",
            "d_0".to_string(),
        ),
        (
            "a body carrying both separators",
            "ipl-not_a-minted-id".to_string(),
        ),
        (
            "a body of 32 digits",
            "cg_99999999999999999999999999999999".to_string(),
        ),
        (
            "the longest label, under the other separator",
            "aip-0123456789abcdef".to_string(),
        ),
    ]
}

/// Values that could be NO identifier of a namespaced resource: `(case, value)`.
///
/// One row per way the form is broken — no separator at all, the label dropped, the label
/// upper-cased, a label longer than a namespace label, an empty body, a body outside the
/// identifier alphabet, a grossly malformed value, a lone separator, and a UUID (the shape of
/// an id of a resource kind whose ids are NOT namespaced).
fn could_be_no_identifier() -> Vec<(&'static str, String)> {
    vec![
        ("a bare word, with no separator", "notanid".to_string()),
        (
            "the minted form with its separator dropped",
            "dtNEVERMINTED12".to_string(),
        ),
        (
            "an upper-case namespace label",
            "DT_NEVERMINTED12".to_string(),
        ),
        (
            "a label longer than a namespace label",
            "tenant-never-minted".to_string(),
        ),
        ("the label alone, with an empty body", "dt_".to_string()),
        (
            "a body outside the identifier alphabet",
            "dt_NEVER.MINTED".to_string(),
        ),
        ("a grossly malformed value", "A".repeat(512)),
        ("a lone separator", "-".to_string()),
        (
            "a UUID, which carries no namespace label",
            "f9a6a0b4-8d39-4a1e-9f67-1b0f7f5d2c3e".to_string(),
        ),
    ]
}

// ---------------------------------------------------------------------------
// The resources that really exist
// ---------------------------------------------------------------------------

/// One live resource of each kind: the id its create minted and the version it handed back.
#[derive(Debug, Clone)]
struct Live {
    id: String,
    etag: String,
}

/// Seed one Anycast static IP list and one distribution tenant.
async fn seed(router: &MultiServiceRouter) -> (Live, Live) {
    let create = Create {
        case: "the list the absence rule is asserted around",
        name: "absent-id-list".to_string(),
        ip_count: 3,
        ip_address_type: None,
        ipam_cidr_configs: Vec::new(),
        tags: Vec::new(),
    };
    let created = create_anycast_ip_list(router, &create.xml()).await;
    assert_eq!(
        created.status, 202,
        "case {}: expected HTTP 202 from CreateAnycastIpList, got {} with {}",
        create.case, created.status, created.raw
    );
    let list = Live {
        id: created.root().text_at("Id").unwrap_or_default().to_string(),
        etag: created.header("etag").to_string(),
    };

    let distribution_id = multi_tenant_distribution(router, "ref-absent-identifier").await;
    let (id, etag) = create_tenant(
        router,
        &distribution_id,
        "absentidtenant",
        "the tenant the absence rule is asserted around",
        STARTS[0],
    )
    .await;

    (list, Live { id, etag })
}

// ---------------------------------------------------------------------------
// The rule
// ---------------------------------------------------------------------------

/// Every operation that addresses a namespaced resource by a path identifier answers an
/// identifier the store holds nothing for as the resource ABSENT — with its modelled
/// not-found code, status and fixed sentence — whatever namespace label, body width or body
/// alphabet the value carries.
#[tokio::test]
async fn an_identifier_the_store_holds_nothing_for_is_an_absent_resource() {
    let router = emulator();
    let (list, tenant) = seed(&router).await;

    for op in OPS {
        let live = match op.kind() {
            Kind::AnycastIpList => &list,
            Kind::DistributionTenant => &tenant,
        };
        let (status, code, message) = op.absent_resource();
        for (case, identifier) in could_have_been_issued() {
            let answer = call_op(&router, *op, &identifier, &live.etag).await;
            assert_eq!(
                answer.status, status,
                "case {op:?}/{case} ({identifier:?}): expected HTTP {status}, got {} with {}",
                answer.status, answer.raw
            );
            assert_eq!(
                answer.error_code(),
                code,
                "case {op:?}/{case} ({identifier:?}): expected {code}, got {:?} with {}",
                answer.error_code(),
                answer.raw
            );
            let actual = answer.root().find_text("Message").unwrap_or_default();
            assert_eq!(
                actual, message,
                "case {op:?}/{case} ({identifier:?}): expected {message:?}, got {actual:?} in {}",
                answer.raw
            );
        }
    }

    // Every refusal above touched nothing: both live resources are still readable, at the
    // versions their creates handed back.
    let read = get_anycast_ip_list(&router, &list.id).await;
    assert_eq!(
        read.status, 200,
        "case the live Anycast static IP list: expected HTTP 200, got {} with {}",
        read.status, read.raw
    );
    assert_eq!(
        read.header("etag"),
        list.etag,
        "case the live Anycast static IP list: expected the version {:?}, got {:?}",
        list.etag,
        read.header("etag")
    );
    let read = call_op(&router, Op::GetDistributionTenant, &tenant.id, &tenant.etag).await;
    assert_eq!(
        read.status, 200,
        "case the live distribution tenant: expected HTTP 200, got {} with {}",
        read.status, read.raw
    );
    assert_eq!(
        read.header("etag"),
        tenant.etag,
        "case the live distribution tenant: expected the version {:?}, got {:?}",
        tenant.etag,
        read.header("etag")
    );
}

/// The contrast that makes the rule about the identifier's FORM rather than about absence: a
/// value that could be no identifier of its resource at all is reported as the unreadable
/// argument it is, under the code each operation's model serves for one.
#[tokio::test]
async fn a_value_of_no_identifier_form_is_an_argument_failure() {
    let router = emulator();
    let (list, tenant) = seed(&router).await;

    for op in OPS {
        let live = match op.kind() {
            Kind::AnycastIpList => &list,
            Kind::DistributionTenant => &tenant,
        };
        let (status, code, message) = op.unreadable_identifier();
        for (case, identifier) in could_be_no_identifier() {
            let answer = call_op(&router, *op, &identifier, &live.etag).await;
            assert_eq!(
                answer.status, status,
                "case {op:?}/{case} ({identifier:?}): expected HTTP {status}, got {} with {}",
                answer.status, answer.raw
            );
            assert_eq!(
                answer.error_code(),
                code,
                "case {op:?}/{case} ({identifier:?}): expected {code}, got {:?} with {}",
                answer.error_code(),
                answer.raw
            );
            let actual = answer.root().find_text("Message").unwrap_or_default();
            assert_eq!(
                actual, message,
                "case {op:?}/{case} ({identifier:?}): expected {message:?}, got {actual:?} in {}",
                answer.raw
            );
        }
    }
}

/// Each operation under the rule still SERVES the resource that exists, addressed by the id
/// its create minted at the version that create handed back: the form check refuses only the
/// identifiers that could be none. One emulator per operation, because several of them mutate
/// or remove the record they are given.
#[tokio::test]
async fn the_form_check_still_serves_the_resource_that_exists() {
    for op in OPS {
        let router = emulator();
        let (list, tenant) = seed(&router).await;
        let live = match op.kind() {
            Kind::AnycastIpList => &list,
            Kind::DistributionTenant => &tenant,
        };

        // `DeleteDistributionTenant` refuses a tenant that is still enabled, so the row that
        // deletes one disables it first — through the same update the table drives.
        let mut version = live.etag.clone();
        if *op == Op::DeleteDistributionTenant {
            let disabled =
                call_op(&router, Op::UpdateDistributionTenant, &live.id, &version).await;
            assert_eq!(
                disabled.status, 200,
                "case {op:?}: expected HTTP 200 disabling the tenant first, got {} with {}",
                disabled.status, disabled.raw
            );
            version = disabled.header("etag").to_string();
        }

        let answer = call_op(&router, *op, &live.id, &version).await;
        let expected = match op {
            Op::DeleteDistributionTenant => 204,
            Op::CreateInvalidationForDistributionTenant => 201,
            _ => 200,
        };
        assert_eq!(
            answer.status, expected,
            "case {op:?}: expected HTTP {expected} for the resource that exists, got {} with {}",
            answer.status, answer.raw
        );
    }
}

/// `GetInvalidationForDistributionTenant` reads its tenant identifier against the shape a
/// tenant id this service MINTS takes, which is its own recorded contract rather than its
/// siblings': an identifier at another width or under another label is reported under its
/// own not-found code with the sentence naming the member it arrived in, while a
/// mint-shaped identifier the store holds no tenant for is the absent tenant. The tenant
/// that EXISTS is resolved either way, so what refuses that call is the invalidation id.
#[tokio::test]
async fn the_tenant_scoped_invalidation_read_keeps_its_own_identifier_contract() {
    let router = emulator();
    let (_list, tenant) = seed(&router).await;
    let op = Op::GetInvalidationForDistributionTenant;
    let member_format_invalid = id_format_invalid_message("DistributionTenantId");

    // Mint-shaped identifiers no create handed out: the tenant is ABSENT.
    for (case, identifier) in [
        ("an id never minted", "dt_3JO1OQa51Jb3BZh7XQh6pbvF3Wa"),
        ("another id never minted", "dt_000000000000000000000000000"),
    ] {
        let answer = call_op(&router, op, identifier, &tenant.etag).await;
        assert_eq!(
            answer.status, 404,
            "case {case} ({identifier:?}): expected HTTP 404, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            "EntityNotFound",
            "case {case} ({identifier:?}): expected EntityNotFound, got {:?} with {}",
            answer.error_code(),
            answer.raw
        );
        let actual = answer.root().find_text("Message").unwrap_or_default();
        assert_eq!(
            actual, TENANT_NOT_FOUND,
            "case {case} ({identifier:?}): expected {TENANT_NOT_FOUND:?}, got {actual:?} in {}",
            answer.raw
        );
    }

    // Identifiers outside that shape, in every form one takes: the member-naming sentence.
    let grossly_malformed = "A".repeat(512);
    for (case, identifier) in [
        ("a body at another width", "dt_NEVERMINTED12"),
        ("a body shorter than the mint's", "dt_SHORTTOKEN"),
        ("another namespace label", "zz_3JO1OQa51Jb3BZh7XQh6pbvF3Wa"),
        (
            "the minted form with its separator dropped",
            "dt3JO1OQa51Jb3BZh7XQh6pbvF3Wa",
        ),
        ("a bare word", "notanid"),
        ("a grossly malformed value", grossly_malformed.as_str()),
    ] {
        let answer = call_op(&router, op, identifier, &tenant.etag).await;
        assert_eq!(
            answer.status, 404,
            "case {case} ({identifier:?}): expected HTTP 404, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            "EntityNotFound",
            "case {case} ({identifier:?}): expected EntityNotFound, got {:?} with {}",
            answer.error_code(),
            answer.raw
        );
        let actual = answer.root().find_text("Message").unwrap_or_default();
        assert_eq!(
            actual, member_format_invalid,
            "case {case} ({identifier:?}): expected {member_format_invalid:?}, got {actual:?} in \
             {}",
            answer.raw
        );
    }

    // The tenant that exists: the read reaches past it, so the invalidation id is what
    // refuses the call.
    let answer = call_op(&router, op, &tenant.id, &tenant.etag).await;
    assert_eq!(
        answer.status, 404,
        "case the live distribution tenant: expected HTTP 404 from the invalidation id, got {} \
         with {}",
        answer.status, answer.raw
    );
    let actual = answer.root().find_text("Message").unwrap_or_default();
    assert_eq!(
        actual, NO_SUCH_INVALIDATION_MESSAGE,
        "case the live distribution tenant: expected the refusal to be about the invalidation \
         rather than the tenant ({NO_SUCH_INVALIDATION_MESSAGE:?}), got {actual:?} in {}",
        answer.raw
    );
}
