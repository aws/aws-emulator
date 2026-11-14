// SPDX-License-Identifier: CC0-1.0

//! The FORM of every identifier this service hands a caller back, end to end over the
//! wire.
//!
//! Every call goes through the real `MultiServiceRouter` the emulator boots (see
//! `crate::support`), and every id is read back through its own resource class's reader
//! before it counts, so nothing here passes against a handler that answers a canned
//! body.
//!
//! RULE: an identifier this service hands back for a resource class is in that class's
//! own id form — the fixed prefix the class is read by, then a body freshly generated for
//! that one resource — so the class can be read off the id, no two classes hand out ids
//! of one form, and a resource a caller creates is addressed out of the SAME namespace as
//! the ones the service itself ships for that class.
//!
//! The cases assert that rule over a TABLE whose rows span what it generalises over: the
//! resource CLASS, and within a class the request — the optional members it carries, the
//! verification method a domain names, and a request repeated byte for byte, none of
//! which the identity may depend on.
//!
//! `crate::minted_identifiers` owns the uniqueness and opacity of what the mint hands
//! out; this suite owns the form it is rendered in.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_securityagent_securityagent::state::{
    AGENT_SPACE_ID_PREFIX, MANAGEMENT_TYPE_AWS_MANAGED, PACK_ID_AWS_MANAGED_MARKER,
    PACK_ID_CLASS_PREFIX, PACK_ID_CUSTOMER_MANAGED_MARKER, SECURITY_REQUIREMENT_PACK_ID_PREFIX,
    TARGET_DOMAIN_ID_PREFIX, VERIFICATION_METHOD_DNS_TXT, VERIFICATION_METHOD_HTTP_ROUTE,
    VERIFICATION_METHOD_PRIVATE_VPC,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::support::{assert_uuid_body, call, emulator, member, ok};

/// One resource class of this service whose identifier form it hands out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Class {
    AgentSpace,
    Pack,
    TargetDomain,
}

impl Class {
    /// The prefix every identifier of this class carries, as the service's own state
    /// module states it — one place, so a handler and a case cannot disagree about the
    /// form.
    fn prefix(self) -> &'static str {
        match self {
            Self::AgentSpace => AGENT_SPACE_ID_PREFIX,
            Self::Pack => SECURITY_REQUIREMENT_PACK_ID_PREFIX,
            Self::TargetDomain => TARGET_DOMAIN_ID_PREFIX,
        }
    }

    /// The response member the class's create answers its new id in.
    fn id_member(self) -> &'static str {
        match self {
            Self::AgentSpace => "agentSpaceId",
            Self::Pack => "packId",
            Self::TargetDomain => "targetDomainId",
        }
    }
}

/// Every class in the table, so a case can assert that an id of one class carries no
/// other class's prefix.
const CLASSES: [Class; 3] = [Class::AgentSpace, Class::Pack, Class::TargetDomain];

/// A string member a response must carry.
fn required(payload: &Value, name: &str, case: &str) -> String {
    member(payload, name)
        .unwrap_or_else(|| panic!("case {case:?}: the response carries no {name}: {payload}"))
}

