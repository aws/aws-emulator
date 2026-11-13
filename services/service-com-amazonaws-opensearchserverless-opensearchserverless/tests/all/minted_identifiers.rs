// SPDX-License-Identifier: CC0-1.0

//! The values this service MINTS: the `CollectionId` and `CollectionGroupId` a create
//! hands out, and the `PolicyVersion` / `configVersion` every policy and
//! security-configuration write hands out.
//!
//! Every case drives the real request path (`MultiServiceRouter::dispatch` with `POST /`,
//! `X-Amz-Target: OpenSearchServerless.<Operation>` and an `awsJson1_0` body) through the
//! harness this crate's `collection_lifecycle` suite owns, so the boundary validator, the
//! protocol codec and the handler all take part exactly as they do for an SDK client.
//!
//! The rule under test: a value the service mints is DRAWN afresh for each resource and for
//! each write, over the character class AWS's own values of that member are drawn from — so it
//! matches the member's modelled pattern, no two minted values collide (across resource kinds,
//! and across stores), and none carries structure a caller can read out of it: no filler run,
//! no counter, no write instant, nothing the request supplied. The WIDTH and the alphabet each
//! minted value carries are the sibling `minted_identifier_form` suite's subject.
//!
//! The tables span what the rule generalises over: both resources whose id is
//! `^[a-z0-9]{3,40}$`, every `CollectionType`, several Regions, both policy families with
//! all three policy `type` values, the lifecycle policy and the security configuration, and
//! creates beside updates.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The epoch-millisecond clock the service's own timestamps are stamped from, so a write's
// instant is bracketed by the same reading the handler takes.
use service_com_amazonaws_opensearchserverless_opensearchserverless::state::now_epoch_millis;

use crate::collection_group_lifecycle::create_group;
use crate::collection_lifecycle::{
    at, call, call_in, emulator, is_collection_id, is_policy_version, text,
};

/// Every `CollectionType` member, from the model's own value list
/// (`smithy_query(query="shape", shape="CollectionType")` -> `values`).
pub(crate) const COLLECTION_TYPES: [&str; 3] = ["SEARCH", "TIMESERIES", "VECTORSEARCH"];

/// The characters a hex rendering is built from. A minted token drawn over the alphanumeric
/// class AWS's own version tokens use reaches outside this set, which is what tells a drawn
/// token from a hex-rendered counter or clock.
const HEX_CHARACTERS: &str = "0123456789abcdefABCDEF";

/// An IdP metadata document, so the security-configuration rows have something to carry.
/// `samlMetadata` is printable text
/// (`@pattern("[\u{0009}\u{000A}\u{000D}\u{0020}-\u{007E}\u{00A1}-\u{00FF}]+")`).
pub(crate) const SAML_METADATA: &str =
    r#"<EntityDescriptor entityID="https://minted.example.com/metadata"></EntityDescriptor>"#;

/// One family of version-bearing resource: the operations that create, write and read one,
/// the member its detail is reported under and the member carrying the minted version.
pub(crate) struct Family {
    pub(crate) kind: &'static str,
    pub(crate) create: &'static str,
    /// The operation that writes one.
    pub(crate) update: Option<&'static str>,
    pub(crate) detail: &'static str,
    pub(crate) version_member: &'static str,
}

/// Every resource of this service whose write mints a version token: both policy families
/// with all three policy `type` values (`smithy_query(query="shape",
/// shape="AccessPolicyType")` -> `["data"]`, `shape="SecurityPolicyType"` ->
/// `["encryption", "network"]`), the lifecycle policy
/// (`shape="LifecyclePolicyType"` -> `["retention"]`) and the security configuration, whose
/// `configVersion` is a `PolicyVersion` too.
pub(crate) const FAMILIES: [Family; 5] = [
    Family {
        kind: "data",
        create: "CreateAccessPolicy",
        update: Some("UpdateAccessPolicy"),
        detail: "accessPolicyDetail",
        version_member: "policyVersion",
    },
    Family {
        kind: "encryption",
        create: "CreateSecurityPolicy",
        update: Some("UpdateSecurityPolicy"),
        detail: "securityPolicyDetail",
        version_member: "policyVersion",
    },
    Family {
        kind: "network",
        create: "CreateSecurityPolicy",
        update: Some("UpdateSecurityPolicy"),
        detail: "securityPolicyDetail",
        version_member: "policyVersion",
    },
    Family {
        kind: "retention",
        create: "CreateLifecyclePolicy",
        update: Some("UpdateLifecyclePolicy"),
        detail: "lifecyclePolicyDetail",
        version_member: "policyVersion",
    },
    Family {
        kind: "saml",
        create: "CreateSecurityConfig",
        update: Some("UpdateSecurityConfig"),
        detail: "securityConfigDetail",
        version_member: "configVersion",
    },
];

