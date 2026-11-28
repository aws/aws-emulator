// SPDX-License-Identifier: CC0-1.0

//! `DescribeRegistries`: the public registry this account has, reported from state.
//!
//! The rule these cases pin: an account has exactly ONE public registry, and
//! `DescribeRegistries` reports it from persisted state for every request the modelled
//! input admits — the same `registryId` (the account id), the same partition-wide,
//! region-less `registryArn`, the same alias-scoped `registryUri`, and an alias record
//! whose every modelled member is populated — while changing nothing a later caller sees.
//! Its `@paginated` contract holds over that one-entry list: any page size in the
//! modelled range serves it with no continuation token, and a token this service did not
//! mint is an invalid parameter.
//!
//! Every case goes through the wire front door (`POST /` with
//! `X-Amz-Target: SpencerFrontendService.<Operation>`) using the harness in
//! `repository_image_lifecycle`, so nothing reads the state struct directly.

use serde_json::{json, Value};
use service_com_amazonaws_ecrpublic_spencerfrontendservice::ops::types::REGISTRY_ALIAS_STATUS_VALUES;
// The status the registry's alias carries: one source, shared with the handler rather than
// restated here.
use service_com_amazonaws_ecrpublic_spencerfrontendservice::state::REGISTRY_ALIAS_STATUS_ACTIVE;

use crate::repository_image_lifecycle::{
    ok, published_authority, refused, service, text, ARN_PREFIX, REGISTRY_ID,
};

#[test]
fn the_alias_status_this_service_reports_is_one_the_model_declares() {
    assert!(
        REGISTRY_ALIAS_STATUS_VALUES.contains(&REGISTRY_ALIAS_STATUS_ACTIVE),
        "expected the reported status to be one of {REGISTRY_ALIAS_STATUS_VALUES:?}, \
         got {REGISTRY_ALIAS_STATUS_ACTIVE:?}"
    );
}

/// Whether `name` satisfies the modelled `RegistryAliasName` pattern
/// `^[a-z][a-z0-9]+(?:[._-][a-z0-9]+)*$` and its 2..50 length. Source: the model, via
/// `smithy_query(query="shape", shape="RegistryAlias")`.
fn is_registry_alias_name(name: &str) -> bool {
    if !(2..=50).contains(&name.len()) {
        return false;
    }
    let mut chars = name.chars();
    // `[a-z]` then `[a-z0-9]+`: the leading run is at least two characters long, so a
    // separator cannot appear before the second one.
    if !chars.next().is_some_and(|first| first.is_ascii_lowercase()) {
        return false;
    }
    if !chars
        .next()
        .is_some_and(|second| second.is_ascii_lowercase() || second.is_ascii_digit())
    {
        return false;
    }
    // `(?:[._-][a-z0-9]+)*`: every separator is followed by at least one alphanumeric,
    // which rules out a doubled separator and a trailing one.
    let mut separator_pending = false;
    for character in chars {
        match character {
            'a'..='z' | '0'..='9' => separator_pending = false,
            '.' | '_' | '-' if !separator_pending => separator_pending = true,
            _ => return false,
        }
    }
    !separator_pending
}

#[test]
fn the_alias_name_check_is_the_modelled_pattern() {
    // Each row is a form the modelled `RegistryAliasName`
    // (`^[a-z][a-z0-9]+(?:[._-][a-z0-9]+)*$`, length 2..50) does or does not admit, so a
    // check weaker than the pattern cannot pass silently.
    let admitted = [
        "ab",
        "a1",
        "emulator",
        "ab.cd",
        "ab_cd",
        "ab-cd",
        "a0.b1-c2_d3",
        &"z".repeat(50),
    ];
    for name in admitted {
        assert!(
            is_registry_alias_name(name),
            "case {name:?}: the modelled pattern admits this name"
        );
    }

    let refused = [
        "a",                 // below the 2..50 length minimum
        &"z".repeat(51),     // above the maximum
        "1ab",               // does not start with a letter
        "AB",                // upper case
        "a.bc",              // separator before the second character of the first run
        "ab..cd",            // doubled separator
        "ab.",               // trailing separator
        ".ab",               // leading separator
        "ab cd",             // a character the pattern does not admit
    ];
    for name in refused {
        assert!(
            !is_registry_alias_name(name),
            "case {name:?}: the modelled pattern refuses this name"
        );
    }
}

