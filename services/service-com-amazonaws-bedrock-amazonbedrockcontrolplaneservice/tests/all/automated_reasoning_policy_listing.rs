// SPDX-License-Identifier: CC0-1.0

//! `ListAutomatedReasoningPolicies` end-to-end: what the create, the version cut and the
//! read persist is what the listing reports, over the same `MultiServiceRouter` the
//! emulator boots.
//!
//! The rules under test, each asserted over a SET of inputs rather than over one recorded
//! request:
//!
//! 1. **An unfiltered listing reports the WORKING DRAFT of every policy of the signing
//!    region**, member for member what `GetAutomatedReasoningPolicy` reports for the same
//!    ARN — for policies differing in name spelling, in whether a description, a definition
//!    and tags are present, and in the region they were created in. No numbered version is
//!    listed, as `policyArn` documents ("If not provided, the DRAFT versions for all
//!    policies are listed"), and the listing writes nothing: a second call answers the
//!    same, and a read after it is unchanged.
//! 2. **The listing is ordered by `(createdAt, policyArn)` and its pages partition it**:
//!    over page sizes from 1 to past the whole listing, the pages concatenate to that one
//!    order with neither a gap nor a repeat, and `nextToken` is present exactly while rows
//!    remain.
//! 3. **A `policyArn` filter selects the revisions of the one policy it names**: an
//!    unversioned ARN lists that policy's draft and every version cut from it, and a
//!    versioned ARN lists exactly that version — for policies carrying 0, 1 and 3 versions.
//! 4. **An ARN naming no revision of this account is 404**, refused rather than answered as
//!    an empty listing, and refused with the sentence THIS operation words that miss in —
//!    over the halves of the ARN that can name nothing, each checked against the sentence a
//!    read of the same ARN answers, which words the absent policy its own way.
//! 5. **A `nextToken` this service did not issue is refused** with `ValidationException`,
//!    over a set of malformed tokens, while a token the listing itself issued resumes it.
//!
//! Every expectation is derived from the model, cited per row
//! (`smithy_query(query="operation",operation="ListAutomatedReasoningPolicies")`,
//! `smithy_query(query="shape",shape="AutomatedReasoningPolicySummary")`). The requests go
//! through `crate::support` — the ONE harness every suite in this crate drives the emulator
//! with.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_bedrock_amazonbedrockcontrolplaneservice as bedrock;

use crate::support::{emulator, label, Emulator, Reply, ACCOUNT, REGION};

/// The working version a policy's unversioned ARN names:
/// `CreateAutomatedReasoningPolicyResponse$version` documents "The initial version is
/// always DRAFT".
const DRAFT: &str = "DRAFT";

/// The page size an absent `maxResults` takes: that member's own modelled `default`
/// (`smithy_query(query="operation",operation="ListAutomatedReasoningPolicies")`).
const DEFAULT_PAGE_SIZE: usize = 25;

/// `ResourceNotFoundException` for a `policyArn` FILTER naming a policy this account holds
/// no record of: this operation's own sentence for that class, written out here rather than
/// read off the crate so this suite pins the text independently of the constant that
/// produces it. The model carries no template for it
/// (`smithy_query(query="shape",shape="ResourceNotFoundException")` documents the class
/// only), so the words are the requirement.
const LISTING_POLICY_NOT_FOUND: &str = "Policy does not exist.";

/// The same class as [`LISTING_POLICY_NOT_FOUND`] as a READ of one policy words it, which
/// is not this operation's wording.
const READ_POLICY_NOT_FOUND: &str = "Policy does not exist";

/// `ResourceNotFoundException` for an ARN naming a version a policy this account DOES hold
/// has not cut: the one sentence every operation reports that half of the ARN in.
const POLICY_VERSION_NOT_FOUND: &str = "Policy version does not exist";

/// The ARN prefix every policy minted for `region` carries.
fn arn_prefix(region: &str) -> String {
    format!("arn:aws:bedrock:{region}:{ACCOUNT}:automated-reasoning-policy/")
}

