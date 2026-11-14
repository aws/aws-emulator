// SPDX-License-Identifier: CC0-1.0

//! What CloudFront decides from the REQUEST before it consults the store, end to end over
//! HTTP.
//!
//! Two rules, both about ordering, and both per OPERATION rather than per resource kind —
//! each operation's own recorded behaviour decides whether it reads the identifier's form
//! before resolving it, exactly as the response-headers-policy family already shows (its
//! config read answers the shape sentence while its listing answers the absent-policy one):
//!
//! * an identifier bound to the path is checked for the FORM an identifier of its resource
//!   can take before it is resolved: a value that could be no such identifier at all is
//!   reported as the unreadable argument it is — `InvalidArgument` (400) where the
//!   operation's model serves one, otherwise under the operation's own
//!   unresolvable-identifier code, with the message naming the MEMBER the identifier arrived
//!   in — and never as a resource that is absent. A value that COULD be one and that the
//!   store holds nothing for is the absent-resource answer, whatever namespace label, body
//!   width or body alphabet it takes, which is the contrast that makes the rule about the
//!   identifier's form rather than about absence;
//! * a mutation that runs under an `If-Match` evaluates the header before it reports on the
//!   resource: what the header has to be is the current version of a resource the id can
//!   ADDRESS, so a header that is absent or is no version token at all, and any header at
//!   all for an id outside the shape its resource's ids take, is `InvalidIfMatchVersion`
//!   (400) — while an identifier of the right shape that the store holds nothing for is the
//!   absent-resource answer.
//!
//! The rows span what the rules generalise over: three resource kinds whose ids take three
//! different shapes (an Anycast static IP list's `ipl-` + lowercase hex, a distribution
//! tenant's `dt_` + the minted token alphabet, a key group's UUID), every operation the
//! recorded behaviour puts under each rule, both member names those identifiers arrive in
//! (`Id` and `DistributionTenantId`), and six forms an identifier can take that is no
//! identifier at all — another namespace's id, a namespace label with no separator, the
//! right characters in the wrong case, the resource's NAME, a bare word and a lone
//! separator. Each kind also carries identifiers the store holds nothing for — including, for
//! the Anycast static IP list, other labels, other body widths and other body alphabets than
//! the one this service mints — and each operation is exercised against the resource that
//! really exists, so the precheck is shown not to refuse the calls it must serve. How wide
//! each OPERATION reads a tenant identifier is its own recorded contract, so the identifiers
//! that only the siblings of `GetInvalidationForDistributionTenant` resolve live in
//! [`crate::absent_identifier_namespaces`] rather than in the tables here.
//!
//! Every assertion travels the router, so each case exercises routing, the restXml reader,
//! the handler, the persisted state and serialization. The request harness and the create
//! helpers are the crate's own.

use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::{
    id_format_invalid_message, DISTRIBUTION_TENANT_ID_PREFIX, INVALID_IF_MATCH_VERSION_MESSAGE,
    NO_SUCH_KEY_GROUP_MESSAGE,
};

use crate::anycast_ip_list_lifecycle::{create_anycast_ip_list, get_anycast_ip_list, Create};
use crate::continuous_deployment_policies::{create_policy, policy_table, staged};
use crate::distribution_lifecycle::{call, config, emulator, Answer, API_VERSION};
use crate::distribution_tenant_lifecycle::multi_tenant_distribution;
use crate::key_group_lifecycle::{create_group, get_group, mint_public_keys, Group};
use crate::tenant_web_acl_association::{associate, create_tenant, disassociate, STARTS};
use crate::web_acl_association::create as create_distribution_returning_id;

/// The sentence a read of an Anycast static IP list the store does not hold answers with,
/// as the recorded contract carries it (no final period).
const ANYCAST_IP_LIST_NOT_FOUND: &str = "The specified Anycast IP list does not exist";

/// The sentence every read of a distribution tenant the store does not hold answers with.
const TENANT_NOT_FOUND: &str = "The specified distribution tenant doesn't exist.";