/// Create one resource of `class` with `request` and answer the id it was minted,
/// having first read that resource back under that id through the class's own reader.
///
/// The read is what makes the id the STORE's and not a serializer's: a form a create
/// invented but nothing persisted under would resolve nothing here.
async fn mint_and_read_back(
    router: &MultiServiceRouter,
    class: Class,
    request: &Value,
    case: &str,
) -> String {
    match class {
        Class::AgentSpace => {
            let created = ok(router, "CreateAgentSpace", request, case).await;
            let id = required(&created, class.id_member(), case);
            let read = ok(
                router,
                "BatchGetAgentSpaces",
                &json!({"agentSpaceIds": [&id]}),
                case,
            )
            .await;
            let records = read
                .get("agentSpaces")
                .and_then(Value::as_array)
                .unwrap_or_else(|| panic!("case {case:?}: the read carries no agentSpaces: {read}"));
            assert_eq!(
                records.len(),
                1,
                "case {case:?}: expected the one agent space just created, got {read}"
            );
            assert_eq!(
                member(&records[0], class.id_member()).as_deref(),
                Some(id.as_str()),
                "case {case:?}: the agent space must read back under the minted id \
                 {id:?}: {read}"
            );
            id
        }
        Class::Pack => {
            // The pack create answers the modelled 201, so it is driven through `call`
            // rather than the 200-expecting helper.
            let answer = call(router, "CreateSecurityRequirementPack", request).await;
            assert_eq!(
                answer.status, 201,
                "case {case:?}: expected CreateSecurityRequirementPack to answer 201, got \
                 {} ({}) with {}",
                answer.status, answer.error_code, answer.body
            );
            let id = required(&answer.body, class.id_member(), case);
            let read = ok(
                router,
                "GetSecurityRequirementPack",
                &json!({"packId": &id}),
                case,
            )
            .await;
            assert_eq!(
                required(&read, class.id_member(), case),
                id,
                "case {case:?}: the pack must read back under the minted id {id:?}: {read}"
            );
            id
        }
        Class::TargetDomain => {
            let created = ok(router, "CreateTargetDomain", request, case).await;
            let id = required(&created, class.id_member(), case);
            let read = ok(
                router,
                "BatchGetTargetDomains",
                &json!({"targetDomainIds": [&id]}),
                case,
            )
            .await;
            let records = read
                .get("targetDomains")
                .and_then(Value::as_array)
                .unwrap_or_else(|| {
                    panic!("case {case:?}: the read carries no targetDomains: {read}")
                });
            assert_eq!(
                records.len(),
                1,
                "case {case:?}: expected the one target domain just registered, got {read}"
            );
            assert_eq!(
                member(&records[0], class.id_member()).as_deref(),
                Some(id.as_str()),
                "case {case:?}: the domain must read back under the minted id {id:?}: \
                 {read}"
            );
            id
        }
    }
}

/// The table: for each class, the creates whose requests differ along everything the
/// identity may NOT depend on — which optional members the request carries, the
/// verification method of a domain registration
/// (`smithy_query(query="shape",shape="DomainVerificationMethod")`: `DNS_TXT`,
/// `HTTP_ROUTE`, `PRIVATE_VPC`), and the same request a second time.
///
/// Each name and domain carries letters a hexadecimal body cannot (`z`, `x`, `w`), so a
/// form assertion below also witnesses that no request text reaches the id.
fn rows() -> Vec<(&'static str, Class, Value)> {
    vec![
        (
            "an agent space, the required name alone",
            Class::AgentSpace,
            json!({"name": "zwx-space-one"}),
        ),
        (
            "an agent space carrying a description",
            Class::AgentSpace,
            json!({"name": "zwx-space-two", "description": "the second space"}),
        ),
        (
            "an agent space created again with the first name",
            Class::AgentSpace,
            json!({"name": "zwx-space-one"}),
        ),
        (
            "a pack, the required name alone",
            Class::Pack,
            json!({"name": "zwx-pack-one"}),
        ),
        (
            "a pack carrying a description and tags",
            Class::Pack,
            json!({
                "name": "zwx-pack-two",
                "description": "the second pack",
                "tags": {"owner": "zwx-team"},
            }),
        ),
        (
            "a pack created again with the first name",
            Class::Pack,
            json!({"name": "zwx-pack-one"}),
        ),
        (
            "a DNS_TXT target domain",
            Class::TargetDomain,
            json!({
                "targetDomainName": "zwx-one.example.com",
                "verificationMethod": VERIFICATION_METHOD_DNS_TXT,
            }),
        ),
        (
            "an HTTP_ROUTE target domain",
            Class::TargetDomain,
            json!({
                "targetDomainName": "zwx-two.example.com",
                "verificationMethod": VERIFICATION_METHOD_HTTP_ROUTE,
            }),
        ),
        (
            "a PRIVATE_VPC target domain",
            Class::TargetDomain,
            json!({
                "targetDomainName": "zwx-three.example.com",
                "verificationMethod": VERIFICATION_METHOD_PRIVATE_VPC,
            }),
        ),
        (
            "the first domain name registered a second time",
            Class::TargetDomain,
            json!({
                "targetDomainName": "zwx-one.example.com",
                "verificationMethod": VERIFICATION_METHOD_DNS_TXT,
            }),
        ),
    ]
}

/// `id` is in `class`'s own form: that class's prefix, then a body of the `8-4-4-4-12`
/// hyphenated lowercase-hex groups a freshly generated one carries (the shared
/// `crate::support::assert_uuid_body`). Answers the body, so a case can assert it is this
/// resource's own.
fn assert_class_form(id: &str, class: Class, case: &str) -> String {
    let member = class.id_member();
    let body = id.strip_prefix(class.prefix()).unwrap_or_else(|| {
        panic!(
            "case {case:?}: expected {member} to start {:?}, got {id:?}",
            class.prefix()
        )
    });
    assert_uuid_body(body, case, member);
    body.to_string()
}

