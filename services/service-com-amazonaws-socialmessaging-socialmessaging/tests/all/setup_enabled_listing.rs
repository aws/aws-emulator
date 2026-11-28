// SPDX-License-Identifier: CC0-1.0

//! What `ListLinkedWhatsAppBusinessAccounts` reports beside the page.
//!
//! RULE: the listing reports the account-level `setupEnabled` fact as a top-level member
//! beside `linkedAccounts`, read from the store — so every page of every listing carries
//! it, whatever that page holds, including a listing with nothing in it, and it reports
//! `false` exactly when the account's stored fact is `false`.
//!
//! The rows span what the listing HOLDS (nothing, one account, several, one page of
//! several, the page a `nextToken` resumes, and a listing emptied again by a
//! disassociation) and, for each, both values of the stored fact — the dimensions the flag
//! is independent of, and the one it is not.
//!
//! `setupEnabled` is recorded behaviour the published model snapshot does not declare, so
//! the listing's output shape is generated from this crate's own overlay model
//! `smithy-model/local-additions.json` (see `smithy-model/LOCAL-ADDITIONS.md` and the
//! module docs of `src/ops/types/list_linked_whats_app_business_accounts_types.rs`). The
//! last test pins that overlay against the snapshot, shape by shape: a refreshed snapshot
//! that declares the member itself, drops a member the overlay restates or changes a
//! target or type fails there rather than silently narrowing what the service reports.

use serde_json::{json, Map, Value};
use service_com_amazonaws_socialmessaging_socialmessaging::state::{
    ComAmazonawsSocialmessagingSocialmessagingState as Store, SETUP_ENABLED_DEFAULT,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{call, emulator, emulator_with_engine, link, query_escape, Answer};

/// The published model snapshot this crate is generated from, embedded at compile time so
/// the overlay is checked against the model rather than against a copy of it.
const SNAPSHOT: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/smithy-model/com-amazonaws-socialmessaging-socialmessaging.json"
));

/// The overlay model the listing's output is declared from, embedded for the same reason:
/// what it restates of the snapshot must still match it.
const OVERLAY: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/smithy-model/local-additions.json"
));

/// The shape the overlay restates with the account-level fact.
const OUTPUT_SHAPE: &str = "com.amazonaws.socialmessaging#ListLinkedWhatsAppBusinessAccountsOutput";
/// The member it adds, and the shape that member targets: the Smithy prelude boolean.
const ADDED_MEMBER: &str = "setupEnabled";
const ADDED_MEMBER_TARGET: &str = "smithy.api#Boolean";

/// The three accounts the paging rows list, in the order their minted ids sort.
const WABA_IDS: [&str; 3] = [
    "waba-11111111111111111111111111111111",
    "waba-22222222222222222222222222222222",
    "waba-33333333333333333333333333333333",
];

async fn listing(router: &MultiServiceRouter, query: &str) -> Answer {
    call(router, "GET", &format!("/v1/whatsapp/waba/list{query}"), None).await
}