/// A policy definition with `count` types, variables and rules, as the definition shapes
/// spell them (`smithy_query(query="closure",operation="CreateAutomatedReasoningPolicy")`:
/// a type name is `^\p{L}[\p{L}\p{M}0-9_]*( [\p{L}\p{M}0-9_]+)*$`, a rule id is
/// `^[A-Z][0-9A-Z]{11}$`).
fn definition(count: usize) -> Value {
    let types: Vec<Value> = (0..count)
        .map(|index| {
            json!({
                "name": format!("Type{index}"),
                "description": format!("type {index}"),
                "values": [{ "value": format!("Value{index}A") }],
            })
        })
        .collect();
    let variables: Vec<Value> = (0..count)
        .map(|index| {
            json!({
                "name": format!("variable{index}"),
                "type": format!("Type{index}"),
                "description": format!("variable {index}"),
            })
        })
        .collect();
    let rules: Vec<Value> = (0..count)
        .map(|index| {
            json!({
                "id": format!("R{:011}", index),
                "expression": format!("variable{index} == Value{index}A"),
            })
        })
        .collect();
    json!({ "version": "1", "types": types, "variables": variables, "rules": rules })
}

async fn create_policy_in_region(emu: &Emulator, region: &str, body: Value) -> Reply {
    emu.call_in_region(region, "POST", "/automated-reasoning-policies", Some(body))
        .await
}

async fn create_policy(emu: &Emulator, body: Value) -> Reply {
    create_policy_in_region(emu, REGION, body).await
}

/// Cut a numbered version of `policy_arn`, held against the draft's current
/// `definitionHash` — the concurrency token `CreateAutomatedReasoningPolicyVersion`
/// requires. Cutting a version leaves the draft alone, so one hash cuts every version of
/// one policy.
async fn create_version(emu: &Emulator, policy_arn: &str, definition_hash: &str) -> Reply {
    emu.call(
        "POST",
        &format!(
            "/automated-reasoning-policies/{}/versions",
            label(policy_arn)
        ),
        Some(json!({ "lastUpdatedDefinitionHash": definition_hash })),
    )
    .await
}

async fn get_policy(emu: &Emulator, policy_arn: &str) -> Reply {
    emu.call(
        "GET",
        &format!("/automated-reasoning-policies/{}", label(policy_arn)),
        None,
    )
    .await
}

async fn list_in_region(emu: &Emulator, region: &str, query: &str) -> Reply {
    let path = if query.is_empty() {
        "/automated-reasoning-policies".to_string()
    } else {
        format!("/automated-reasoning-policies?{query}")
    };
    emu.call_in_region(region, "GET", &path, None).await
}

async fn list(emu: &Emulator, query: &str) -> Reply {
    list_in_region(emu, REGION, query).await
}

/// The policy `body` creates, as `(policyArn, definitionHash)`.
async fn policy_of(emu: &Emulator, case: &str, body: Value) -> (String, String) {
    let created = create_policy(emu, body).await;
    assert_eq!(
        created.status, 200,
        "case {case:?}: CreateAutomatedReasoningPolicy must answer 200, got {} {:?}",
        created.status, created.body
    );
    (created.text("policyArn"), created.text("definitionHash"))
}

/// The policy `body` creates, as its `policyArn`.
async fn policy_arn_of(emu: &Emulator, case: &str, body: Value) -> String {
    policy_of(emu, case, body).await.0
}

/// The summaries a listing reply carries, panicking when the required member is absent or
/// is not a list.
fn summaries(case: &str, reply: &Reply) -> Vec<Value> {
    let value = reply.get("automatedReasoningPolicySummaries");
    value
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: automatedReasoningPolicySummaries is required and modelled a \
                 list, got {value:?}"
            )
        })
        .clone()
}

/// The `policyArn` of each summary, in the order the listing reported them.
fn listed_arns(case: &str, reply: &Reply) -> Vec<String> {
    summaries(case, reply)
        .iter()
        .map(|summary| {
            summary
                .get("policyArn")
                .and_then(Value::as_str)
                .unwrap_or_else(|| {
                    panic!("case {case:?}: every summary carries a policyArn, got {summary:?}")
                })
                .to_string()
        })
        .collect()
}

/// Move the stored policy `policy_arn`'s instants to `at`, as though it had been created
/// then.
///
/// The record is reached through the very store the handlers write
/// (`bedrock::state::service_state` over the harness's shared engine), so what a row fixes
/// is what a request finds — and fixing the instant a policy was created is what lets a row
/// pin the ORDER a listing must walk without sleeping between creates.
fn set_created_at(emu: &Emulator, policy_arn: &str, at: SystemTime) {
    let state = bedrock::state::service_state(&*emu.engine)
        .expect("the shared engine carries this service's store");
    let mut store = state.lock();
    let policy = store
        .automated_reasoning_policy_mut(policy_arn)
        .unwrap_or_else(|| panic!("the store must hold the policy {policy_arn}"));
    policy.created_at = at;
    policy.updated_at = at;
}

