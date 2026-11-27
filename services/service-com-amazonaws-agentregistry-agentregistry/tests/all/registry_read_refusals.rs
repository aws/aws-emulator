// SPDX-License-Identifier: CC0-1.0

//! How the three Agent Registry discovery reads WORD a refusal, end-to-end over the
//! same `MultiServiceRouter::dispatch` the emulator serves on its socket.
//!
//! Three rules, each asserted over a set of inputs rather than one:
//!
//! 1. a request naming a registry this service does not hold is refused with
//!    `404 ResourceNotFoundException` and one sentence — the fixed text plus the
//!    registry identifier exactly as the request supplied it. The per-entry `errors`
//!    list of `BatchGetDiscoverableRegistryRecord` is for record ids a registry that
//!    EXISTS does not expose, which the contrast rows pin;
//! 2. no constraint clause quotes a value: each names the member it was stated at and
//!    the requirement that member's value had to satisfy, whatever the request carried
//!    — over a collection's `@length`, a scalar's `@pattern` and `@range`, a string's
//!    `@length` and an absent `@required` member;
//! 3. a constraint the ELEMENTS of a collection declare is stated at that MEMBER with
//!    the element shape's whole declared set nested inside the clause, and a `@pattern`
//!    inside or outside that set is quoted as the service advertises the expression:
//!    without the pair of whole-value anchors the model wraps it in;
//! 4. a registry-record filter narrows ONE dimension to ONE of the values that
//!    dimension has, so a `filters` entry carrying any other number of values, or a
//!    value the dimension it names does not have, is refused with
//!    `400 ValidationException` before the caller is authorized, before the registry is
//!    resolved and before any record is read — over both members of
//!    `RegistryRecordFilterName`, every value count the modelled list admits, every
//!    position in the filter list and whatever else the request carries;
//! 5. resolving the registry a request names fails in exactly two ways — the store holds
//!    no such registry, and this engine carries no such store — and each of them is
//!    answered by ONE modelled error at ONE status with ONE sentence, whichever of the
//!    three reads asked, over both failures, every identifier form and two engine types
//!    that carry no store.
//!
//! Two tests pin the other direction: a value AT each published bound is served, and a
//! registry that DOES resolve is served by every read — so the refusals above are about
//! values outside the model's bounds and about registries that did not resolve.
//!
//! Every bound, pattern and member path below comes from the model
//! (`smithy_query(query="closure", operation="BatchGetDiscoverableRegistryRecord,\
//! ListDiscoverableRegistryRecords,SearchDiscoverableRegistryRecords")`), cited per row.
//! The harness (`seeded_router`, `post`, `list_path`, `encode_label`) is the one
//! `tests/all/discovery_read_path.rs` already drives this service with.

use serde_json::{json, Value};
use service_com_amazonaws_agentregistry_agentregistry::ops::types::{
    RECORD_TYPE_VALUES, REGISTRY_RECORD_FILTER_NAME_VALUES,
};
use service_com_amazonaws_agentregistry_agentregistry::state::DESCRIPTOR_TYPE_KEYS;
use service_com_amazonaws_agentregistrycontrol_agentregistrycontrol::state as owner;
use smithy_server_runtime::MultiServiceRouter;

use crate::discovery_read_path::{
    encode_label, list_path, post, post_response, record_ids, seeded_router, EMPTY_REGISTRY,
    REGION, REGISTRY,
};

/// The fixed text every discovery read states for a registry it does not hold; the
/// identifier the request carried is the only variable part.
const NOT_FOUND_PREFIX: &str = "Registry not found: ";

/// `com.amazonaws.agentregistry#RegistryIdentifier`'s `@pattern` as the service
/// advertises it: the modelled expression
/// `^(arn:aws(-[^:]+)?:agent-registry:[a-z0-9-]+:[0-9]{12}:registry/)?[a-zA-Z0-9]{12,16}$`
/// without the pair of whole-value anchors the model wraps it in.
const REGISTRY_IDENTIFIER_PATTERN: &str =
    r"(arn:aws(-[^:]+)?:agent-registry:[a-z0-9-]+:[0-9]{12}:registry/)?[a-zA-Z0-9]{12,16}";

/// `com.amazonaws.agentregistry#RecordIdentifier`'s `@pattern` (the element shape of
/// `RegistryRecordsEntry$recordIds`), advertised the same way.
const RECORD_IDENTIFIER_PATTERN: &str = r"(arn:aws(-[^:]+)?:agent-registry:[a-z0-9-]+:[0-9]{12}:registry/[a-zA-Z0-9]{12,16}/record/)?[a-zA-Z0-9]{12}";

/// A record id that satisfies `RecordIdentifier` (12 alphanumerics), so a refusal of a
/// batch request is about the registry it names and nothing else.
const WELL_FORMED_RECORD_ID: &str = "rec000000001";

/// The `message` member of a refusal body.
fn stated(body: &Value) -> String {
    body["message"].as_str().unwrap_or_default().to_string()
}

/// The error code a refusal body carries (`__type` on restJson1).
fn error_code(body: &Value) -> String {
    body["__type"].as_str().unwrap_or_default().to_string()
}

/// The AWS envelope over one constraint clause.
fn one_error(clause: &str) -> String {
    format!("1 validation error detected: {clause}")
}