/// Every listing of one account reports that account's stored fact, whatever the page
/// holds and however the page was asked for.
#[tokio::test]
async fn every_page_reports_the_accounts_stored_setup_fact() {
    for setup_enabled in [true, false] {
        let (router, engine) = emulator_with_engine();
        engine.state::<Store>().lock().setup_enabled = setup_enabled;

        // An empty account first: the flag is not derived from the listing, so it is
        // reported where there is nothing to list.
        let empty = listing(&router, "").await;
        assert_eq!(
            (empty.status, empty.at("/linkedAccounts")),
            (200, &json!([])),
            "case setup_enabled={setup_enabled}, nothing linked: expected an empty 200 \
             listing, got {} {:?}",
            empty.status,
            empty.body
        );
        assert_eq!(
            empty.at("/setupEnabled"),
            &json!(setup_enabled),
            "case setup_enabled={setup_enabled}, nothing linked: expected setupEnabled \
             {setup_enabled}, got {:?}",
            empty.body
        );

        let mut linked = Vec::new();
        for waba_id in WABA_IDS {
            linked.push(link(&router, waba_id, &["phone-number-id-a"]).await);
        }
        linked.sort();

        // Each row is one way of asking for a page: the whole listing, a page of one, the
        // page its token resumes, and a page bigger than the listing.
        let first = listing(&router, "?maxResults=1").await;
        let resumed_query = format!(
            "?maxResults=1&nextToken={}",
            query_escape(&first.string_at("/nextToken"))
        );
        let pages = [
            ("the whole listing", listing(&router, "").await, 3),
            ("a page of one", first, 1),
            ("the page its token resumes", listing(&router, &resumed_query).await, 1),
            ("a page wider than the listing", listing(&router, "?maxResults=100").await, 3),
        ];
        for (case, page, expected_len) in pages {
            let listed = page.at("/linkedAccounts").as_array().cloned().unwrap_or_default();
            assert_eq!(
                (page.status, listed.len()),
                (200, expected_len),
                "case setup_enabled={setup_enabled}, {case}: expected a 200 page of \
                 {expected_len}, got {} {:?}",
                page.status,
                page.body
            );
            assert_eq!(
                page.at("/setupEnabled"),
                &json!(setup_enabled),
                "case setup_enabled={setup_enabled}, {case}: expected setupEnabled \
                 {setup_enabled}, got {:?}",
                page.body
            );
            // The summaries beside the flag are still the modelled ones: the overlay
            // restates `LinkedWhatsAppBusinessAccountSummary` for its TYPE alone (see
            // `smithy-model/LOCAL-ADDITIONS.md`), so a page that serialised them as empty
            // objects would be a member set this listing silently lost.
            for (index, summary) in listed.iter().enumerate() {
                let id = summary.get("id").and_then(Value::as_str).unwrap_or_default();
                let waba_id = summary
                    .get("wabaId")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                assert!(
                    linked.iter().any(|linked_id| linked_id == id)
                        && WABA_IDS.contains(&waba_id),
                    "case setup_enabled={setup_enabled}, {case}: expected summary {index} to \
                     carry a linked account's id and its Meta WABA id, got {summary:?}"
                );
            }
        }

        // And a listing emptied again: unlinking every account does not unset the fact.
        for id in &linked {
            let gone = call(
                &router,
                "DELETE",
                &format!("/v1/whatsapp/waba/disassociate?id={}", query_escape(id)),
                None,
            )
            .await;
            assert_eq!(
                gone.status, 200,
                "case setup_enabled={setup_enabled}, disassociating {id:?}: expected 200, \
                 got {} {:?}",
                gone.status, gone.body
            );
        }
        let emptied = listing(&router, "").await;
        assert_eq!(
            (emptied.at("/linkedAccounts"), emptied.at("/setupEnabled")),
            (&json!([]), &json!(setup_enabled)),
            "case setup_enabled={setup_enabled}, emptied again: expected an empty listing \
             still reporting setupEnabled {setup_enabled}, got {:?}",
            emptied.body
        );
    }
}

/// The `shapes` object of one model document.
fn model_shapes(model: &str) -> Map<String, Value> {
    let parsed: Value = serde_json::from_str(model).expect("a model document must be JSON");
    parsed["shapes"]
        .as_object()
        .cloned()
        .expect("a model document must declare shapes")
}

/// The members of one shape declaration, or an empty set for a shape declared without any.
fn members_of(declaration: &Value) -> Map<String, Value> {
    declaration["members"].as_object().cloned().unwrap_or_default()
}

/// Is this member declaration `@required`?
fn is_required(member: &Value) -> bool {
    !member["traits"]["smithy.api#required"].is_null()
}

