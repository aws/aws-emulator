// SPDX-License-Identifier: CC0-1.0

//! A domain's tags over awsJson1_1: what `UpdateTagsForDomain` writes and
//! `ListTagsForDomain` reports.
//!
//! The rules pinned here are the ones the two operations document:
//!
//! * a tag is identified by its key — "[i]f you specify a key that already exists, the
//!   corresponding value will be replaced" — so writing a key twice leaves one tag, and
//!   the keys a request does not name keep the values they had;
//! * `ListTagsForDomain` "returns all of the tags that are associated with the specified
//!   domain", so a domain nobody has tagged reports an empty list rather than no list;
//! * tags belong to the domain, so they follow the store: a re-registered name starts
//!   untagged again.
//!
//! Every test drives the real wire path through the shared harness in
//! `tests/all/domain_lifecycle.rs`.

use crate::domain_lifecycle::{error_code, register, Emulator};
use serde_json::json;

/// The tags `ListTagsForDomain` reports for `name`, as (key, value) pairs sorted by key.
///
/// `pub(crate)` so the restore suite in `tests/all/domain_lifecycle.rs` can read a
/// restored registration's tags back through the one helper that reads them.
pub(crate) async fn tags_of(emulator: &Emulator, name: &str) -> Vec<(String, String)> {
    let (status, body) = emulator
        .call("ListTagsForDomain", &json!({ "DomainName": name }))
        .await;
    assert_eq!(
        status, 200,
        "case {name:?}: expected ListTagsForDomain to answer 200, got {status} {body}"
    );
    let list = body["TagList"]
        .as_array()
        .unwrap_or_else(|| panic!("case {name:?}: TagList must be a list, got {body}"))
        .clone();
    let mut tags: Vec<(String, String)> = list
        .iter()
        .map(|tag| {
            (
                tag["Key"].as_str().unwrap_or_default().to_string(),
                tag["Value"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    tags.sort();
    tags
}

/// Write `tags` through `UpdateTagsForDomain`, asserting it succeeded.
async fn update_tags(emulator: &Emulator, name: &str, tags: &[(&str, &str)]) {
    let body = json!({
        "DomainName": name,
        "TagsToUpdate": tags
            .iter()
            .map(|(key, value)| json!({ "Key": key, "Value": value }))
            .collect::<Vec<_>>()
    });
    let (status, response) = emulator.call("UpdateTagsForDomain", &body).await;
    assert_eq!(
        status, 200,
        "case {name:?} {tags:?}: expected UpdateTagsForDomain to answer 200, got {status} \
         {response}"
    );
}

#[tokio::test]
async fn tags_written_for_a_domain_are_the_tags_it_reports() {
    // Rows are successive writes against one domain, differing along what each write
    // does to the set: the first tags, a second key added, a key whose value is
    // replaced, a value written empty (`TagValue` allows "0-256 characters"), and a
    // write of no tags at all, which changes nothing.
    type Tags<'a> = &'a [(&'a str, &'a str)];
    let cases: &[(Tags<'_>, Tags<'_>)] = &[
        (&[("Env", "prod")], &[("Env", "prod")]),
        (
            &[("Owner", "platform")],
            &[("Env", "prod"), ("Owner", "platform")],
        ),
        (
            &[("Env", "staging")],
            &[("Env", "staging"), ("Owner", "platform")],
        ),
        (
            &[("Owner", "")],
            &[("Env", "staging"), ("Owner", "")],
        ),
        (
            &[],
            &[("Env", "staging"), ("Owner", "")],
        ),
        (
            // A key with the punctuation `Tag.Key` documents as valid
            // ("A-Z, a-z, 0-9, space, \".:/=+-@\""), written alongside a replacement.
            &[("cost.center/1", "a=b+c"), ("Env", "prod")],
            &[("Env", "prod"), ("Owner", ""), ("cost.center/1", "a=b+c")],
        ),
    ];

    let emulator = Emulator::new();
    let name = "sim-probe-tags.com";
    register(&emulator, name).await;

    // A domain nobody has tagged reports an empty list, not a missing one.
    assert_eq!(
        tags_of(&emulator, name).await,
        Vec::new(),
        "case \"a freshly registered domain\": expected no tags"
    );

    for (write, expected) in cases {
        update_tags(&emulator, name, write).await;
        let reported = tags_of(&emulator, name).await;
        let expected: Vec<(String, String)> = expected
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        assert_eq!(
            reported, expected,
            "case {write:?}: expected {expected:?}, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn tags_belong_to_the_domain_they_were_written_for() {
    // Rows differ along the domain: a write names one domain, so another domain's tags
    // are untouched, and a name that is registered again starts untagged.
    let emulator = Emulator::new();
    let first = "sim-probe-tags-one.com";
    let second = "sim-probe-tags-two.net";
    register(&emulator, first).await;
    register(&emulator, second).await;

    update_tags(&emulator, first, &[("Env", "prod")]).await;
    assert_eq!(
        tags_of(&emulator, first).await,
        vec![("Env".to_string(), "prod".to_string())],
        "case {first:?}: expected the tag that was written for it"
    );
    assert_eq!(
        tags_of(&emulator, second).await,
        Vec::new(),
        "case {second:?}: a write for another domain must not tag this one"
    );

    // The tags go with the registration: deleting the name drops them.
    let (status, body) = emulator
        .call("DeleteDomain", &json!({ "DomainName": first }))
        .await;
    assert_eq!(
        status, 200,
        "case {first:?}: expected DeleteDomain to succeed, got {status} {body}"
    );
    register(&emulator, first).await;
    assert_eq!(
        tags_of(&emulator, first).await,
        Vec::new(),
        "case {first:?} re-registered: expected a fresh registration to carry no tags"
    );
}

#[tokio::test]
async fn a_domain_holds_at_most_the_stated_number_of_tags() {
    // The tag store is caller-driven, so it has a ceiling. Rows differ along how the
    // request reaches it: exactly at the ceiling (served), one write past it (refused),
    // and a single request that is itself larger than the ceiling (refused before
    // anything is merged).
    use service_com_amazonaws_route53domains_route53domains_v20140515::state::MAX_TAGS_PER_DOMAIN;

    let emulator = Emulator::new();
    let name = "sim-probe-tags-limit.com";
    register(&emulator, name).await;

    let keys: Vec<String> = (0..MAX_TAGS_PER_DOMAIN)
        .map(|index| format!("key-{index:04}"))
        .collect();
    let at_ceiling: Vec<(&str, &str)> = keys.iter().map(|key| (key.as_str(), "v")).collect();
    update_tags(&emulator, name, &at_ceiling).await;
    assert_eq!(
        tags_of(&emulator, name).await.len(),
        MAX_TAGS_PER_DOMAIN,
        "case \"at the ceiling\": expected every tag to be kept"
    );

    for case in ["one past the ceiling", "a request larger than the ceiling"] {
        let tags: Vec<serde_json::Value> = if case == "one past the ceiling" {
            vec![json!({ "Key": "one-too-many", "Value": "v" })]
        } else {
            (0..=MAX_TAGS_PER_DOMAIN)
                .map(|index| json!({ "Key": format!("bulk-{index:04}"), "Value": "v" }))
                .collect()
        };
        let (status, body) = emulator
            .call(
                "UpdateTagsForDomain",
                &json!({ "DomainName": name, "TagsToUpdate": tags }),
            )
            .await;
        assert_eq!(
            (status, error_code(&body)),
            (400, "InvalidInput".to_string()),
            "case {case:?}: expected 400 InvalidInput, got {status} {body}"
        );
        assert_eq!(
            tags_of(&emulator, name).await.len(),
            MAX_TAGS_PER_DOMAIN,
            "case {case:?}: a refused request must write nothing"
        );
    }
}

#[tokio::test]
async fn a_tag_without_a_key_is_refused_and_writes_nothing() {
    // A tag is identified by its key, so a tag that carries none names nothing to add or
    // update. Rows differ along the form the missing key takes, and each is sent
    // alongside a well-formed tag to show the whole request is refused rather than
    // partly applied.
    let emulator = Emulator::new();
    let name = "sim-probe-tags-refused.com";
    register(&emulator, name).await;
    update_tags(&emulator, name, &[("Env", "prod")]).await;

    let cases = [
        // No key at all, and a key that is only spaces: both satisfy `TagKey`'s modelled
        // length, so the handler is what refuses them, with the modelled `InvalidInput`.
        (json!({ "Value": "no key at all" }), "InvalidInput"),
        (json!({ "Key": "   ", "Value": "a blank key" }), "InvalidInput"),
        // An empty key breaks `TagKey`'s modelled length (1..128), so the boundary
        // validator refuses it before the handler runs — with the same modelled error.
        (
            json!({ "Key": "", "Value": "an empty key" }),
            "InvalidInput",
        ),
    ];

    for (case, expected_code) in cases {
        let (status, body) = emulator
            .call(
                "UpdateTagsForDomain",
                &json!({
                    "DomainName": name,
                    "TagsToUpdate": [json!({ "Key": "Owner", "Value": "platform" }), case]
                }),
            )
            .await;
        assert_eq!(
            (status, error_code(&body)),
            (400, expected_code.to_string()),
            "case {case:?}: expected 400 {expected_code}, got {status} {body}"
        );
        assert_eq!(
            tags_of(&emulator, name).await,
            vec![("Env".to_string(), "prod".to_string())],
            "case {case:?}: a refused request must write nothing"
        );
    }
}