/// Every discovery read, as the request that reaches it for `registry`: the operation's
/// path and the body that names that registry and nothing else wrong.
fn reads_of(registry: &str) -> [(&'static str, String, Value); 3] {
    [
        (
            "ListDiscoverableRegistryRecords",
            list_path(registry),
            json!({}),
        ),
        (
            "SearchDiscoverableRegistryRecords",
            "/discoverable-records-search".to_string(),
            json!({ "searchQuery": "", "registryIds": [registry], "maxResults": 10 }),
        ),
        (
            "BatchGetDiscoverableRegistryRecord",
            "/discoverable-records-batch".to_string(),
            json!({ "entries": [
                { "registryId": registry, "recordIds": [WELL_FORMED_RECORD_ID] },
            ] }),
        ),
    ]
}

/// A registry identifier the model admits but this store does not hold is refused by
/// every one of the three reads, with the one sentence and the identifier as supplied.
#[tokio::test]
async fn a_registry_this_service_does_not_hold_is_refused_by_every_read() {
    let router = seeded_router();
    // Well-formed `RegistryIdentifier` values (bare id of 12, 14 and 16 characters —
    // the pattern's `[a-zA-Z0-9]{12,16}` run and both of its bounds — and the ARN form
    // in two partitions) that name no seeded registry.
    let absent: [(&str, &str); 5] = [
        ("bare id at the pattern's minimum length", "zzz999999999"),
        ("bare id of mixed case", "Zz9MissingXyz1"),
        ("bare id at the pattern's maximum length", "ZZ99missing00000"),
        (
            "registry ARN",
            "arn:aws:agent-registry:us-east-1:000000000000:registry/zzz999999999",
        ),
        (
            "registry ARN in another partition",
            "arn:aws-us-gov:agent-registry:us-gov-west-1:000000000000:registry/zzz999999999",
        ),
    ];

    for (form, identifier) in absent {
        for (operation, path, body) in reads_of(identifier) {
            let case = format!("{operation} with a {form}");
            let (status, response) = post(&router, &path, &body).await;
            assert_eq!(
                status, 404,
                "case {case:?}: expected 404, got {status}: {response}"
            );
            let code = error_code(&response);
            assert!(
                code.contains("ResourceNotFoundException"),
                "case {case:?}: expected ResourceNotFoundException, got {code:?}: {response}"
            );
            let expected = format!("{NOT_FOUND_PREFIX}{identifier}");
            let actual = stated(&response);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }
}

/// The contrast: a registry this store DOES hold is served, whatever it holds — the
/// 404 above is about the registry's existence and not about the records a request
/// asked for. `BatchGetDiscoverableRegistryRecord` reports those per entry, as its
/// `errors` list is modelled to.
#[tokio::test]
async fn a_registry_that_exists_is_served_and_missing_records_are_reported_per_entry() {
    let router = seeded_router();
    // (case, the registry an entry names, the ids it asks for, records returned, errors)
    let cases: [(&str, &str, Value, usize, usize); 4] = [
        (
            "a seeded registry, every id resolvable",
            REGISTRY,
            json!([WELL_FORMED_RECORD_ID]),
            1,
            0,
        ),
        (
            "a seeded registry, one id it does not expose",
            REGISTRY,
            json!([WELL_FORMED_RECORD_ID, "zzz999999999"]),
            1,
            1,
        ),
        (
            // Exists, holds no record whose status is discoverable.
            "a registry holding no discoverable record",
            EMPTY_REGISTRY,
            json!([WELL_FORMED_RECORD_ID]),
            0,
            1,
        ),
        (
            "the same registry named by its ARN",
            "arn:aws:agent-registry:us-east-1:000000000000:registry/abc123def456",
            json!([WELL_FORMED_RECORD_ID]),
            1,
            0,
        ),
    ];

    for (case, registry, record_id_list, records, errors) in cases {
        let (status, response) = post(
            &router,
            "/discoverable-records-batch",
            &json!({ "entries": [{ "registryId": registry, "recordIds": record_id_list }] }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status}: {response}"
        );
        assert_eq!(
            record_ids(case, &response).len(),
            records,
            "case {case:?}: expected {records} records, got {response}"
        );
        assert_eq!(
            response["errors"].as_array().map(Vec::len),
            Some(errors),
            "case {case:?}: expected {errors} per-entry errors, got {response}"
        );
    }

    // And every read answers 200 for a registry that exists but holds nothing
    // discoverable, so the refusal above cannot be read as "this registry is empty".
    for (operation, path, body) in reads_of(EMPTY_REGISTRY) {
        let (status, response) = post(&router, &path, &body).await;
        assert_eq!(
            status, 200,
            "case {operation:?}: an existing registry must be served, got {status}: {response}"
        );
    }
}

/// A collection member outside its modelled `@length` names the member and the bound
/// and quotes nothing: a list has no scalar rendering, so neither its element count nor
/// its elements reach the sentence.
#[tokio::test]
async fn a_collection_outside_its_length_quotes_nothing_of_the_collection() {
    let router = seeded_router();
    let eleven_filters: Vec<Value> = (0..11)
        .map(|_| json!({ "name": "recordType", "values": ["MCP"] }))
        .collect();
    // (case, path, body, the clause the envelope carries). Every bound is the model's:
    // `entries` and `registryIds` are `@length{min:1,max:1}`, `filters` is
    // `@length{max:10}`.
    let cases: [(&str, &str, Value, String); 5] = [
        (
            "entries over its maximum of one",
            "/discoverable-records-batch",
            json!({ "entries": [
                { "registryId": REGISTRY, "recordIds": [WELL_FORMED_RECORD_ID] },
                { "registryId": REGISTRY, "recordIds": [WELL_FORMED_RECORD_ID] },
            ] }),
            "Value at 'entries' failed to satisfy constraint: Member must have length less \
             than or equal to 1"
                .to_string(),
        ),
        (
            "entries below its minimum of one",
            "/discoverable-records-batch",
            json!({ "entries": [] }),
            "Value at 'entries' failed to satisfy constraint: Member must have length \
             greater than or equal to 1"
                .to_string(),
        ),
        (
            "registryIds over its maximum of one",
            "/discoverable-records-search",
            json!({ "searchQuery": "", "maxResults": 10,
                    "registryIds": [REGISTRY, "bulkreg00001"] }),
            "Value at 'registryIds' failed to satisfy constraint: Member must have length \
             less than or equal to 1"
                .to_string(),
        ),
        (
            "registryIds below its minimum of one",
            "/discoverable-records-search",
            json!({ "searchQuery": "", "maxResults": 10, "registryIds": [] }),
            "Value at 'registryIds' failed to satisfy constraint: Member must have length \
             greater than or equal to 1"
                .to_string(),
        ),
        (
            // A member the issue this suite closes did not cite: the same rule, a
            // different bound, on the other operation.
            "filters over its maximum of ten",
            &list_path(REGISTRY),
            json!({ "filters": eleven_filters }),
            "Value at 'filters' failed to satisfy constraint: Member must have length less \
             than or equal to 10"
                .to_string(),
        ),
    ];

    for (case, path, body, clause) in cases {
        let (status, response) = post(&router, path, &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status}: {response}"
        );
        let code = error_code(&response);
        assert!(
            code.contains("ValidationException"),
            "case {case:?}: expected ValidationException, got {code:?}: {response}"
        );
        let expected = one_error(&clause);
        let actual = stated(&response);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {expected:?}, got {actual:?}"
        );
    }
}