#[test]
fn the_overlay_restates_the_snapshot_and_adds_setup_enabled() {
    // The overlay is what the listing's output shape is generated from, so everything it
    // restates of the snapshot — a member's target and requiredness, a shape's type — must
    // still BE what the snapshot says, or what goes on the wire would differ from the model
    // without any other test noticing. Constraint traits are deliberately NOT restated
    // (see `smithy-model/LOCAL-ADDITIONS.md`): every request is validated against the
    // snapshot's own declarations on the way in.
    let snapshot = model_shapes(SNAPSHOT);
    let overlay = model_shapes(OVERLAY);

    let declared = overlay
        .get(OUTPUT_SHAPE)
        .unwrap_or_else(|| panic!("the overlay must declare {OUTPUT_SHAPE}"));
    let modelled = snapshot
        .get(OUTPUT_SHAPE)
        .unwrap_or_else(|| panic!("the snapshot no longer declares {OUTPUT_SHAPE}"));
    let declared_members = members_of(declared);
    let modelled_members = members_of(modelled);

    for (member, modelled_member) in &modelled_members {
        let restated = declared_members.get(member).unwrap_or_else(|| {
            panic!("the overlay drops {member} of {OUTPUT_SHAPE}, which the snapshot declares")
        });
        assert_eq!(
            restated["target"], modelled_member["target"],
            "case member {member:?}: must be restated with the target the snapshot gives it, \
             got {restated:?}"
        );
        assert_eq!(
            is_required(restated),
            is_required(modelled_member),
            "case member {member:?}: must be restated as required={}, got {restated:?}",
            is_required(modelled_member)
        );
    }

    let added: Vec<&String> = declared_members
        .keys()
        .filter(|member| !modelled_members.contains_key(*member))
        .collect();
    assert_eq!(
        added,
        vec![&ADDED_MEMBER.to_string()],
        "case {OUTPUT_SHAPE:?}: the overlay may add the account-level fact and nothing else, \
         added {added:?}"
    );
    assert!(
        !modelled_members.contains_key(ADDED_MEMBER),
        "case {OUTPUT_SHAPE:?}: the snapshot now declares {ADDED_MEMBER} itself, so the local \
         declaration should give way to the modelled member"
    );
    assert_eq!(
        declared_members[ADDED_MEMBER]["target"],
        json!(ADDED_MEMBER_TARGET),
        "case member {ADDED_MEMBER:?}: is a boolean, got {:?}",
        declared_members[ADDED_MEMBER]
    );

    // Every OTHER shape the overlay names is there only because those members target it,
    // and is restated as the snapshot declares it.
    for (shape_id, declared) in &overlay {
        if shape_id == OUTPUT_SHAPE {
            continue;
        }
        let modelled = snapshot.get(shape_id).unwrap_or_else(|| {
            panic!("the overlay restates {shape_id}, which the snapshot does not declare")
        });
        assert_eq!(
            declared["type"], modelled["type"],
            "case {shape_id:?}: must be restated with the type the snapshot gives it, got \
             {declared:?}"
        );
        match declared["type"].as_str() {
            Some("list") => assert_eq!(
                declared["member"]["target"], modelled["member"]["target"],
                "case {shape_id:?}: must be restated over the snapshot's own member target, \
                 got {declared:?}"
            ),
            // A structure member is resolved through the Rust field's type, which is
            // generated from the snapshot, so the overlay restates no member of one.
            Some("structure") => assert!(
                members_of(declared).is_empty(),
                "case {shape_id:?}: is restated for its type alone, so it must declare no \
                 members, got {declared:?}"
            ),
            // A scalar carries nothing but its type here.
            _ => assert!(
                members_of(declared).is_empty(),
                "case {shape_id:?}: a scalar target is restated for its type alone, got \
                 {declared:?}"
            ),
        }
    }
}

/// A store nobody has touched reports the account fact it starts from, so a caller reading
/// the flag to decide whether to drive the sign-up flow gets a value rather than nothing.
#[tokio::test]
async fn a_fresh_account_reports_the_default_setup_fact() {
    let router = emulator();
    let answer = listing(&router, "").await;
    assert_eq!(
        answer.at("/setupEnabled"),
        &json!(SETUP_ENABLED_DEFAULT),
        "a fresh account: expected setupEnabled {SETUP_ENABLED_DEFAULT}, got {:?}",
        answer.body
    );
}
