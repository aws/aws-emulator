// SPDX-License-Identifier: CC0-1.0

//! The FORM of the values this service MINTS: the `CollectionId` / `CollectionGroupId` a
//! create hands out, and the `PolicyVersion` / `configVersion` every policy and
//! security-configuration write hands out.
//!
//! Every case drives the real request path (`MultiServiceRouter::dispatch` with `POST /`,
//! `X-Amz-Target: OpenSearchServerless.<Operation>` and an `awsJson1_0` body) through the
//! harness this crate's `collection_lifecycle` suite owns, so the boundary validator, the
//! protocol codec and the handler all take part exactly as they do for an SDK client. The
//! resource tables are the sibling `minted_identifiers` suite's, shared rather than copied:
//! that suite asserts a minted value is DRAWN and unique, this one asserts the FORM it is
//! drawn in.
//!
//! The rule under test: a minted value carries the ONE form AWS mints that value in, which is
//! narrower than the modelled pattern — a collection or collection-group id is exactly
//! [`ID_CHARACTERS`] characters of `[a-z0-9]`, and a version token is exactly
//! [`VERSION_CHARACTERS`] characters of `[0-9a-zA-Z]`, so no token carries the `+`, `/` or `=`
//! its base64-shaped pattern would also admit and every one of them travels in a path, a URL
//! and a configuration file unescaped.
//!
//! Where the two widths come from: `CollectionId` is `@length(min: 3, max: 40)` with
//! `@pattern("^[a-z0-9]{3,40}$")` and `PolicyVersion` is `@length(min: 20, max: 36)` with
//! `@pattern("^([0-9a-zA-Z+/]{4})*(([0-9a-zA-Z+/]{2}==)|([0-9a-zA-Z+/]{3}=))?$")`
//! (`smithy_query(query="shape", shape="CollectionId,PolicyVersion")`), and each bound admits
//! several widths — the one AWS mints is the recorded response's, which is what
//! `crate::state`'s own `ID_LEN` and `POLICY_VERSION_LEN` record.
//!
//! The tables span what the rule generalises over: both resources whose id carries that
//! shape, every `CollectionType`, several Regions, both policy families with all three policy
//! `type` values, the lifecycle policy and the security configuration, and creates beside
//! updates.

use serde_json::{json, Value};

use crate::collection_group_lifecycle::create_group;
use crate::collection_lifecycle::{
    at, call_in, emulator, is_collection_id, is_policy_version, text,
};
use crate::minted_identifiers::{
    create_collection, create_versioned, update_versioned, COLLECTION_TYPES, FAMILIES,
};

/// How many characters a minted collection or collection-group id carries (see the module
/// docs for where the width comes from).
const ID_CHARACTERS: usize = 20;

/// How many characters a minted version token carries (see the module docs).
const VERSION_CHARACTERS: usize = 20;

/// The characters `PolicyVersion`'s pattern admits that AWS's own tokens never carry: the
/// two non-alphanumeric base64 characters and the padding character.
const NON_ALPHANUMERIC_BASE64: [char; 3] = ['+', '/', '='];

/// Assert `id` has the form of an id AWS mints: the modelled pattern, at AWS's own width.
fn assert_id_form(case: &str, id: &str) {
    assert!(
        is_collection_id(id),
        "case {case}: the id must satisfy CollectionId's ^[a-z0-9]{{3,40}}$, got {id:?}"
    );
    assert_eq!(
        id.chars().count(),
        ID_CHARACTERS,
        "case {case}: a minted id carries {ID_CHARACTERS} characters, got {} in {id:?}",
        id.chars().count()
    );
}

/// Assert `version` has the form of a version token AWS mints: the modelled shape, at AWS's
/// own width, and drawn from the alphanumeric half of the class its pattern admits — so it
/// is safe in a path.
fn assert_version_form(case: &str, version: &str) {
    assert!(
        is_policy_version(version),
        "case {case}: the version must satisfy PolicyVersion's length and pattern, got \
         {version:?}"
    );
    assert_eq!(
        version.chars().count(),
        VERSION_CHARACTERS,
        "case {case}: a minted version carries {VERSION_CHARACTERS} characters, got {} in \
         {version:?}",
        version.chars().count()
    );
    for character in NON_ALPHANUMERIC_BASE64 {
        assert!(
            !version.contains(character),
            "case {case}: a minted version is alphanumeric and never carries {character:?}, got \
             {version:?}"
        );
    }
    assert!(
        version.chars().all(|character| character.is_ascii_alphanumeric()),
        "case {case}: a minted version is drawn from [0-9a-zA-Z], got {version:?}"
    );
    // The token travels in a URI path unescaped, which is the property the alphabet buys:
    // percent-encoding it changes nothing.
    assert_eq!(
        version
            .chars()
            .filter(|character| matches!(character, '/' | '?' | '#' | '%' | '&' | '=' | '+'))
            .count(),
        0,
        "case {case}: a minted version carries no character a path or query would escape, got \
         {version:?}"
    );
}

#[tokio::test]
async fn every_minted_id_has_the_form_aws_mints() {
    let router = emulator();

    // Each row: the Region the resource is created in, and the collection type — every
    // `CollectionType` over four Regions, plus the collection GROUP beside them, whose id
    // carries the same shape and comes out of the same mint.
    for (index, region) in ["us-east-1", "us-west-2", "eu-west-2", "ap-southeast-2"]
        .iter()
        .enumerate()
    {
        for kind in COLLECTION_TYPES {
            let name = format!("form-{}-{index}", kind.to_lowercase());
            let case = format!("CreateCollection({name}/{kind} in {region})");
            let detail = create_collection(&router, region, &name, kind).await;
            let id = text(&detail, "id");
            assert_id_form(&case, &id);

            // The id is the host label of the endpoints derived from it, so the read that
            // follows reports the same one — the form is what every later reader serves.
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
            assert_id_form(&format!("{case}, read back"), &text(&stored, "id"));
        }

        let group_name = format!("form-group-{index}");
        let case = format!("CreateCollectionGroup({group_name} in {region})");
        let group = create_group(
            &router,
            region,
            json!({ "name": group_name, "standbyReplicas": "ENABLED" }),
        )
        .await;
        assert_id_form(&case, &text(&group, "id"));
    }
}

#[tokio::test]
async fn every_minted_version_has_the_form_aws_mints() {
    let router = emulator();

    // One row per version-bearing family of this service, on the create and then on the
    // write: both policy families with all three policy `type` values, the lifecycle policy
    // and the security configuration.
    for family in &FAMILIES {
        // `PolicyName` / `ConfigName` is @length(min: 3, max: 32) with `^[a-z][a-z0-9-]+$`.
        let name = format!("form-{}", family.kind);
        let created = create_versioned(&router, family, &name).await;
        let case = format!("{}({}/{name})", family.create, family.kind);
        let created_version = text(&created, family.version_member);
        assert_version_form(&case, &created_version);

        // The write is addressed AT the version the create minted — which is what makes the
        // form a contract and not decoration — and mints another one of the same form.
        let Some(update) = family.update else {
            continue;
        };
        let id = text(&created, "id");
        let written =
            update_versioned(&router, family, update, &name, &id, &created_version).await;
        let case = format!("{update}({}/{name})", family.kind);
        let written_version = text(&written, family.version_member);
        assert_version_form(&case, &written_version);
        assert_ne!(
            written_version, created_version,
            "case {case}: a write mints its own version, got the created one again"
        );
    }
}