/// The same rule where the collection is nested inside a list element: the clause still
/// states the member and the bound and quotes no count.
/// `RegistryRecordsEntry$recordIds` is `@length{min:1,max:100}`.
#[tokio::test]
async fn a_nested_collection_outside_its_length_quotes_nothing_either() {
    let router = seeded_router();
    let hundred_and_one: Vec<String> = (1..=101).map(|n| format!("rec{n:09}")).collect();
    for (case, record_id_list, bound) in [
        ("recordIds over its maximum", json!(hundred_and_one), "less than or equal to 100"),
        ("recordIds below its minimum", json!([]), "greater than or equal to 1"),
    ] {
        let (status, response) = post(
            &router,
            "/discoverable-records-batch",
            &json!({ "entries": [{ "registryId": REGISTRY, "recordIds": record_id_list }] }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status}: {response}"
        );
        let actual = stated(&response);
        assert!(
            actual.starts_with("1 validation error detected: Value at '")
                && actual.ends_with(&format!(
                    "failed to satisfy constraint: Member must have length {bound}"
                )),
            "case {case:?}: expected the member and the bound with no value, got {actual:?}"
        );
        assert!(
            !actual.contains("items") && !actual.contains("Value '"),
            "case {case:?}: nothing of the collection may be quoted, got {actual:?}"
        );
    }
}

/// A value outside its member's `@pattern` is refused with the expression that member
/// admits, quoted without the whole-value anchors the model wraps it in, and with no
/// value quoted: the requirement is the expression, not the one value that missed it.
#[tokio::test]
async fn a_pattern_violation_states_the_unanchored_expression_and_quotes_no_value() {
    let router = seeded_router();
    // Values outside `RegistryIdentifier`'s pattern, reached through the `@httpLabel`
    // `registryId` of ListDiscoverableRegistryRecords.
    let bad_ids: [(&str, &str); 5] = [
        ("a space and a punctuation character", "bad id!"),
        ("shorter than the pattern's run", "short"),
        ("longer than the pattern's run", "abcdefghijklmnopqr"),
        ("a punctuation character inside the run", "abc123def456!"),
        (
            "an ARN whose registry segment is malformed",
            "arn:aws:agent-registry:us-east-1:000000000000:registry/nope",
        ),
    ];
    let expected = one_error(&format!(
        "Value at 'registryId' failed to satisfy constraint: Member must satisfy regular \
         expression pattern: {REGISTRY_IDENTIFIER_PATTERN}"
    ));

    for (case, bad_id) in bad_ids {
        let path = format!(
            "/registries/{}/discoverable-records-list",
            encode_label(bad_id)
        );
        let (status, response) = post(&router, &path, &json!({})).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status}: {response}"
        );
        let actual = stated(&response);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {expected:?}, got {actual:?}"
        );
        assert!(
            !actual.contains(bad_id)
                // The anchors the model wraps the expression in would show as the
                // first and last characters of the quoted expression; the `^` inside
                // `[^:]` is part of the expression itself.
                && !actual.contains("pattern: ^")
                && !actual.ends_with('$'),
            "case {case:?}: neither the refused value nor an anchor may reach the sentence, \
             got {actual:?}"
        );
    }
}