/// A WAF web ACL ARN an association names. Nothing resolves it, so it only has to be a
/// non-blank value the association can store.
const WEB_ACL_ARN: &str =
    "arn:aws:wafv2:us-east-1:000000000000:global/webacl/PathIdShape/473e64fd-f30b-4765-81a0-62ad96dd167a";

/// An invalidation id the tenant-scoped read addresses — never minted, so what the read
/// answers is decided by the TENANT identifier the row varies.
const INVALIDATION_ID: &str = "I1111111111111";

// ---------------------------------------------------------------------------
// The operations under test
// ---------------------------------------------------------------------------

/// One operation whose recorded behaviour reads the identifier's shape before resolving it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    GetAnycastIpList,
    DeleteDistributionTenant,
    UpdateDistributionTenant,
    AssociateDistributionTenantWebAcl,
    DisassociateDistributionTenantWebAcl,
    CreateInvalidationForDistributionTenant,
    GetInvalidationForDistributionTenant,
    ListInvalidationsForDistributionTenant,
    GetKeyGroup,
}

impl Op {
    /// The resource kind the operation addresses, which is the id shape it reads.
    fn kind(self) -> Kind {
        match self {
            Self::GetAnycastIpList => Kind::AnycastIpList,
            Self::GetKeyGroup => Kind::KeyGroup,
            _ => Kind::DistributionTenant,
        }
    }

    /// What the operation answers an identifier outside its resource's id shape:
    /// `(status, code, message)`.
    ///
    /// `InvalidArgument` where this operation's model serves one. Where it does not, the
    /// code and status are the operation's own unresolvable-identifier ones and the message
    /// carries the argument defect: `GetInvalidationForDistributionTenant` models
    /// `EntityNotFound` and `NoSuchInvalidation` only, and `GetKeyGroup` models
    /// `NoSuchResource` alone.
    fn unreadable_identifier(self) -> (u16, &'static str, String) {
        match self {
            Self::GetInvalidationForDistributionTenant => (
                404,
                "EntityNotFound",
                id_format_invalid_message("DistributionTenantId"),
            ),
            Self::GetKeyGroup => (404, "NoSuchResource", id_format_invalid_message("Id")),
            _ => (400, "InvalidArgument", id_format_invalid_message("Id")),
        }
    }

    /// What the operation answers a WELL-FORMED identifier the store holds no resource for.
    fn absent_resource(self) -> (u16, &'static str, &'static str) {
        match self.kind() {
            Kind::AnycastIpList => (404, "EntityNotFound", ANYCAST_IP_LIST_NOT_FOUND),
            Kind::DistributionTenant => (404, "EntityNotFound", TENANT_NOT_FOUND),
            Kind::KeyGroup => (404, "NoSuchResource", NO_SUCH_KEY_GROUP_MESSAGE),
        }
    }

    /// The status the operation answers for the resource that really exists, so the
    /// precheck is shown not to refuse a call it must serve.
    fn served_status(self) -> u16 {
        match self {
            Self::DeleteDistributionTenant => 204,
            Self::CreateInvalidationForDistributionTenant => 201,
            // The tenant-scoped invalidation READ names an invalidation nothing created, so
            // what it reaches past the tenant is that invalidation's own refusal.
            Self::GetInvalidationForDistributionTenant => 404,
            _ => 200,
        }
    }
}

/// The three resource kinds whose ids this suite addresses, each with its own id shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    AnycastIpList,
    DistributionTenant,
    KeyGroup,
}

/// Every operation under the shape rule, in the order the suite reports them.
const OPS: &[Op] = &[
    Op::GetAnycastIpList,
    Op::DeleteDistributionTenant,
    Op::UpdateDistributionTenant,
    Op::AssociateDistributionTenantWebAcl,
    Op::DisassociateDistributionTenantWebAcl,
    Op::CreateInvalidationForDistributionTenant,
    Op::GetInvalidationForDistributionTenant,
    Op::ListInvalidationsForDistributionTenant,
    Op::GetKeyGroup,
];