// ---------------------------------------------------------------------------------------
// 1. What the store holds is what the listing reports.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn the_listing_reports_the_draft_of_every_policy_of_the_region() {
    // Rows differ along what the create carried and the region it addressed: the listing's
    // rule is about the stored record, not about one request's shape.
    struct Row {
        name: &'static str,
        region: &'static str,
        description: Option<&'static str>,
        definition: Option<Value>,
        tags: bool,
    }

    let rows = vec![
        Row {
            name: "minimal policy",
            region: REGION,
            description: None,
            definition: None,
            tags: false,
        },
        Row {
            name: "Described-Policy_1",
            region: REGION,
            description: Some("validates the returns rules"),
            definition: None,
            tags: true,
        },
        Row {
            name: "0123456789",
            region: REGION,
            description: Some("three types, variables and rules"),
            definition: Some(definition(3)),
            tags: false,
        },
        Row {
            name: "elsewhere policy",
            region: "eu-west-1",
            description: Some("another account-region's policy"),
            definition: Some(definition(1)),
            tags: true,
        },
    ];

    let emu = emulator();
    let mut created: Vec<(&Row, String)> = Vec::new();
    for row in &rows {
        let mut body = json!({ "name": row.name });
        if let Some(description) = row.description {
            body["description"] = json!(description);
        }
        if let Some(definition) = &row.definition {
            body["policyDefinition"] = definition.clone();
        }
        if row.tags {
            body["tags"] = json!([{ "key": "team", "value": "search" }]);
        }
        let reply = create_policy_in_region(&emu, row.region, body).await;
        assert_eq!(
            reply.status, 200,
            "case {:?}: CreateAutomatedReasoningPolicy must answer 200, got {} {:?}",
            row.name, reply.status, reply.body
        );
        created.push((row, reply.text("policyArn")));
    }

    let listed = list(&emu, "").await;
    assert_eq!(
        listed.status, 200,
        "case the whole listing: ListAutomatedReasoningPolicies must answer 200, got {} {:?}",
        listed.status, listed.body
    );
    assert_eq!(
        listed.get("nextToken"),
        Value::Null,
        "case the whole listing: {} policies fit in one page of {DEFAULT_PAGE_SIZE}, so no \
         nextToken is issued, got {:?}",
        rows.len(),
        listed.get("nextToken")
    );

    let in_region: Vec<&(&Row, String)> = created
        .iter()
        .filter(|(row, _)| row.region == REGION)
        .collect();
    let reported = summaries("the whole listing", &listed);
    assert_eq!(
        reported.len(),
        in_region.len(),
        "case the whole listing: expected one summary per policy of {REGION} ({}), got {} \
         {:?}",
        in_region.len(),
        reported.len(),
        listed.body
    );

    // Each summary is checked against what a READ of the same ARN reports, so the listing
    // is held to the stored record rather than to a literal.
    for (row, arn) in &in_region {
        let case = row.name;
        let summary = reported
            .iter()
            .find(|summary| summary.get("policyArn") == Some(&json!(arn)))
            .unwrap_or_else(|| {
                panic!("case {case:?}: the listing must carry {arn:?}, got {:?}", listed.body)
            });
        let read = get_policy(&emu, arn).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: GetAutomatedReasoningPolicy must answer 200, got {} {:?}",
            read.status, read.body
        );
        for member in ["name", "policyId", "createdAt", "updatedAt"] {
            assert_eq!(
                summary.get(member).cloned().unwrap_or(Value::Null),
                read.get(member),
                "case {case:?}: the summary's {member} must be what the read reports, got {:?}",
                summary.get(member)
            );
        }
        assert_eq!(
            summary.get("description").cloned().unwrap_or(Value::Null),
            row.description.map_or(Value::Null, |text| json!(text)),
            "case {case:?}: the summary must report the stored description, got {:?}",
            summary.get("description")
        );
        assert_eq!(
            summary.get("version").cloned().unwrap_or(Value::Null),
            json!(DRAFT),
            "case {case:?}: an unfiltered listing reports the DRAFT version, got {:?}",
            summary.get("version")
        );
    }

    // The policy of another region is listed by THAT region and by no other.
    let elsewhere = list_in_region(&emu, "eu-west-1", "").await;
    let elsewhere_arns = listed_arns("eu-west-1", &elsewhere);
    let expected_elsewhere: Vec<String> = created
        .iter()
        .filter(|(row, _)| row.region == "eu-west-1")
        .map(|(_, arn)| arn.clone())
        .collect();
    assert_eq!(
        elsewhere_arns, expected_elsewhere,
        "case eu-west-1: a listing reports the policies of the region it addressed, got {:?}",
        elsewhere.body
    );

    // A listing REPORTS: calling it again answers the same, and a read after it is
    // unchanged.
    let again = list(&emu, "").await;
    assert_eq!(
        listed_arns("the whole listing again", &again),
        listed_arns("the whole listing", &listed),
        "case the whole listing again: a listing must not change what a later caller sees, \
         got {:?}",
        again.body
    );
    let (first_row, first_arn) = in_region[0];
    let read_again = get_policy(&emu, first_arn).await;
    assert_eq!(
        read_again.status, 200,
        "case {:?}: the policy still reads back after two listings, got {} {:?}",
        first_row.name, read_again.status, read_again.body
    );
}