/// The same class on members the issue this suite closes did not cite: a constraint
/// broken by the ELEMENTS of a collection is stated at that MEMBER, carrying the whole
/// set of requirements its elements have — the element shape's declared set, not just
/// the constraint one value happened to break, since the set is a property of the model
/// rather than of the request.
#[tokio::test]
async fn a_constraint_its_elements_broke_is_stated_at_the_collection_with_its_whole_set() {
    let router = seeded_router();
    // (case, path, body, the member the clause names, the set its elements must satisfy).
    // Every element shape is the model's: `RegistryIdList` over `RegistryIdentifier`
    // (@length 1..2048, @pattern), `RegistryRecordIdList` over `RecordIdentifier`
    // (@length 1..2048, @pattern) and `DiscoverableFilterValues` over `FilterValue`
    // (@length 1..255, no pattern).
    let cases: [(&str, &str, Value, &str, String); 3] = [
        (
            "a registryIds element outside RegistryIdentifier's pattern",
            "/discoverable-records-search",
            json!({ "searchQuery": "", "maxResults": 10, "registryIds": ["bad id!"] }),
            "registryIds",
            format!(
                "Member must have length less than or equal to 2048, Member must have \
                 length greater than or equal to 1, Member must satisfy regular expression \
                 pattern: {REGISTRY_IDENTIFIER_PATTERN}"
            ),
        ),
        (
            // Declared inside `RegistryRecordsEntry`, so the clause is stated at the
            // nested member path the walk reported the element at.
            "a recordIds element outside RecordIdentifier's pattern",
            "/discoverable-records-batch",
            json!({ "entries": [{ "registryId": REGISTRY, "recordIds": ["bad id!"] }] }),
            "entries.1.member.recordIds",
            format!(
                "Member must have length less than or equal to 2048, Member must have \
                 length greater than or equal to 1, Member must satisfy regular expression \
                 pattern: {RECORD_IDENTIFIER_PATTERN}"
            ),
        ),
        (
            // An element shape that declares a `@length` and no pattern, so the set is
            // the two length requirements alone.
            "a filter values element shorter than FilterValue's minimum",
            "",
            json!({ "filters": [{ "name": "recordType", "values": [""] }] }),
            "filters.1.member.values",
            "Member must have length less than or equal to 255, Member must have length \
             greater than or equal to 1"
                .to_string(),
        ),
    ];

    for (case, path, body, member, set) in cases {
        // An empty path row addresses the list operation, whose path carries a label.
        let path = if path.is_empty() {
            list_path(REGISTRY)
        } else {
            path.to_string()
        };
        let (status, response) = post(&router, &path, &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status}: {response}"
        );
        let expected = one_error(&format!(
            "Value at '{member}' failed to satisfy constraint: Member must satisfy \
             constraint: [{set}]"
        ));
        let actual = stated(&response);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {expected:?}, got {actual:?}"
        );
    }
}

/// The rule the two tests above are instances of, over every OTHER constraint class
/// this model declares: a clause names the member and the requirement and quotes no
/// value, whatever the request carried — so the refusal a caller reads back is the same
/// for every value outside the same bound.
#[tokio::test]
async fn no_constraint_clause_quotes_the_value_the_request_carried() {
    let router = seeded_router();
    let search = "/discoverable-records-search";
    let list = list_path(REGISTRY);
    let over_long_query = "q".repeat(300);
    // (case, path, body, the clause the envelope carries). `maxResults` is
    // `@range{min:1,max:100}` on the list and `@range{min:1,max:50}` on the search;
    // `searchQuery` is `@length{min:0,max:256}` and `@sensitive`; `entries` is
    // `@required`.
    let cases: [(&str, &str, Value, &str); 6] = [
        (
            "list maxResults one past its maximum",
            &list,
            json!({ "maxResults": 101 }),
            "Value at 'maxResults' failed to satisfy constraint: Member must have value \
             less than or equal to 100",
        ),
        (
            "list maxResults one below its minimum",
            &list,
            json!({ "maxResults": 0 }),
            "Value at 'maxResults' failed to satisfy constraint: Member must have value \
             greater than or equal to 1",
        ),
        (
            "search maxResults one past its maximum",
            search,
            json!({ "searchQuery": "", "registryIds": [REGISTRY], "maxResults": 51 }),
            "Value at 'maxResults' failed to satisfy constraint: Member must have value \
             less than or equal to 50",
        ),
        (
            "search maxResults below its minimum",
            search,
            json!({ "searchQuery": "", "registryIds": [REGISTRY], "maxResults": -1 }),
            "Value at 'maxResults' failed to satisfy constraint: Member must have value \
             greater than or equal to 1",
        ),
        (
            "searchQuery past its maximum length",
            search,
            json!({ "searchQuery": over_long_query, "registryIds": [REGISTRY],
                    "maxResults": 10 }),
            "Value at 'searchQuery' failed to satisfy constraint: Member must have length \
             less than or equal to 256",
        ),
        (
            "entries absent altogether",
            "/discoverable-records-batch",
            json!({}),
            "Value at 'entries' failed to satisfy constraint: Member must not be null",
        ),
    ];

    for (case, path, body, clause) in cases {
        let (status, response) = post(&router, path, &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status}: {response}"
        );
        let expected = one_error(clause);
        let actual = stated(&response);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {expected:?}, got {actual:?}"
        );
        assert!(
            !actual.contains("Value '"),
            "case {case:?}: no clause may quote a value, got {actual:?}"
        );
    }
}