/// Call `op` with `id` as its path identifier, over the operation's own wire binding.
///
/// `version` is the `If-Match` the mutations run under.
async fn call_op(router: &MultiServiceRouter, op: Op, id: &str, version: &str) -> Answer {
    match op {
        Op::GetAnycastIpList => get_anycast_ip_list(router, id).await,
        Op::DeleteDistributionTenant => {
            call(
                router,
                "DELETE",
                &format!("/{API_VERSION}/distribution-tenant/{id}"),
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
                &format!("/{API_VERSION}/distribution-tenant/{id}"),
                Some(&body),
                Some(version),
            )
            .await
        }
        Op::AssociateDistributionTenantWebAcl => {
            associate(router, id, WEB_ACL_ARN, Some(version)).await
        }
        Op::DisassociateDistributionTenantWebAcl => disassociate(router, id, Some(version)).await,
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
                &format!("/{API_VERSION}/distribution-tenant/{id}/invalidation"),
                Some(&body),
                None,
            )
            .await
        }
        Op::GetInvalidationForDistributionTenant => {
            call(
                router,
                "GET",
                &format!("/{API_VERSION}/distribution-tenant/{id}/invalidation/{INVALIDATION_ID}"),
                None,
                None,
            )
            .await
        }
        Op::ListInvalidationsForDistributionTenant => {
            call(
                router,
                "GET",
                &format!("/{API_VERSION}/distribution-tenant/{id}/invalidation"),
                None,
                None,
            )
            .await
        }
        Op::GetKeyGroup => get_group(router, id).await,
    }
}

/// The identifiers of each kind that could be NO identifier of their resource at all:
/// `(case, value)`.
///
/// One row per form such a value takes — another namespace's id, the namespace label with no
/// separator at all, the right characters in the wrong case, the resource's own NAME, a bare
/// word, and a lone separator — so the rule is asserted over the class rather than over the
/// recorded token.
fn malformed(kind: Kind, name: &str, minted: &str) -> Vec<(String, String)> {
    let foreign = match kind {
        // A distribution id, whose namespace is `E` + 13 of the minted alphabet.
        Kind::AnycastIpList | Kind::KeyGroup => "EDFDVBD6EXAMPLE".to_string(),
        // A key group id, which is a UUID.
        Kind::DistributionTenant => "f9a6a0b4-8d39-4a1e-9f67-1b0f7f5d2c3e".to_string(),
    };
    let unseparated = match kind {
        // The namespace label with its SEPARATOR dropped: a namespaced id is a label, a
        // separator and a body, and the body's width and alphabet are no part of it (see
        // `well_formed_but_absent`), so what makes this one unreadable is that it carries no
        // namespace.
        Kind::AnycastIpList => "ipl0123456789abcdef0".to_string(),
        Kind::DistributionTenant => "dtNEVERMINTED12".to_string(),
        Kind::KeyGroup => "f9a6a0b4-8d39-4a1e-9f67-1b0f7f5d2c".to_string(),
    };
    vec![
        ("another resource kind's id".to_string(), foreign),
        (
            "the namespace label with no separator, or a UUID at the wrong length".to_string(),
            unseparated,
        ),
        ("the minted id in the wrong case".to_string(), flip_case(minted)),
        ("the resource's name".to_string(), name.to_string()),
        ("a bare word".to_string(), "nope".to_string()),
        ("a lone separator".to_string(), "-".to_string()),
    ]
}