/// Every rule a reported `Registry` record obeys, whatever the request that asked for it.
///
/// Returns the registry's primary alias name, which is the alias every `repositoryUri` of
/// this registry is scoped by.
fn assert_registry_record(record: &Value, case: &str) -> String {
    // `Registry$registryId` is "the Amazon Web Services account ID that's associated with
    // the registry", modelled `^[0-9]{12}$`.
    let registry_id = text(record, &["registryId"], case);
    assert!(
        registry_id.len() == 12 && registry_id.chars().all(|digit| digit.is_ascii_digit()),
        "case {case:?}: expected a 12-digit registryId, got {registry_id:?}"
    );
    assert_eq!(
        registry_id, REGISTRY_ID,
        "case {case:?}: expected the account id this emulator serves, got {registry_id:?}"
    );

    // ECR Public is partition-wide, so the ARN of the registry carries an EMPTY region
    // segment, the registry id as its account, and the registry itself as its resource.
    let registry_arn = text(record, &["registryArn"], case);
    assert_eq!(
        registry_arn,
        format!("arn:aws:ecr-public::{registry_id}:registry/{registry_id}"),
        "case {case:?}: expected a region-less registry ARN naming the registry, \
         got {registry_arn:?}"
    );

    // `verified` is "whether the account is a verified Amazon Web Services Marketplace
    // vendor": modelled as a required boolean, so it is always present.
    let verified = record["verified"].as_bool().unwrap_or_else(|| {
        panic!("case {case:?}: expected a boolean `verified` member, got {record}")
    });
    assert!(
        !verified,
        "case {case:?}: an emulated account is no Marketplace vendor, got verified={verified}"
    );

    let aliases = record["aliases"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected an aliases array, got {record}"));
    assert!(
        !aliases.is_empty(),
        "case {case:?}: expected the registry's alias record, got {record}"
    );

    let mut primary: Option<String> = None;
    let mut defaults = 0;
    for alias in aliases {
        let alias_case = format!("{case} alias {alias}");
        let name = text(alias, &["name"], &alias_case);
        assert!(
            is_registry_alias_name(&name),
            "case {alias_case:?}: expected a name satisfying RegistryAliasName, got {name:?}"
        );
        // Every member of `RegistryAlias` is modelled required, so each one is populated.
        let status = text(alias, &["status"], &alias_case);
        assert!(
            REGISTRY_ALIAS_STATUS_VALUES.contains(&status.as_str()),
            "case {alias_case:?}: expected one of {REGISTRY_ALIAS_STATUS_VALUES:?}, \
             got {status:?}"
        );
        let is_primary = alias["primaryRegistryAlias"].as_bool().unwrap_or_else(|| {
            panic!("case {alias_case:?}: expected a boolean primaryRegistryAlias")
        });
        let is_default = alias["defaultRegistryAlias"].as_bool().unwrap_or_else(|| {
            panic!("case {alias_case:?}: expected a boolean defaultRegistryAlias")
        });
        if is_primary {
            assert!(
                primary.is_none(),
                "case {case:?}: expected at most one primary alias, got {record}"
            );
            primary = Some(name.clone());
        }
        if is_default {
            defaults += 1;
        }
    }
    assert_eq!(
        defaults, 1,
        "case {case:?}: expected exactly one default registry alias, got {record}"
    );

    let primary = primary.unwrap_or_else(|| {
        panic!("case {case:?}: expected a primary alias, the one shown in a pull URL, {record}")
    });

    // `registryUri` is the prefix a client prepends to a repository name and points a
    // login at, so it is an ADDRESS first: the public-registry name this emulator announces
    // to the reader, scoped by the alias shown in a `docker pull` command — the primary
    // alias — and carrying neither a region nor an account.
    let registry_uri = text(record, &["registryUri"], case);
    assert_eq!(
        registry_uri,
        format!("{}/{primary}", published_authority()),
        "case {case:?}: expected the published registry name scoped by the primary alias, \
         got {registry_uri:?}"
    );

    primary
}

/// One `DescribeRegistries` call that must answer the single registry, returning its
/// record.
async fn the_registry(
    router: &smithy_server_runtime::MultiServiceRouter,
    body: Value,
    case: &str,
) -> Value {
    let reply = ok(router, "DescribeRegistries", body, case).await;
    let registries = reply["registries"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a registries array, got {reply}"));
    assert_eq!(
        registries.len(),
        1,
        "case {case:?}: expected the account's one public registry, got {reply}"
    );
    assert!(
        reply["nextToken"].as_str().is_none(),
        "case {case:?}: one registry fits on any page, so no token is due, got {reply}"
    );
    registries[0].clone()
}

#[tokio::test]
async fn describe_registries_reports_the_one_registry_at_every_page_size() {
    let router = service();

    // `maxResults` is modelled `range 1..1000`: the rows are the absent parameter, both
    // published bounds, and sizes in between.
    let page_sizes = [None, Some(1), Some(2), Some(7), Some(100), Some(999), Some(1000)];

    let mut reported: Vec<(String, Value)> = Vec::new();
    for max_results in page_sizes {
        let case = format!("maxResults={max_results:?}");
        let body = match max_results {
            Some(max_results) => json!({"maxResults": max_results}),
            None => json!({}),
        };
        let record = the_registry(&router, body, &case).await;
        assert_registry_record(&record, &case);
        reported.push((case, record));
    }

    // The registry's identity does not depend on the request that asked for it, and it is
    // stable across calls.
    let (first_case, first) = &reported[0];
    for (case, record) in &reported[1..] {
        assert_eq!(
            record, first,
            "case {case:?}: expected the same registry as case {first_case:?} reported, \
             got {record} and {first}"
        );
    }
}

#[tokio::test]
async fn the_registry_is_the_one_every_repository_is_scoped_by() {
    let router = service();

    let before = the_registry(&router, json!({}), "before any repository").await;
    let alias = assert_registry_record(&before, "before any repository");
    let registry_uri = text(&before, &["registryUri"], "before any repository");

    // One row per repository-name form the modelled pattern admits: the registry's
    // scoping holds for every one of them, not just a flat name.
    for name in ["ab", "nginx", "team.app", "project-a/nginx-web-app", "org/team/app"] {
        let case = format!("repository {name:?}");
        let created = ok(
            &router,
            "CreateRepository",
            json!({"repositoryName": name}),
            &case,
        )
        .await;
        let uri = text(&created, &["repository", "repositoryUri"], &case);
        assert_eq!(
            uri,
            format!("{registry_uri}/{name}"),
            "case {case:?}: expected the repository URI under the registry URI, got {uri:?}"
        );
        let arn = text(&created, &["repository", "repositoryArn"], &case);
        assert_eq!(
            arn,
            format!("{ARN_PREFIX}{name}"),
            "case {case:?}: expected a region-less repository ARN in this registry, \
             got {arn:?}"
        );
        assert_eq!(
            text(&created, &["repository", "registryId"], &case),
            REGISTRY_ID,
            "case {case:?}: expected the registry id DescribeRegistries reports, {created}"
        );
    }

    // Creating repositories neither moved the registry's identity nor its alias record.
    let after = the_registry(&router, json!({}), "after repositories").await;
    assert_eq!(
        after, before,
        "expected the same registry record across calls, got {after} and {before}"
    );

    // Both names the registry answers to — its account id and its alias — reach the same
    // registry on an operation whose `registryId` admits either, and the answer always
    // normalizes to the account id.
    for (case, registry_id) in [("account id", REGISTRY_ID.to_string()), ("alias", alias)] {
        let pushed = ok(
            &router,
            "PutImage",
            json!({
                "registryId": registry_id,
                "repositoryName": "nginx",
                "imageManifest": format!("{{\"mediaType\":\"m\",\"by\":\"{case}\"}}"),
                "imageTag": case.replace(' ', "-"),
            }),
            case,
        )
        .await;
        assert_eq!(
            text(&pushed, &["image", "registryId"], case),
            REGISTRY_ID,
            "case {case:?}: expected the account id, whichever name the request used, \
             got {pushed}"
        );
    }
}

#[tokio::test]
async fn describe_registries_refuses_input_outside_the_modelled_contract() {
    let router = service();

    // One past each published `maxResults` bound.
    for (case, max_results) in [("below the minimum", 0), ("above the maximum", 1001)] {
        refused(
            &router,
            "DescribeRegistries",
            json!({"maxResults": max_results}),
            "InvalidParameterException",
            case,
        )
        .await;
    }

    // A `nextToken` this service did not mint, in every form one can take: not hex,
    // odd-length hex, hex that decodes to no text, and — the case a well-formed token
    // makes reachable — hex of a key this listing could never have ended on.
    for case in [
        "not-hex".to_string(),
        "zz".to_string(),
        "abc".to_string(),
        "ff".to_string(),
        hex::encode("zzz"),
        hex::encode("nginx"),
        hex::encode("999999999999"),
    ] {
        refused(
            &router,
            "DescribeRegistries",
            json!({"nextToken": case}),
            "InvalidParameterException",
            &case,
        )
        .await;
    }

    // The one key a page of this listing CAN end on resumes after the registry: an empty
    // page, and no further token.
    let case = "resumed past the registry";
    let resumed = ok(
        &router,
        "DescribeRegistries",
        json!({"nextToken": hex::encode(REGISTRY_ID)}),
        case,
    )
    .await;
    assert_eq!(
        resumed["registries"].as_array().map(Vec::len),
        Some(0),
        "case {case:?}: expected nothing after the only registry, got {resumed}"
    );
    assert!(
        resumed["nextToken"].as_str().is_none(),
        "case {case:?}: expected no further token, got {resumed}"
    );
}

#[tokio::test]
async fn describing_registries_changes_nothing_a_later_caller_sees() {
    let router = service();

    let empty = ok(&router, "DescribeRepositories", json!({}), "before").await;
    assert_eq!(
        empty["repositories"].as_array().map(Vec::len),
        Some(0),
        "expected a fresh registry to hold no repository, got {empty}"
    );

    let first = the_registry(&router, json!({}), "first read").await;
    let second = the_registry(&router, json!({}), "second read").await;
    assert_eq!(
        first, second,
        "expected a repeated read to report the same registry, got {first} and {second}"
    );

    let still_empty = ok(&router, "DescribeRepositories", json!({}), "after").await;
    assert_eq!(
        still_empty, empty,
        "expected DescribeRegistries to create nothing, got {still_empty} after {empty}"
    );
}