/// Each bound at its published edge is SERVED, so the refusals above are about values
/// outside the model's bounds and not about the members themselves.
#[tokio::test]
async fn a_value_at_a_published_bound_is_served() {
    let router = seeded_router();
    let cases: [(&str, String, Value); 5] = [
        (
            "list maxResults at its minimum",
            list_path(REGISTRY),
            json!({ "maxResults": 1 }),
        ),
        (
            "list maxResults at its maximum",
            list_path(REGISTRY),
            json!({ "maxResults": 100 }),
        ),
        (
            // The modelled maximum, which `maxResults`'s doc admits for a `KEYWORD`
            // search: "Valid values are 1 through 50 when searchType is KEYWORD".
            "search maxResults at its maximum",
            "/discoverable-records-search".to_string(),
            json!({ "searchQuery": "", "registryIds": [REGISTRY], "maxResults": 50,
                    "searchType": "KEYWORD" }),
        ),
        (
            // And at the maximum the same doc admits for the other two search types:
            // "1 through 20 when it is HYBRID or SEMANTIC".
            "search maxResults at the maximum of its default search type",
            "/discoverable-records-search".to_string(),
            json!({ "searchQuery": "", "registryIds": [REGISTRY], "maxResults": 20 }),
        ),
        (
            "recordIds at its minimum of one",
            "/discoverable-records-batch".to_string(),
            json!({ "entries": [
                { "registryId": REGISTRY, "recordIds": [WELL_FORMED_RECORD_ID] },
            ] }),
        ),
    ];

    for (case, path, body) in cases {
        let (status, response) = post(&router, &path, &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status}: {response}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 4: what a registry-record filter's own `values` may carry.
// ---------------------------------------------------------------------------

/// `ValidationExceptionReason`'s modelled default, which every `ValidationException` of
/// this service carries unless it states another
/// (`smithy_query(query="shape", shape="ValidationException")`).
const REASON_FIELD_VALIDATION_FAILED: &str = "FieldValidationFailed";

/// `DiscoverableFilterValues` is modelled `@length{min:1,max:10}`
/// (`smithy_query(query="shape", shape="DiscoverableFilterValues")`), so ten values is
/// the most one `filters` entry can carry before the modelled bound refuses it first.
const MOST_FILTER_VALUES: usize = 10;

/// `FilterValue` is modelled `@length{min:1,max:255}`, so a value of this length is at
/// the published maximum: it is refused for what it NAMES, not for its length.
const LONGEST_FILTER_VALUE: usize = 255;

/// `RegistryRecordFilterList` is modelled `@length{max:10}`, so this is the widest
/// position an offending filter can sit at.
const MOST_FILTERS: usize = 10;

/// The whole set of values the dimension `filter_name` narrows, from the model: the
/// `RecordType` enum for `recordType` (through this crate's generated catalogue of it),
/// and the descriptor entry keys the `Descriptors` structure models for `descriptorType`
/// — the values `DiscoverableRegistryRecordSummary$descriptorTypes` carries. Nothing
/// here spells a value set by hand.
pub(crate) fn dimension_values(filter_name: &str) -> Vec<&'static str> {
    match filter_name {
        "recordType" => RECORD_TYPE_VALUES.to_vec(),
        "descriptorType" => DESCRIPTOR_TYPE_KEYS.to_vec(),
        other => panic!("{other:?} is no member of RegistryRecordFilterName"),
    }
}

/// One value the dimension `filter_name` narrows, for the elements of a filter list
/// whose offending element sits somewhere else.
pub(crate) fn admissible_value(filter_name: &str) -> &'static str {
    dimension_values(filter_name)[0]
}

/// The sentence a filter carrying any number of values other than one is refused with:
/// fixed text whose only variable part is the filter name the request carried.
fn exactly_one_value(filter_name: &str) -> String {
    format!("Filter {filter_name} must specify exactly one value.")
}

/// The sentence a value the named dimension does not have is refused with: fixed text
/// plus the filter name and the offending value, both as the request carried them.
fn invalid_value(filter_name: &str, value: &str) -> String {
    format!("Invalid value for filter {filter_name}: {value}")
}

/// A `filters` entry naming `filter_name` and carrying `values`.
fn filter(filter_name: &str, values: &[&str]) -> Value {
    json!({ "name": filter_name, "values": values })
}

/// The list read of `registry` carrying `body` is refused with HTTP 400, the
/// `ValidationException` errortype both on the wire and in the body, the `reason` the
/// model defaults that error to, `expected` as its whole sentence — and no page.
async fn assert_filter_refused(
    router: &MultiServiceRouter,
    case: &str,
    registry: &str,
    body: &Value,
    expected: &str,
) {
    let response = post_response(router, &list_path(registry), body).await;
    let status = response.status().as_u16();
    let errortype = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let parsed: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    assert_eq!(
        status, 400,
        "case {case:?}: expected 400, got {status}: {parsed}"
    );
    assert!(
        errortype.contains("ValidationException"),
        "case {case:?}: expected the ValidationException errortype on the wire, got {errortype:?}"
    );
    let code = error_code(&parsed);
    assert!(
        code.contains("ValidationException"),
        "case {case:?}: expected a ValidationException, got {code:?}: {parsed}"
    );
    let actual = stated(&parsed);
    assert_eq!(
        actual, expected,
        "case {case:?}: expected {expected:?}, got {actual:?}"
    );
    let reason = parsed["reason"].as_str().unwrap_or_default();
    assert_eq!(
        reason, REASON_FIELD_VALIDATION_FAILED,
        "case {case:?}: expected reason {REASON_FIELD_VALIDATION_FAILED:?}, got {reason:?}"
    );
    assert!(
        parsed.get("registryRecords").is_none(),
        "case {case:?}: a refused read returns no page, got {parsed}"
    );
}