/// The document a policy of `kind` carries, in the form that type's rules take.
pub(crate) fn policy_document(kind: &str) -> String {
    match kind {
        "data" => r#"{"Rules":[{"ResourceType":"collection","Resource":["collection/demo"],"Permission":["aoss:CreateCollectionItems"]}],"Principal":["arn:aws:iam::000000000000:role/Writer"]}"#.to_string(),
        "encryption" => r#"{"Rules":[{"ResourceType":"collection","Resource":["collection/demo"]}],"AWSOwnedKey":true}"#.to_string(),
        "network" => r#"{"Rules":[{"ResourceType":"collection","Resource":["collection/demo"]}],"AllowFromPublic":true}"#.to_string(),
        _ => r#"{"Rules":[{"ResourceType":"index","Resource":["index/demo/*"],"MinIndexRetention":"24h"}]}"#.to_string(),
    }
}

/// Call `operation` with `body`, asserting it was accepted, and answer the member of the
/// response named by `detail`.
pub(crate) async fn accepted(
    router: &MultiServiceRouter,
    operation: &str,
    detail: &str,
    body: Value,
) -> Value {
    let case = body.to_string();
    let (status, code, answer) = call(router, operation, body).await;
    assert_eq!(
        status, 200,
        "case {operation}({case}): expected 200, got {status} {code} with {answer}"
    );
    at(&answer, detail).clone()
}

/// Create the collection `name` of `kind` in `region` with the AWS-owned key, asserting the
/// create was accepted, and answer its `createCollectionDetail`.
pub(crate) async fn create_collection(
    router: &MultiServiceRouter,
    region: &str,
    name: &str,
    kind: &str,
) -> Value {
    let body = json!({
        "name": name,
        "type": kind,
        "encryptionConfig": { "aWSOwnedKey": true },
    });
    let (status, code, answer) = call_in(router, region, "CreateCollection", body).await;
    assert_eq!(
        status, 200,
        "case CreateCollection({name}/{kind} in {region}): expected 200, got {status} {code} with {answer}"
    );
    at(&answer, "createCollectionDetail").clone()
}

/// Create the resource `family` describes under `name`, answering its detail.
pub(crate) async fn create_versioned(router: &MultiServiceRouter, family: &Family, name: &str) -> Value {
    let request = if family.create == "CreateSecurityConfig" {
        json!({
            "type": family.kind,
            "name": name,
            "samlOptions": { "metadata": SAML_METADATA },
        })
    } else {
        json!({
            "type": family.kind,
            "name": name,
            "policy": policy_document(family.kind),
        })
    };
    accepted(router, family.create, family.detail, request).await
}

/// Write the resource `family` describes, addressed at `version` (its optimistic-concurrency
/// precondition), answering its detail.
pub(crate) async fn update_versioned(
    router: &MultiServiceRouter,
    family: &Family,
    update: &str,
    name: &str,
    id: &str,
    version: &str,
) -> Value {
    let request = if update == "UpdateSecurityConfig" {
        json!({ "id": id, "configVersion": version, "description": "written" })
    } else {
        json!({
            "type": family.kind,
            "name": name,
            "policyVersion": version,
            "description": "written",
        })
    };
    accepted(router, update, family.detail, request).await
}

/// How many distinct characters `value` is built from.
fn distinct_characters(value: &str) -> usize {
    value.chars().collect::<BTreeSet<char>>().len()
}

/// The highest number of times any one character occurs in `value`.
fn longest_repeat(value: &str) -> usize {
    let mut counts: BTreeMap<char, usize> = BTreeMap::new();
    for character in value.chars() {
        *counts.entry(character).or_default() += 1;
    }
    counts.into_values().max().unwrap_or_default()
}

/// Assert `id` is a drawn identifier and not a filled-in placeholder: it satisfies the
/// modelled pattern, it ranges over the alphabet rather than running on one character, and
/// nothing in it is the resource's name.
fn assert_drawn_id(case: &str, id: &str, name: &str) {
    assert!(
        is_collection_id(id),
        "case {case}: the id must satisfy CollectionId's ^[a-z0-9]{{3,40}}$, got {id:?}"
    );
    // A drawn id of this length ranges over the whole 36-character alphabet, so a third of
    // its positions being distinct is a floor no draw realistically misses, while a
    // placeholder padded with one filler character (as a radix conversion that ran out of
    // value produces) sits far below it.
    let floor = id.len() / 3;
    let distinct = distinct_characters(id);
    assert!(
        distinct >= floor,
        "case {case}: a drawn id of {} characters must use at least {floor} distinct ones, got {distinct} in {id:?}",
        id.len()
    );
    let repeat = longest_repeat(id);
    assert!(
        repeat <= id.len() / 2,
        "case {case}: no character may fill half a drawn id, got one repeated {repeat} times in {id:?}"
    );
    assert!(
        !id.contains(name),
        "case {case}: the id must not carry the requested name {name:?}, got {id:?}"
    );
}