// ---------------------------------------------------------------------------------------
// 2. The order, and the pages that partition it.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn the_pages_partition_the_listing_in_creation_order() {
    // Seven policies whose creation instants are fixed in an order that is NOT the order
    // they were created in, so a listing that walked insertion or map order would report
    // another sequence. `offsets[i]` is how many seconds before the base instant policy `i`
    // was created.
    let offsets: [u64; 7] = [40, 10, 70, 20, 60, 30, 50];
    let emu = emulator();
    let base = SystemTime::now();
    let mut arns: Vec<(u64, String)> = Vec::new();
    for (index, offset) in offsets.iter().enumerate() {
        let arn = policy_arn_of(
            &emu,
            &format!("policy {index}"),
            json!({ "name": format!("paged policy {index}") }),
        )
        .await;
        set_created_at(&emu, &arn, base - Duration::from_secs(*offset));
        arns.push((*offset, arn));
    }
    // Oldest first: the largest offset is the earliest instant.
    arns.sort_by(|(left, _), (right, _)| right.cmp(left));
    let expected: Vec<String> = arns.iter().map(|(_, arn)| arn.clone()).collect();

    // Page sizes spanning the span `maxResults` admits around this listing: one row at a
    // time, a size that does not divide it, the whole listing exactly, one past it, and the
    // modelled bounds (`range 1..1000`).
    for page_size in [1_usize, 2, 3, expected.len(), expected.len() + 1, 1000] {
        let case = format!("maxResults={page_size}");
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let query = match &token {
                Some(token) => format!("maxResults={page_size}&nextToken={}", label(token)),
                None => format!("maxResults={page_size}"),
            };
            let page = list(&emu, &query).await;
            assert_eq!(
                page.status, 200,
                "case {case}: page {pages} must answer 200, got {} {:?}",
                page.status, page.body
            );
            let rows = listed_arns(&case, &page);
            let remaining = expected.len() - walked.len();
            assert_eq!(
                rows.len(),
                remaining.min(page_size),
                "case {case}: page {pages} must carry {} rows, got {} {:?}",
                remaining.min(page_size),
                rows.len(),
                page.body
            );
            walked.extend(rows);
            pages += 1;
            assert!(
                pages <= expected.len() + 1,
                "case {case}: the pages must terminate, walked {walked:?}"
            );
            match page.get("nextToken") {
                Value::Null => {
                    assert_eq!(
                        walked.len(),
                        expected.len(),
                        "case {case}: a page without a nextToken is the last one, so every \
                         policy must have been reported, got {walked:?}"
                    );
                    break;
                }
                Value::String(next) => {
                    assert!(
                        walked.len() < expected.len(),
                        "case {case}: a nextToken is issued only while rows remain, got \
                         {next:?} after {walked:?}"
                    );
                    token = Some(next);
                }
                other => panic!("case {case}: nextToken is modelled a string, got {other:?}"),
            }
        }
        assert_eq!(
            walked, expected,
            "case {case}: the pages must concatenate to the (createdAt, policyArn) order \
             with neither a gap nor a repeat"
        );
    }
}