/// A filter narrows its dimension to ONE value, so a `values` list of any other length
/// the modelled list admits is refused — over both members of
/// `RegistryRecordFilterName`, every count from two up to the list's own maximum, and
/// values the dimension HAS, so the count is the only thing wrong.
#[tokio::test]
async fn a_filter_carrying_any_number_of_values_but_one_is_refused() {
    let router = seeded_router();
    for filter_name in REGISTRY_RECORD_FILTER_NAME_VALUES.iter().copied() {
        let values = dimension_values(filter_name);
        let expected = exactly_one_value(filter_name);
        for count in [2_usize, 3, MOST_FILTER_VALUES] {
            // Distinct values of the dimension, cycling once it has fewer than `count`.
            let carried: Vec<&str> = (0..count)
                .map(|index| values[index % values.len()])
                .collect();
            let case = format!("{filter_name} carrying {count} of its own values");
            let body = json!({ "filters": [filter(filter_name, &carried)] });
            assert_filter_refused(&router, &case, REGISTRY, &body, &expected).await;

            // The same count of ONE value repeated: repeating a value is still not
            // narrowing to one value.
            let repeated = vec![values[0]; count];
            let case = format!("{filter_name} carrying {count} copies of one value");
            let body = json!({ "filters": [filter(filter_name, &repeated)] });
            assert_filter_refused(&router, &case, REGISTRY, &body, &expected).await;
        }

        // One past the list's modelled maximum is refused by that bound before the
        // handler states a sentence of its own, so this row asserts the refusal only.
        let over_long = vec![values[0]; MOST_FILTER_VALUES + 1];
        let case = format!("{filter_name} carrying one value past the modelled maximum");
        let (status, response) = post(
            &router,
            &list_path(REGISTRY),
            &json!({ "filters": [filter(filter_name, &over_long)] }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status}: {response}"
        );
        let code = error_code(&response);
        assert!(
            code.contains("ValidationException"),
            "case {case:?}: expected a ValidationException, got {code:?}: {response}"
        );
    }
}

/// A filter's single value must be one the dimension it names HAS, whatever form the
/// offending value takes: a value of the other dimension, one of its own values in
/// another casing or with a suffix, a value of another enumerated type of this model, a
/// one-character value, and a value at `FilterValue`'s published maximum length.
#[tokio::test]
async fn a_filter_value_the_named_dimension_does_not_have_is_refused() {
    let router = seeded_router();
    let longest = "x".repeat(LONGEST_FILTER_VALUE);
    for filter_name in REGISTRY_RECORD_FILTER_NAME_VALUES.iter().copied() {
        let own = dimension_values(filter_name);
        let other_dimension = REGISTRY_RECORD_FILTER_NAME_VALUES
            .iter()
            .copied()
            .find(|name| *name != filter_name)
            .expect("RegistryRecordFilterName has another member");
        let candidates: Vec<(String, String)> = vec![
            (
                "a value of the other dimension".to_string(),
                admissible_value(other_dimension).to_string(),
            ),
            (
                "one of its own values lowercased".to_string(),
                own[0].to_lowercase(),
            ),
            (
                "one of its own values uppercased".to_string(),
                own[0].to_uppercase(),
            ),
            (
                "one of its own values with a suffix".to_string(),
                format!("{}s", own[0]),
            ),
            (
                // `RegistryRecordStatus` is another enumerated type of this model, and
                // no member of either dimension.
                "a value of another enumerated type of this model".to_string(),
                "APPROVED".to_string(),
            ),
            ("a one-character value".to_string(), "x".to_string()),
            (
                "a value at FilterValue's maximum length".to_string(),
                longest.clone(),
            ),
        ];

        for (form, value) in candidates
            .into_iter()
            // A casing variant that happens to BE one of the dimension's own values is
            // no offender, so it is not a case.
            .filter(|(_, value)| !own.contains(&value.as_str()))
        {
            let case = format!("{filter_name} given {form}");
            let body = json!({ "filters": [filter(filter_name, &[value.as_str()])] });
            assert_filter_refused(&router, &case, REGISTRY, &body, &invalid_value(filter_name, &value))
                .await;
        }
    }
}