/// Assert `version` is a drawn version token: it satisfies the modelled shape and carries
/// neither the write instant (`now` is bracketed by `before` and `after`) nor a count of
/// the writes before it.
fn assert_drawn_version(case: &str, version: &str, before: i64, after: i64) {
    assert!(
        is_policy_version(version),
        "case {case}: the version must satisfy PolicyVersion's length and pattern, got {version:?}"
    );
    let distinct = distinct_characters(version);
    let floor = version.len() / 4;
    assert!(
        distinct >= floor,
        "case {case}: a drawn version of {} characters must use at least {floor} distinct ones, got {distinct} in {version:?}",
        version.len()
    );
    // An opaque token tells the caller nothing about WHEN it was minted: every instant the
    // write could have happened at is checked in both the renderings a timestamp reaches a
    // token through.
    for instant in before..=after {
        for rendering in [format!("{instant}"), format!("{instant:x}")] {
            assert!(
                !version.contains(&rendering),
                "case {case}: the version must not carry the write instant {instant} ({rendering}), got {version:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// The minted id: one per resource, drawn, and the key the resource is stored under.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn every_created_resource_carries_its_own_drawn_id() {
    let router = emulator();
    let mut ids: BTreeSet<String> = BTreeSet::new();

    // Each row: the Region the resource is created in, and the resource kind — the two
    // resources of this service whose id carries `^[a-z0-9]{3,40}$`, over every
    // `CollectionType` and four Regions.
    let regions = ["us-east-1", "us-west-2", "eu-west-2", "ap-southeast-2"];
    for (index, region) in regions.iter().enumerate() {
        for kind in COLLECTION_TYPES {
            let name = format!("minted-{}-{index}", kind.to_lowercase());
            let case = format!("CreateCollection({name}/{kind} in {region})");
            let detail = create_collection(&router, region, &name, kind).await;
            let id = text(&detail, "id");
            assert_drawn_id(&case, &id, &name);
            assert!(
                ids.insert(id.clone()),
                "case {case}: every resource is minted its OWN id, got a repeat of {id:?}"
            );

            // The id is the key the collection is stored under and the ARN is derived from
            // it — so the read that follows reports the same one.
            let read = call_in(
                &router,
                region,
                "BatchGetCollection",
                json!({ "ids": [id.clone()] }),
            )
            .await;
            let stored = at(&read.2, "collectionDetails")
                .as_array()
                .and_then(|details| details.first())
                .cloned()
                .unwrap_or(Value::Null);
            assert_eq!(
                text(&stored, "id"),
                id,
                "case {case}: the read must answer the collection the minted id names, got {stored}"
            );
            assert_eq!(
                text(&stored, "name"),
                name,
                "case {case}: the minted id must name the collection that was created, got {stored}"
            );
            // The ARN is derived from the minted id. The ENDPOINTS are not: each names the
            // address this emulator serves that collection at (its own listener,
            // `crate::state::collection_plane`), which is what a client dials — asserted by
            // the `announced_collection_endpoints` suite.
            let arn = text(&stored, "arn");
            assert!(
                arn.contains(&id),
                "case {case}: arn is derived from the minted id {id:?}, got {arn:?}"
            );
        }

        // A collection GROUP's id carries the same shape and comes out of the same mint, so
        // it can never be the id of a collection beside it.
        let group_name = format!("minted-group-{index}");
        let case = format!("CreateCollectionGroup({group_name} in {region})");
        let group = create_group(
            &router,
            region,
            json!({ "name": group_name, "standbyReplicas": "ENABLED" }),
        )
        .await;
        let group_id = text(&group, "id");
        assert_drawn_id(&case, &group_id, &group_name);
        assert!(
            ids.insert(group_id.clone()),
            "case {case}: a collection group's id is its own, got a repeat of {group_id:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// The same request in two stores is minted two different ids: the id is drawn, not derived
// from the request and not a fixed value the first resource of a store always takes.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn two_stores_mint_different_ids_for_the_same_request() {
    let first = emulator();
    let second = emulator();

    let collection_name = "twin-collection";
    let group_name = "twin-group";

    let mut collection_ids: Vec<String> = Vec::new();
    let mut group_ids: Vec<String> = Vec::new();
    for router in [&first, &second] {
        let detail =
            create_collection(router, "us-east-1", collection_name, COLLECTION_TYPES[1]).await;
        collection_ids.push(text(&detail, "id"));
        let group = create_group(
            router,
            "us-east-1",
            json!({ "name": group_name, "standbyReplicas": "DISABLED" }),
        )
        .await;
        group_ids.push(text(&group, "id"));
    }

    assert_ne!(
        collection_ids[0], collection_ids[1],
        "case CreateCollection({collection_name}) in two stores: the id is drawn per resource, not derived from the request, got {collection_ids:?}"
    );
    assert_ne!(
        group_ids[0], group_ids[1],
        "case CreateCollectionGroup({group_name}) in two stores: the id is drawn per resource, not derived from the request, got {group_ids:?}"
    );
}

// ---------------------------------------------------------------------------
// The minted version token: one per write, drawn over the pattern's whole class, opaque,
// and the precondition the next write is addressed at.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn every_write_mints_its_own_opaque_version() {
    let router = emulator();
    let mut versions: BTreeSet<String> = BTreeSet::new();
    let mut seen_characters: BTreeSet<char> = BTreeSet::new();

    for family in &FAMILIES {
        // `PolicyName` / `ConfigName` is @length(min: 3, max: 32) with `^[a-z][a-z0-9-]+$`.
        let name = format!("minted-{}", family.kind);

        let before = now_epoch_millis();
        let created = create_versioned(&router, family, &name).await;
        let after = now_epoch_millis();
        let case = format!("{}({}/{name})", family.create, family.kind);
        let created_version = text(&created, family.version_member);
        assert_drawn_version(&case, &created_version, before, after);
        assert!(
            versions.insert(created_version.clone()),
            "case {case}: every write mints its OWN version, got a repeat of {created_version:?}"
        );
        seen_characters.extend(created_version.chars());

        // The write is addressed at the version the create minted, and mints another one.
        let Some(update) = family.update else {
            continue;
        };
        let id = text(&created, "id");
        let before = now_epoch_millis();
        let written = update_versioned(&router, family, update, &name, &id, &created_version).await;
        let after = now_epoch_millis();
        let case = format!("{update}({}/{name})", family.kind);
        let written_version = text(&written, family.version_member);
        assert_drawn_version(&case, &written_version, before, after);
        assert!(
            versions.insert(written_version.clone()),
            "case {case}: a write mints a version no earlier write minted, got a repeat of {written_version:?}"
        );
        seen_characters.extend(written_version.chars());
    }

    // Drawn over `PolicyVersion`'s own character class, so the tokens as a whole reach
    // outside the 16 characters a hex rendering of a counter or a clock is confined to.
    let outside_hex: Vec<char> = seen_characters
        .iter()
        .copied()
        .filter(|character| !HEX_CHARACTERS.contains(*character))
        .collect();
    assert!(
        !outside_hex.is_empty(),
        "case every_write_mints_its_own_opaque_version: the minted tokens must be drawn over PolicyVersion's whole class, got only hex characters {seen_characters:?}"
    );
}

// ---------------------------------------------------------------------------
// A minted version is STABLE: a read reports the version the write stored, and reading
// twice reports it again.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_minted_version_is_what_every_later_read_reports() {
    let router = emulator();

    for family in &FAMILIES {
        let name = format!("stable-{}", family.kind);
        let created = create_versioned(&router, family, &name).await;
        let version = text(&created, family.version_member);
        let id = text(&created, "id");

        let (read_operation, request) = match family.create {
            "CreateAccessPolicy" => (
                "GetAccessPolicy",
                json!({ "type": family.kind, "name": name }),
            ),
            "CreateSecurityPolicy" => (
                "GetSecurityPolicy",
                json!({ "type": family.kind, "name": name }),
            ),
            "CreateLifecyclePolicy" => (
                "BatchGetLifecyclePolicy",
                json!({ "identifiers": [{ "type": family.kind, "name": name }] }),
            ),
            _ => ("GetSecurityConfig", json!({ "id": id })),
        };

        for attempt in 1..=2 {
            let case = format!("{read_operation}({}/{name}), read {attempt}", family.kind);
            let (status, code, answer) = call(&router, read_operation, request.clone()).await;
            assert_eq!(
                status, 200,
                "case {case}: expected 200, got {status} {code} with {answer}"
            );
            let reported = if read_operation == "BatchGetLifecyclePolicy" {
                at(&answer, "lifecyclePolicyDetails")
                    .as_array()
                    .and_then(|details| details.first())
                    .cloned()
                    .unwrap_or(Value::Null)
            } else {
                at(&answer, family.detail).clone()
            };
            assert_eq!(
                text(&reported, family.version_member),
                version,
                "case {case}: a read reports the version the write minted, not a fresh one, got {reported}"
            );
        }
    }
}