/// The identifiers of each kind that COULD be one of its resource's identifiers but name no
/// stored resource: `(case, value)`.
///
/// For the namespaced kinds the rows vary the parts nothing in the model constrains — the
/// namespace label, the body's width and the body's alphabet — because none of them can
/// refuse an identifier: only the store knows which ids were issued.
fn well_formed_but_absent(kind: Kind) -> Vec<(String, String)> {
    match kind {
        Kind::AnycastIpList => vec![
            (
                "an id never minted".to_string(),
                "ipl-00000000000000000".to_string(),
            ),
            (
                "an id never minted at another width".to_string(),
                "ipl-0123456789abcdef".to_string(),
            ),
            (
                "an id never minted outside the minted alphabet".to_string(),
                "ipl-notaminted-id".to_string(),
            ),
            (
                "an id under another namespace label".to_string(),
                "aip_1A2B3C4D5E6F7G8H9J0K1L".to_string(),
            ),
            (
                "the same label under the other separator".to_string(),
                "ipl_0123456789abcdef0".to_string(),
            ),
        ],
        Kind::DistributionTenant => vec![
            (
                "an id never minted".to_string(),
                format!("{DISTRIBUTION_TENANT_ID_PREFIX}{}", "Z".repeat(27)),
            ),
            (
                "another id never minted".to_string(),
                format!("{DISTRIBUTION_TENANT_ID_PREFIX}{}", "0".repeat(27)),
            ),
        ],
        Kind::KeyGroup => vec![
            (
                "a UUID never minted".to_string(),
                "00000000-0000-0000-0000-000000000000".to_string(),
            ),
            (
                "another UUID never minted".to_string(),
                "f9a6a0b4-8d39-4a1e-9f67-1b0f7f5d2c3e".to_string(),
            ),
        ],
    }
}