/// Both refusals read alike at EVERY position the filter list admits, so the sentence is
/// about the offending filter and not about where it sits: one offender walked across a
/// full-length list, every other element a filter the service applies.
#[tokio::test]
async fn a_filter_refusal_reads_alike_at_every_position_the_list_admits() {
    let router = seeded_router();
    for filter_name in REGISTRY_RECORD_FILTER_NAME_VALUES.iter().copied() {
        let admissible = admissible_value(filter_name);
        let outside = "no-such-value";
        // (what the offending element carries, the sentence it is refused with)
        let offenders: [(Vec<&str>, String); 2] = [
            (vec![admissible, admissible], exactly_one_value(filter_name)),
            (vec![outside], invalid_value(filter_name, outside)),
        ];
        for (carried, expected) in offenders {
            for position in 1..=MOST_FILTERS {
                let filters: Vec<Value> = (1..=MOST_FILTERS)
                    .map(|at| {
                        if at == position {
                            filter(filter_name, &carried)
                        } else {
                            filter(filter_name, &[admissible])
                        }
                    })
                    .collect();
                let case = format!(
                    "{filter_name} offending at position {position} of {MOST_FILTERS}: {carried:?}"
                );
                let body = json!({ "filters": filters });
                assert_filter_refused(&router, &case, REGISTRY, &body, &expected).await;
            }
        }
    }
}

/// Whether a filter is well formed is a property of the REQUEST: the same sentence
/// whatever else the read carries — the page members at their published bounds, a
/// resumption token this service never minted, either admitted form of the registry
/// identifier — and whatever the store holds for the registry named there, including a
/// registry holding no discoverable record and one the store does not hold at all, which
/// a 404 would otherwise pre-empt.
#[tokio::test]
async fn a_filter_is_refused_before_the_registry_is_resolved_or_a_record_is_read() {
    let router = seeded_router();
    let filter_name = "recordType";
    let offending = json!([filter(filter_name, &["MCP", "AGENT"])]);
    let expected = exactly_one_value(filter_name);
    // A `RegistryIdentifier` the model admits that the store does not hold.
    let absent_registry = "zzz999999999";
    let arn_form = owner::registry_arn(REGION, REGISTRY);
    // (case, the registry the path names, the body)
    let cases: [(&str, &str, Value); 6] = [
        (
            "the offending filter alone",
            REGISTRY,
            json!({ "filters": offending }),
        ),
        (
            "beside maxResults at its published minimum",
            REGISTRY,
            json!({ "filters": offending, "maxResults": 1 }),
        ),
        (
            "beside maxResults at its published maximum",
            REGISTRY,
            json!({ "filters": offending, "maxResults": 100 }),
        ),
        (
            "beside a token this service never minted",
            REGISTRY,
            json!({ "filters": offending, "nextToken": "cGFnZQ" }),
        ),
        (
            "a registry that holds no discoverable record",
            EMPTY_REGISTRY,
            json!({ "filters": offending }),
        ),
        (
            "a registry the store does not hold",
            absent_registry,
            json!({ "filters": offending }),
        ),
    ];
    for (case, registry, body) in cases {
        assert_filter_refused(&router, case, registry, &body, &expected).await;
    }

    // The same read addressed by the identifier's ARN form.
    assert_filter_refused(
        &router,
        "the registry named by its ARN",
        &arn_form,
        &json!({ "filters": offending }),
        &expected,
    )
    .await;
}

/// The contrast, and what the refusals above are about: EVERY value the filtered
/// dimension has is served as a single-value filter — including the values this suite's
/// store holds no record for — and the page it returns carries only records that value
/// matches.
#[tokio::test]
async fn every_value_the_filtered_dimension_has_is_served_and_narrows_the_page() {
    let router = seeded_router();
    for filter_name in REGISTRY_RECORD_FILTER_NAME_VALUES.iter().copied() {
        for value in dimension_values(filter_name) {
            let case = format!("{filter_name}={value}");
            let (status, body) = post(
                &router,
                &list_path(REGISTRY),
                &json!({ "filters": [filter(filter_name, &[value])] }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200, got {status}: {body}"
            );
            let ids = record_ids(&case, &body);
            let records = body["registryRecords"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            for record in &records {
                if filter_name == "recordType" {
                    assert_eq!(
                        record["recordType"].as_str().unwrap_or_default(),
                        value,
                        "case {case:?}: every record of the page must match the filter, got {record}"
                    );
                } else {
                    let keys: Vec<String> = record["descriptorTypes"]
                        .as_array()
                        .cloned()
                        .unwrap_or_default()
                        .iter()
                        .map(|key| key.as_str().unwrap_or_default().to_string())
                        .collect();
                    assert!(
                        keys.iter().any(|key| key == value),
                        "case {case:?}: every record of the page must carry the descriptor key, got {keys:?} in {ids:?}"
                    );
                }
            }
        }
    }

    // One filter per dimension, each at one value: both are applied together.
    let filters: Vec<Value> = REGISTRY_RECORD_FILTER_NAME_VALUES
        .iter()
        .copied()
        .map(|filter_name| filter(filter_name, &[admissible_value(filter_name)]))
        .collect();
    let case = "one single-value filter per dimension";
    let (status, body) = post(
        &router,
        &list_path(REGISTRY),
        &json!({ "filters": filters }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200, got {status}: {body}"
    );
}

/// An engine the Agent Registry store cannot be resolved in at all: every registry and
/// record a discovery read reports is the CONTROL plane's resource, held in its slot of
/// the emulator's in-memory engine, so a service mounted on any other engine type can
/// resolve no registry. That is the second way resolving a registry fails, and it is
/// reachable no other way.
#[derive(Debug)]
struct ForeignEngine;

/// A second, unrelated engine type, so the agreement asserted below cannot be keyed to
/// one shape of foreign engine.
#[derive(Debug)]
struct ForeignEngineWithState {
    #[allow(dead_code)]
    label: String,
}

/// This service mounted on `engine`, behind the same router the emulator serves it on.
fn mounted_on<Eng: Send + Sync + 'static>(engine: Eng) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_agentregistry_agentregistry::EmulatedService::new(
            std::sync::Arc::new(engine),
        ),
    );
    router
}