// ---------------------------------------------------------------------------------------
// 3. The policyArn filter.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn the_policy_arn_filter_reports_the_revisions_of_that_policy() {
    // Rows differ in how many versions the policy has cut: none, one, several.
    let rows: [(&str, usize); 3] = [("no version", 0), ("one version", 1), ("three versions", 3)];

    let emu = emulator();
    for (case, version_count) in rows {
        let (arn, definition_hash) = policy_of(
            &emu,
            case,
            json!({
                "name": format!("filtered policy {version_count}"),
                "description": format!("{version_count} versions"),
                "policyDefinition": definition(1),
            }),
        )
        .await;
        // A policy created before its versions, so the draft is the oldest revision of it
        // whatever the clock did between the calls.
        set_created_at(&emu, &arn, SystemTime::now() - Duration::from_secs(60));
        let mut version_arns: Vec<String> = Vec::new();
        for number in 1..=version_count {
            let cut = create_version(&emu, &arn, &definition_hash).await;
            assert_eq!(
                cut.status, 200,
                "case {case:?}: CreateAutomatedReasoningPolicyVersion must answer 200, got {} \
                 {:?}",
                cut.status, cut.body
            );
            assert_eq!(
                cut.text("version"),
                number.to_string(),
                "case {case:?}: version numbers ascend from 1, got {:?}",
                cut.get("version")
            );
            version_arns.push(format!("{arn}:{number}"));
        }

        // The unversioned ARN names the policy, so its draft and every version cut from it
        // are in scope, oldest first.
        let filtered = list(&emu, &format!("policyArn={}", label(&arn))).await;
        assert_eq!(
            filtered.status, 200,
            "case {case:?}: a filtered listing must answer 200, got {} {:?}",
            filtered.status, filtered.body
        );
        let mut expected = vec![arn.clone()];
        expected.extend(version_arns.iter().cloned());
        assert_eq!(
            listed_arns(case, &filtered),
            expected,
            "case {case:?}: the filter reports the policy's draft and its {version_count} \
             version(s), got {:?}",
            filtered.body
        );
        for summary in summaries(case, &filtered) {
            let reported_arn = summary.get("policyArn").cloned().unwrap_or(Value::Null);
            let version = summary.get("version").cloned().unwrap_or(Value::Null);
            let expected_version = match reported_arn.as_str() {
                Some(text) if text == arn => json!(DRAFT),
                Some(text) => json!(text.rsplit(':').next().unwrap_or_default()),
                None => Value::Null,
            };
            assert_eq!(
                version, expected_version,
                "case {case:?}: the summary of {reported_arn:?} must report its own version, \
                 got {version:?}"
            );
            assert_eq!(
                summary.get("policyId").cloned().unwrap_or(Value::Null),
                json!(arn.rsplit('/').next().unwrap_or_default()),
                "case {case:?}: every revision carries the POLICY's policyId, got {:?}",
                summary.get("policyId")
            );
        }

        // A versioned ARN names exactly one revision.
        for (number, version_arn) in version_arns.iter().enumerate() {
            let one = list(&emu, &format!("policyArn={}", label(version_arn))).await;
            assert_eq!(
                one.status, 200,
                "case {case:?} version {}: a filtered listing must answer 200, got {} {:?}",
                number + 1,
                one.status,
                one.body
            );
            assert_eq!(
                listed_arns(case, &one),
                vec![version_arn.clone()],
                "case {case:?} version {}: a versioned ARN names exactly that revision, got \
                 {:?}",
                number + 1,
                one.body
            );
        }

        // And the UNFILTERED listing reports that policy's draft and none of its versions.
        let unfiltered = list(&emu, "").await;
        let reported = listed_arns(case, &unfiltered);
        assert!(
            reported.contains(&arn),
            "case {case:?}: the unfiltered listing must carry the policy's draft, got \
             {reported:?}"
        );
        for version_arn in &version_arns {
            assert!(
                !reported.contains(version_arn),
                "case {case:?}: only DRAFT versions are listed without a filter, but \
                 {version_arn:?} is in {reported:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------------------
// 4. An ARN naming no revision of this account.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn an_arn_naming_no_revision_of_this_account_is_not_found() {
    let emu = emulator();
    let arn = policy_arn_of(&emu, "setup", json!({ "name": "found policy" })).await;
    let id = arn
        .strip_prefix(&arn_prefix(REGION))
        .unwrap_or_else(|| panic!("case setup: {arn:?} is not under {:?}", arn_prefix(REGION)))
        .to_string();

    // Rows: which half of a well-formed `AutomatedReasoningPolicyArn` names nothing — the
    // policy id, the region, the id of an ARN that also carries the `:<version>` suffix the
    // member admits, or a version a policy this account DOES hold has not cut — paired with
    // the sentence THIS operation words that miss in, and with whether a read of the same
    // ARN is worded the same way.
    let rows: Vec<(&str, String, &str, bool)> = vec![
        (
            "an id nothing minted",
            format!("{}00000000000z", arn_prefix(REGION)),
            LISTING_POLICY_NOT_FOUND,
            false,
        ),
        (
            "the minted id in another region",
            format!("{}{id}", arn_prefix("eu-west-1")),
            LISTING_POLICY_NOT_FOUND,
            false,
        ),
        (
            "a versioned ARN of an id nothing minted",
            format!("{}00000000000z:3", arn_prefix(REGION)),
            LISTING_POLICY_NOT_FOUND,
            false,
        ),
        (
            "a version nothing cut",
            format!("{arn}:7"),
            POLICY_VERSION_NOT_FOUND,
            true,
        ),
    ];

    for (case, policy_arn, expected, read_words_it_the_same) in &rows {
        let filtered = list(&emu, &format!("policyArn={}", label(policy_arn))).await;
        assert_eq!(
            filtered.status, 404,
            "case {case:?}: {policy_arn:?} names no revision of this account, so the listing \
             must answer 404, got {} {:?}",
            filtered.status, filtered.body
        );
        assert_eq!(
            filtered.error.as_deref(),
            Some("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException, got {:?}",
            filtered.error
        );
        let message = filtered.text("message");
        assert_eq!(
            message, *expected,
            "case {case:?}: the listing must refuse the ARN with its own sentence \
             {expected:?}, got {message:?}"
        );
        // The sentence carries no part of the request: not the ARN it filtered on, not the
        // account inside that ARN.
        assert!(
            !message.contains(policy_arn.as_str()) && !message.contains(ACCOUNT),
            "case {case:?}: the sentence must carry neither the ARN nor the account, got \
             {message:?}"
        );
        // The listing and the read of one policy word the absent POLICY differently, and
        // the missing VERSION of a policy that IS there identically.
        let read = get_policy(&emu, policy_arn).await;
        let read_message = read.text("message");
        if *read_words_it_the_same {
            assert_eq!(
                read_message, message,
                "case {case:?}: the read must word this miss as the listing does \
                 ({message:?}), got {read_message:?}"
            );
        } else {
            assert_eq!(
                read_message, READ_POLICY_NOT_FOUND,
                "case {case:?}: the read keeps its own sentence {READ_POLICY_NOT_FOUND:?}, \
                 got {read_message:?}"
            );
            assert_ne!(
                read_message, message,
                "case {case:?}: the listing and the read word the absent policy differently"
            );
        }
    }

    // The policy that IS there still lists, so the rows above refused on the ARN rather
    // than on the store.
    let listed = list(&emu, &format!("policyArn={}", label(&arn))).await;
    assert_eq!(
        listed_arns("the minted ARN", &listed),
        vec![arn.clone()],
        "case the minted ARN: the filter must report that policy's draft, got {:?}",
        listed.body
    );
}

// ---------------------------------------------------------------------------------------
// 5. The pagination token.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn a_next_token_this_service_did_not_issue_is_refused() {
    let emu = emulator();
    for index in 0..3 {
        policy_arn_of(
            &emu,
            &format!("setup {index}"),
            json!({ "name": format!("token policy {index}") }),
        )
        .await;
    }

    // Rows: tokens `PaginationToken` (len 1..2048, `^\S*$`) admits that this service never
    // issued — a word, a bare number, a cursor whose instant half is too short, one whose
    // instant half is not decimal, and one naming no resource at all.
    let rows: [(&str, &str); 5] = [
        ("a word", "notacursor"),
        ("a bare number", "12345678901234567890"),
        ("too short an instant", "1234:arn:aws:bedrock"),
        (
            "a non-decimal instant",
            "0000000000000000000x:arn:aws:bedrock",
        ),
        ("no resource half", "00000000000000000000:"),
    ];

    for (case, token) in rows {
        let refused = list(&emu, &format!("nextToken={}", label(token))).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: {token:?} is no token this service issued, so it must be refused, \
             got {} {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error.as_deref(),
            Some("ValidationException"),
            "case {case:?}: expected ValidationException, got {:?}",
            refused.error
        );
    }

    // A token the listing itself issued resumes it.
    let first = list(&emu, "maxResults=1").await;
    let token = first.text("nextToken");
    let resumed = list(&emu, &format!("maxResults=1&nextToken={}", label(&token))).await;
    assert_eq!(
        resumed.status, 200,
        "case a token this listing issued: it must resume the listing, got {} {:?}",
        resumed.status, resumed.body
    );
    assert_ne!(
        listed_arns("a token this listing issued", &resumed),
        listed_arns("the first page", &first),
        "case a token this listing issued: the next page must start past the first one"
    );
}