/// `value` with every ASCII letter's case swapped — an id of the right characters and
/// length in the wrong case, which no namespace of this service mints.
fn flip_case(value: &str) -> String {
    value
        .chars()
        .map(|c| {
            if c.is_ascii_uppercase() {
                c.to_ascii_lowercase()
            } else {
                c.to_ascii_uppercase()
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// The resources that really exist
// ---------------------------------------------------------------------------

/// One kind's live resource: the id a create minted, the version token it handed back, and
/// the name the create claimed.
#[derive(Debug, Clone)]
struct Live {
    id: String,
    etag: String,
    name: String,
}

/// Seed one resource of each kind and report them by kind, in `OPS`' kind order.
async fn seed(router: &MultiServiceRouter) -> (Live, Live, Live) {
    // An Anycast static IP list.
    let list = Create {
        case: "the list the shape rule is asserted around",
        name: "path-id-shape-list".to_string(),
        ip_count: 3,
        ip_address_type: None,
        ipam_cidr_configs: Vec::new(),
        tags: Vec::new(),
    };
    let created = create_anycast_ip_list(router, &list.xml()).await;
    assert_eq!(
        created.status, 202,
        "case {}: expected HTTP 202 from CreateAnycastIpList, got {} with {}",
        list.case, created.status, created.raw
    );
    let anycast = Live {
        id: created.root().text_at("Id").unwrap_or_default().to_string(),
        etag: created.header("etag").to_string(),
        name: list.name.clone(),
    };

    // A distribution tenant, under the multi-tenant distribution one needs.
    let distribution_id = multi_tenant_distribution(router, "ref-path-id-shape").await;
    let (tenant_id, tenant_etag) =
        create_tenant(router, &distribution_id, "pathidshape", "the tenant", STARTS[0]).await;
    let tenant = Live {
        id: tenant_id,
        etag: tenant_etag,
        name: "pathidshape".to_string(),
    };

    // A key group, over public keys this service minted.
    let keys = mint_public_keys(router, "path-id-shape", 1).await;
    let answer = create_group(router, &key_group_config(&keys)).await;
    assert_eq!(
        answer.status, 201,
        "case the group the shape rule is asserted around: expected HTTP 201 from \
         CreateKeyGroup, got {} with {}",
        answer.status, answer.raw
    );
    let key_group = Live {
        id: answer.root().text_at("Id").unwrap_or_default().to_string(),
        etag: answer.header("etag").to_string(),
        name: KEY_GROUP_NAME.to_string(),
    };

    (anycast, tenant, key_group)
}

/// The name the live key group claims, so the "the resource's name" row of
/// [`malformed`] addresses it by a name rather than by an id.
const KEY_GROUP_NAME: &str = "path-id-shape-group";

/// The `KeyGroupConfig` payload the key group create submits: a group has to name at least
/// one public key.
fn key_group_config(keys: &[String]) -> String {
    Group {
        case: "the group the shape rule is asserted around".to_string(),
        name: KEY_GROUP_NAME.to_string(),
        items: keys.to_vec(),
        comment: None,
    }
    .xml()
}

// ---------------------------------------------------------------------------
// The shape rule
// ---------------------------------------------------------------------------

/// An identifier outside the shape its resource type's ids take is reported as an
/// unreadable argument, and a well-formed identifier the store holds nothing for is reported
/// as an absent resource — on every operation the recorded behaviour puts under the shape
/// rule, over every form each failure takes.
#[tokio::test]
async fn an_identifier_outside_its_resource_id_shape_is_an_argument_failure() {
    let router = emulator();
    let (anycast, tenant, key_group) = seed(&router).await;

    for op in OPS {
        let live = match op.kind() {
            Kind::AnycastIpList => &anycast,
            Kind::DistributionTenant => &tenant,
            Kind::KeyGroup => &key_group,
        };

        let (status, code, message) = op.unreadable_identifier();
        for (case, candidate) in malformed(op.kind(), &live.name, &live.id) {
            let answer = call_op(&router, *op, &candidate, &live.etag).await;
            assert_eq!(
                answer.status, status,
                "case {op:?}/{case} ({candidate:?}): expected HTTP {status}, got {} with {}",
                answer.status, answer.raw
            );
            assert_eq!(
                answer.error_code(),
                code,
                "case {op:?}/{case} ({candidate:?}): expected {code}, got {:?} with {}",
                answer.error_code(),
                answer.raw
            );
            let actual = answer.root().find_text("Message").unwrap_or_default();
            assert_eq!(
                actual, message,
                "case {op:?}/{case} ({candidate:?}): expected {message:?}, got {actual:?} in {}",
                answer.raw
            );
        }

        let (status, code, message) = op.absent_resource();
        for (case, candidate) in well_formed_but_absent(op.kind()) {
            let answer = call_op(&router, *op, &candidate, &live.etag).await;
            assert_eq!(
                answer.status, status,
                "case {op:?}/{case} ({candidate:?}): expected HTTP {status}, got {} with {}",
                answer.status, answer.raw
            );
            assert_eq!(
                answer.error_code(),
                code,
                "case {op:?}/{case} ({candidate:?}): expected {code}, got {:?} with {}",
                answer.error_code(),
                answer.raw
            );
            let actual = answer.root().find_text("Message").unwrap_or_default();
            assert_eq!(
                actual, message,
                "case {op:?}/{case} ({candidate:?}): expected {message:?}, got {actual:?} in {}",
                answer.raw
            );
        }
    }

    // The resources the creates really minted are still readable at their own versions: the
    // precheck refused only the identifiers that were never ids of their kind.
    let read = get_anycast_ip_list(&router, &anycast.id).await;
    assert_eq!(
        read.status, 200,
        "case the live Anycast static IP list: expected HTTP 200, got {} with {}",
        read.status, read.raw
    );
    let read = get_group(&router, &key_group.id).await;
    assert_eq!(
        read.status, 200,
        "case the live key group: expected HTTP 200, got {} with {}",
        read.status, read.raw
    );
    let read = call(
        &router,
        "GET",
        &format!("/{API_VERSION}/distribution-tenant/{}", tenant.id),
        None,
        None,
    )
    .await;
    assert_eq!(
        read.status, 200,
        "case the live distribution tenant: expected HTTP 200, got {} with {}",
        read.status, read.raw
    );
}

/// Each operation under the shape rule SERVES the resource that exists: addressed by the id
/// its create minted, at the version that create handed back, every one of them answers its
/// own success status. One emulator per operation, because several of them mutate or remove
/// the record they are given.
#[tokio::test]
async fn the_precheck_still_serves_the_resource_that_exists() {
    for op in OPS {
        let router = emulator();
        let (anycast, tenant, key_group) = seed(&router).await;
        let live = match op.kind() {
            Kind::AnycastIpList => &anycast,
            Kind::DistributionTenant => &tenant,
            Kind::KeyGroup => &key_group,
        };

        // `DeleteDistributionTenant` refuses a tenant that is still enabled, so the row
        // that deletes one disables it first — through the same update the table drives.
        if *op == Op::DeleteDistributionTenant {
            let disabled =
                call_op(&router, Op::UpdateDistributionTenant, &live.id, &live.etag).await;
            assert_eq!(
                disabled.status, 200,
                "case {op:?}: expected HTTP 200 disabling the tenant first, got {} with {}",
                disabled.status, disabled.raw
            );
            let version = disabled.header("etag").to_string();
            let answer = call_op(&router, *op, &live.id, &version).await;
            assert_eq!(
                answer.status,
                op.served_status(),
                "case {op:?}: expected HTTP {}, got {} with {}",
                op.served_status(),
                answer.status,
                answer.raw
            );
            continue;
        }

        let answer = call_op(&router, *op, &live.id, &live.etag).await;
        assert_eq!(
            answer.status,
            op.served_status(),
            "case {op:?}: expected HTTP {}, got {} with {}",
            op.served_status(),
            answer.status,
            answer.raw
        );
        // The one row whose success status is a refusal is the tenant-scoped invalidation
        // read: the TENANT resolved, and what refused the call is the invalidation id.
        if *op == Op::GetInvalidationForDistributionTenant {
            assert_eq!(
                answer.error_code(),
                "EntityNotFound",
                "case {op:?}: expected the refusal to be about the invalidation, got {:?} with {}",
                answer.error_code(),
                answer.raw
            );
        }
    }
}

// ---------------------------------------------------------------------------
// The If-Match rule
// ---------------------------------------------------------------------------

/// The two mutations whose `If-Match` is evaluated before the resource is reported on, with
/// the identifiers that reach that refusal: ids that are not even in the shape their
/// resource's ids take, so they address no resource for any version to belong to.
const IF_MATCH_FIRST: &[(&str, &[&str])] = &[
    (
        "DeleteContinuousDeploymentPolicy",
        &["not-a-policy-id", "00000000-0000-0000-0000-00000000000"],
    ),
    (
        "DisassociateDistributionWebACL",
        &["not-a-distribution", "EDFDVBD6EXAMPLE"],
    ),
];

/// Every form the `If-Match` header takes for the rows above: absent, three values that are
/// no version token at all, and two well-formed tokens — including one this service really
/// minted, for the resource that exists.
fn if_match_forms(minted: &str) -> Vec<(String, Option<String>)> {
    vec![
        ("no If-Match at all".to_string(), None),
        (
            "a value that is no version token".to_string(),
            Some("not-a-version".to_string()),
        ),
        ("an empty If-Match".to_string(), Some(String::new())),
        (
            "a token one character short".to_string(),
            Some("E1ABCDEFGHIJK".to_string()),
        ),
        (
            "a well-formed token never minted".to_string(),
            Some("E1Z2Y3X4W5V6U7".to_string()),
        ),
        (
            "another resource's current version".to_string(),
            Some(minted.to_string()),
        ),
    ]
}

/// Send the mutation `operation` names, for `id`, with the header `if_match`.
async fn call_conditional(
    router: &MultiServiceRouter,
    operation: &str,
    id: &str,
    if_match: Option<&str>,
) -> Answer {
    match operation {
        "DeleteContinuousDeploymentPolicy" => {
            call(
                router,
                "DELETE",
                &format!("/{API_VERSION}/continuous-deployment-policy/{id}"),
                None,
                if_match,
            )
            .await
        }
        _ => {
            call(
                router,
                "PUT",
                &format!("/{API_VERSION}/distribution/{id}/disassociate-web-acl"),
                None,
                if_match,
            )
            .await
        }
    }
}

/// A mutation that runs under an `If-Match` answers `InvalidIfMatchVersion` whatever the
/// header carries, for an id that addresses no resource of its kind: the header is evaluated
/// before the resource is reported on, and no version could belong to a resource the id
/// cannot address. An identifier of the right shape that the store holds nothing for IS
/// addressable, so that one is reported as the absent resource; and the same calls against
/// the resource that DOES exist, at its current version, are served.
#[tokio::test]
async fn a_conditional_mutation_decides_the_if_match_before_the_resource() {
    let router = emulator();

    // A continuous deployment policy and a distribution, each at the version its create
    // handed back — the "another resource's current version" row, and the served contrast.
    let table = staged(&router, "ifmfirst", policy_table()).await;
    let policy = &table[0];
    let created = create_policy(&router, &policy.xml()).await;
    assert_eq!(
        created.status, 201,
        "case {}: expected HTTP 201 from CreateContinuousDeploymentPolicy, got {} with {}",
        policy.case, created.status, created.raw
    );
    let policy_id = created.root().text_at("Id").unwrap_or_default().to_string();
    let policy_etag = created.header("etag").to_string();

    let (distribution_id, distribution_etag) = create_distribution_returning_id(
        &router,
        "the distribution the If-Match rule is asserted around",
        &config("if-match-first", "ref-if-match-first").xml(),
    )
    .await;

    for (operation, ids) in IF_MATCH_FIRST {
        let minted = if *operation == "DeleteContinuousDeploymentPolicy" {
            policy_etag.as_str()
        } else {
            distribution_etag.as_str()
        };
        for id in *ids {
            for (case, if_match) in if_match_forms(minted) {
                let answer = call_conditional(&router, operation, id, if_match.as_deref()).await;
                assert_eq!(
                    answer.status, 400,
                    "case {operation}/{id}/{case}: expected HTTP 400, got {} with {}",
                    answer.status, answer.raw
                );
                assert_eq!(
                    answer.error_code(),
                    "InvalidIfMatchVersion",
                    "case {operation}/{id}/{case}: expected InvalidIfMatchVersion, got {:?} \
                     with {}",
                    answer.error_code(),
                    answer.raw
                );
                let actual = answer.root().find_text("Message").unwrap_or_default();
                assert_eq!(
                    actual, INVALID_IF_MATCH_VERSION_MESSAGE,
                    "case {operation}/{id}/{case}: expected {INVALID_IF_MATCH_VERSION_MESSAGE:?}, \
                     got {actual:?} in {}",
                    answer.raw
                );
            }
        }
    }

    // The contrast on each operation: an identifier of the right shape that the store holds
    // nothing for IS addressable, so a usable header is answered on the absent resource
    // rather than on the header.
    for (operation, id, code) in [
        (
            "DeleteContinuousDeploymentPolicy",
            "00000000-0000-0000-0000-000000000000",
            "NoSuchContinuousDeploymentPolicy",
        ),
        (
            "DisassociateDistributionWebACL",
            "E1ABCDEFGHIJKL",
            "EntityNotFound",
        ),
    ] {
        let minted = if operation == "DeleteContinuousDeploymentPolicy" {
            policy_etag.as_str()
        } else {
            distribution_etag.as_str()
        };
        let addressable = call_conditional(&router, operation, id, Some(minted)).await;
        assert_eq!(
            addressable.status, 404,
            "case {operation}/an addressable id the store holds nothing for: expected HTTP 404, \
             got {} with {}",
            addressable.status, addressable.raw
        );
        assert_eq!(
            addressable.error_code(),
            code,
            "case {operation}/an addressable id the store holds nothing for: expected {code}, \
             got {:?} with {}",
            addressable.error_code(),
            addressable.raw
        );
    }

    // The contrast: each call lands on the resource that exists, at its current version.
    let served = call_conditional(
        &router,
        "DisassociateDistributionWebACL",
        &distribution_id,
        Some(&distribution_etag),
    )
    .await;
    assert_eq!(
        served.status, 200,
        "case DisassociateDistributionWebACL/the live distribution: expected HTTP 200, got {} \
         with {}",
        served.status, served.raw
    );
    let served = call_conditional(
        &router,
        "DeleteContinuousDeploymentPolicy",
        &policy_id,
        Some(&policy_etag),
    )
    .await;
    assert_eq!(
        served.status, 204,
        "case DeleteContinuousDeploymentPolicy/the live policy: expected HTTP 204, got {} with {}",
        served.status, served.raw
    );
}