/// What one read answered: the HTTP status, the error code and the sentence.
async fn answer_of(
    router: &MultiServiceRouter,
    path: &str,
    body: &Value,
) -> (u16, String, String) {
    let (status, response) = post(router, path, body).await;
    (status, error_code(&response), stated(&response))
}

/// RULE: resolving the registry a request names fails in exactly two ways — the store
/// holds no such registry, and this engine carries no such store — and each of them is
/// answered by ONE modelled error at ONE status with ONE sentence, whichever of the three
/// discovery reads is the one that asked.
///
/// The rows differ along which read asked (all three, each of which resolved its registry
/// in its own handler before), which failure resolving it hit, which form of
/// `RegistryIdentifier` the request named it by (bare id at both bounds of the pattern's
/// `[a-zA-Z0-9]{12,16}` run, and the ARN form in two partitions) and — for the engine
/// condition — which engine type the service was mounted on.
///
/// The statuses and codes are the modelled ones: `ResourceNotFoundException` is
/// `httpError` 404 and `InternalServerException` is `httpError` 500
/// (`smithy_query(query="operation",
/// operation="BatchGetDiscoverableRegistryRecord,ListDiscoverableRegistryRecords,\
/// SearchDiscoverableRegistryRecords")`). Which part of the request may reach the
/// sentence follows from what the condition is a property of: the identifier for a
/// registry the store does not hold (the caller reads its own value back), and nothing of
/// the request for a store that cannot be reached.
#[tokio::test]
async fn every_read_states_a_registry_that_does_not_resolve_one_way() {
    // (the form of `RegistryIdentifier`, a value in that form naming no seeded registry)
    let identifiers: [(&str, &str); 4] = [
        ("bare id at the pattern's minimum length", "zzz999999999"),
        ("bare id at the pattern's maximum length", "ZZ99missing00000"),
        (
            "registry ARN",
            "arn:aws:agent-registry:us-east-1:000000000000:registry/zzz999999999",
        ),
        (
            "registry ARN in another partition",
            "arn:aws-us-gov:agent-registry:us-gov-west-1:000000000000:registry/zzz999999999",
        ),
    ];

    // (the condition, the router serving it, the modelled status and code it is answered
    //  with, whether the identifier the request named is part of the sentence)
    let conditions: [(&str, MultiServiceRouter, u16, &str, bool); 3] = [
        (
            "a registry the store does not hold",
            seeded_router(),
            404,
            "ResourceNotFoundException",
            true,
        ),
        (
            "an engine carrying no Agent Registry store",
            mounted_on(ForeignEngine),
            500,
            "InternalServerException",
            false,
        ),
        (
            "an engine of another shape carrying no Agent Registry store",
            mounted_on(ForeignEngineWithState {
                label: "no Agent Registry store".to_string(),
            }),
            500,
            "InternalServerException",
            false,
        ),
    ];

    for (condition, router, expected_status, expected_code, states_identifier) in conditions {
        for (form, identifier) in identifiers {
            let mut stated_by: Vec<(&str, String)> = Vec::new();
            for (operation, path, body) in reads_of(identifier) {
                let case = format!("{operation} on {condition}, named by a {form}");
                let (status, code, message) = answer_of(&router, &path, &body).await;
                assert_eq!(
                    status, expected_status,
                    "case {case:?}: expected {expected_status}, got {status} ({code:?}: \
                     {message:?})"
                );
                assert!(
                    code.contains(expected_code),
                    "case {case:?}: expected {expected_code}, got {code:?}: {message:?}"
                );
                assert_eq!(
                    message.contains(identifier),
                    states_identifier,
                    "case {case:?}: expected the identifier stated in the sentence: \
                     {states_identifier}, got {message:?}"
                );
                assert!(
                    !message.is_empty(),
                    "case {case:?}: expected a sentence, got {message:?}"
                );
                stated_by.push((operation, message));
            }
            let (first_read, first_sentence) = stated_by
                .first()
                .cloned()
                .expect("every condition is asked of the three reads");
            for (operation, message) in &stated_by {
                assert_eq!(
                    message, &first_sentence,
                    "case {condition:?} named by a {form:?}: {operation} words it \
                     {message:?} where {first_read} words it {first_sentence:?}"
                );
            }
        }
    }
}

/// The contrast, so the agreement above is about a registry that did not resolve and not
/// about the reads answering alike whatever happens: on the engine that DOES carry the
/// store, every one of the three reads serves the registries it holds.
#[tokio::test]
async fn a_registry_that_resolves_is_served_by_every_read() {
    let router = seeded_router();
    for registry in [REGISTRY, EMPTY_REGISTRY] {
        for (operation, path, body) in reads_of(registry) {
            let case = format!("{operation} on {registry}");
            let (status, code, message) = answer_of(&router, &path, &body).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200, got {status} ({code:?}: {message:?})"
            );
        }
    }
}
