// SPDX-License-Identifier: CC0-1.0

//! `publicIpStatus` — the public IP connectivity of a Global Resolver — driven end-to-end
//! over this service's own `restJson1` wire through the parent suite's emulator.
//!
//! One rule, asserted over a table that spans the member's whole catalog: the value a
//! create resolves is stored on the resolver and reported by every operation that reports
//! the resource — a value the model declares to itself, an ABSENT member to the
//! connectivity a resolver created without one has — a value outside the catalog is the
//! modelled `ValidationException` (400), and the member is part of what an idempotent
//! replay compares, so one `clientToken` reused with a different connectivity conflicts
//! instead of answering with the resolver it already minted.

use serde_json::{json, Value};

use super::{resolver_service, Emulator, REGION};

/// The values `PublicIpStatus` accepts on the wire, from the crate's own rendering of the
/// model's enum (`smithy_query(query="shape",shape="PublicIpStatus")`), so this table
/// cannot drift from the catalog the handler refuses against.
const DECLARED: &[&str] = resolver_service::ops::types::PUBLIC_IP_STATUS_VALUES;

/// The connectivity a resolver created without the member reports: the service's own
/// default, stated once in `crate::state`.
fn defaulted() -> String {
    resolver_service::state::DEFAULT_PUBLIC_IP_STATUS
        .as_wire()
        .to_string()
}

/// The `publicIpStatus` the list reports for `id`.
fn listed_status(listed: &Value, id: &str) -> String {
    listed
        .as_array()
        .and_then(|items| {
            items
                .iter()
                .find(|item| item["id"] == json!(id))
                .and_then(|item| item["publicIpStatus"].as_str())
        })
        .unwrap_or_default()
        .to_string()
}

#[tokio::test]
async fn the_public_ip_status_a_create_resolves_is_reported_by_every_read() {
    let emulator = Emulator::new();

    // Rows: every value the model declares, each asked for explicitly, plus the two ways a
    // request names none at all — the member absent, and the member absent from a request
    // that carries the other optional members.
    let mut rows: Vec<(String, Option<&str>, String)> = DECLARED
        .iter()
        .map(|value| {
            (
                format!("{value} asked for explicitly"),
                Some(*value),
                (*value).to_string(),
            )
        })
        .collect();
    rows.push((
        "the member absent".to_string(),
        None,
        defaulted(),
    ));
    assert!(
        rows.len() >= 4,
        "case {:?}: expected the model's catalog to carry the values this rule is about, got {DECLARED:?}",
        "the table"
    );

    for (case, asked, expected) in rows {
        let mut body = serde_json::Map::new();
        body.insert("name".to_string(), json!("public-ip"));
        body.insert("regions".to_string(), json!([REGION]));
        if let Some(asked) = asked {
            body.insert("publicIpStatus".to_string(), json!(asked));
        }
        let created = emulator.create_resolver(Value::Object(body)).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected the create to be served, got {} with {}",
            created.status, created.body
        );
        assert_eq!(
            created.text("publicIpStatus"),
            expected,
            "case {case:?}: expected the create to report {expected:?}, got {:?}",
            created.text("publicIpStatus")
        );

        // The read answers out of the stored record, so it reports what the create did.
        let id = created.text("id");
        let fetched = emulator.get_resolver(&id).await;
        assert_eq!(
            fetched.text("publicIpStatus"),
            expected,
            "case {case:?}: expected the get to report the stored {expected:?}, got {:?}",
            fetched.text("publicIpStatus")
        );
        let listed = emulator.list_resolvers("").await;
        assert_eq!(
            listed_status(listed.member("globalResolvers"), &id),
            expected,
            "case {case:?}: expected the list to report the stored {expected:?}, got {}",
            listed.member("globalResolvers")
        );
    }
}

#[tokio::test]
async fn a_public_ip_status_outside_the_catalog_is_the_modelled_400() {
    let emulator = Emulator::new();

    // Rows: values outside the catalog, spanning the forms a caller gets wrong — the right
    // word in the wrong case, a value of the sibling enum, a near miss, and a value that
    // looks like a value of the member's own family.
    for asked in [
        "enabled_reserved",
        "IPV4",
        "ENABLED",
        "ENABLED_RELEASED",
        "DISABLED",
    ] {
        let answer = emulator
            .create_resolver(json!({
                "name": "refused",
                "regions": [REGION],
                "publicIpStatus": asked,
            }))
            .await;
        assert_eq!(
            answer.status, 400,
            "case {asked:?}: expected the modelled 400, got {} with {}",
            answer.status, answer.body
        );
        assert!(
            answer.error_type.contains("ValidationException"),
            "case {asked:?}: expected a ValidationException, got {:?} with {}",
            answer.error_type,
            answer.body
        );
        assert!(
            answer.text("message").contains("publicIpStatus"),
            "case {asked:?}: expected the member named in the message, got {:?}",
            answer.text("message")
        );
        for value in DECLARED {
            assert!(
                answer.text("message").contains(value),
                "case {asked:?}: expected the catalog stated back, got {:?}",
                answer.text("message")
            );
        }
    }

    // Nothing was minted by any of the refused creates.
    let listed = emulator.list_resolvers("").await;
    assert_eq!(
        listed.member("globalResolvers").as_array().map(Vec::len),
        Some(0),
        "case {:?}: expected a refused create to mint no resolver, got {}",
        "after the refusals",
        listed.member("globalResolvers")
    );
}

#[tokio::test]
async fn one_client_token_asking_for_another_public_ip_status_conflicts() {
    let emulator = Emulator::new();
    let body = |token: &str, status: &str| {
        json!({
            "clientToken": token,
            "name": "replayed",
            "regions": [REGION],
            "publicIpStatus": status,
        })
    };

    // Rows: each declared value as the one the token first minted, replayed with itself
    // (the same resolver) and then with each OTHER value (a conflict), so the member is
    // part of the comparison whichever value it carries.
    for (index, first) in DECLARED.iter().enumerate() {
        let token = format!("tok-{index}");
        let minted = emulator.create_resolver(body(&token, first)).await;
        assert_eq!(
            minted.status, 200,
            "case {first:?}: expected the first create to be served, got {} with {}",
            minted.status, minted.body
        );
        let replayed = emulator.create_resolver(body(&token, first)).await;
        assert_eq!(
            replayed.text("id"),
            minted.text("id"),
            "case {first:?}: a replay with the same connectivity answers with the resolver \
             the token already minted, got {:?}",
            replayed.text("id")
        );
        for other in DECLARED.iter().filter(|value| *value != first) {
            let conflicting = emulator.create_resolver(body(&token, other)).await;
            assert_eq!(
                conflicting.status, 409,
                "case {first:?} then {other:?}: expected the modelled 409, got {} with {}",
                conflicting.status, conflicting.body
            );
            assert_eq!(
                conflicting.text("resourceId"),
                minted.text("id"),
                "case {first:?} then {other:?}: expected the conflict to name the resolver \
                 the token minted, got {:?}",
                conflicting.text("resourceId")
            );
        }
    }
}