/// RULE: every id a create hands back is in its own class's form, so the class is
/// readable off the id and an id of one class carries no other class's prefix — over
/// every row of the table.
#[tokio::test]
async fn every_created_id_is_in_its_own_class_form() {
    let router = emulator();
    let mut bodies: BTreeSet<String> = BTreeSet::new();

    for (case, class, request) in rows() {
        let id = mint_and_read_back(&router, class, &request, case).await;
        let body = assert_class_form(&id, class, case);

        // The body is this ONE resource's: a second create — including the same request
        // byte for byte — is a second resource, so no two rows share a body.
        assert!(
            bodies.insert(body.clone()),
            "case {case:?}: the body {body:?} was already handed to another resource, so \
             the id is not freshly generated per resource; the bodies so far are \
             {bodies:?}"
        );

        // One prefix per class: an id of this class must not be readable as an id of
        // another, which is what a client routing or validating on the prefix relies on.
        for other in CLASSES {
            if other == class {
                continue;
            }
            assert!(
                !id.starts_with(other.prefix()),
                "case {case:?}: the {} {id:?} also carries the {} prefix {:?}, so the two \
                 classes cannot be told apart",
                class.id_member(),
                other.id_member(),
                other.prefix()
            );
        }
    }
}

/// RULE, applied to the one class whose ids the service ALSO ships: every pack id is of
/// the one pack-id class — the class prefix, the marker of the management type the pack
/// has, then that pack's own body — so a client that handles the shipped catalog's ids
/// handles a created pack's unchanged, out of ONE namespace rather than two.
#[tokio::test]
async fn every_pack_id_is_of_the_one_pack_class() {
    let router = emulator();

    // The prefix a created pack carries is the class prefix and the customer-managed
    // marker, so a case cannot assert a form the two halves do not spell.
    assert_eq!(
        Class::Pack.prefix(),
        format!("{PACK_ID_CLASS_PREFIX}{PACK_ID_CUSTOMER_MANAGED_MARKER}"),
        "the created-pack prefix must be the class prefix and the customer-managed marker"
    );

    // The catalog as the wire reports it, not as a table in this file: whatever a fresh
    // account ships is what a created pack has to agree with.
    let listed = ok(
        &router,
        "ListSecurityRequirementPacks",
        &json!({}),
        "the shipped catalog",
    )
    .await;
    let catalog: Vec<String> = listed
        .get("securityRequirementPackSummaries")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("the listing carries no summaries: {listed}"))
        .iter()
        .filter(|summary| {
            member(summary, "managementType").as_deref() == Some(MANAGEMENT_TYPE_AWS_MANAGED)
        })
        .map(|summary| {
            member(summary, "packId")
                .unwrap_or_else(|| panic!("a summary carries no packId: {summary}"))
        })
        .collect();
    assert!(
        !catalog.is_empty(),
        "a fresh account ships the AWS managed catalog, got {listed}"
    );
    let shipped_prefix = format!("{PACK_ID_CLASS_PREFIX}{PACK_ID_AWS_MANAGED_MARKER}");
    for shipped in &catalog {
        assert!(
            shipped.starts_with(&shipped_prefix),
            "the shipped pack id {shipped:?} must carry the class prefix and the \
             AWS-managed marker {shipped_prefix:?}"
        );
    }

    // Two created packs, so the case is about the class's form and not one draw.
    for case in ["the first created pack", "the second created pack"] {
        let id = mint_and_read_back(
            &router,
            Class::Pack,
            &json!({"name": format!("zwx-{}", case.replace(' ', "-"))}),
            case,
        )
        .await;
        assert_class_form(&id, Class::Pack, case);
        assert!(
            id.starts_with(PACK_ID_CLASS_PREFIX),
            "case {case:?}: the created pack id {id:?} must carry the one pack class \
             prefix {PACK_ID_CLASS_PREFIX:?}, as the shipped ones do"
        );
        assert!(
            !id.starts_with(&shipped_prefix),
            "case {case:?}: the created pack id {id:?} is customer managed, so it must not \
             carry the AWS-managed marker"
        );
        assert!(
            !catalog.contains(&id),
            "case {case:?}: the created pack id {id:?} is one the catalog already ships"
        );
    }
}
