// SPDX-License-Identifier: CC0-1.0

//! How `DescribeLogGroups` CONTINUES a listing, and how it treats the substring pattern a
//! listing selects with.
//!
//! Every case drives the service the way a client does — `POST /` with
//! `X-Amz-Target: Logs_20140328.DescribeLogGroups` through the router that mounts it — so a
//! response that is not computed from the shared store cannot pass.
//!
//! The rules under test:
//!
//! * **A page hands back a `nextToken` reporting where the read STOPPED.** For a selection
//!   the service addresses directly — a name prefix, an identifier list, a class, or no
//!   filter at all — that is the position of the last group it reported while selected groups
//!   remain UNREPORTED, so the page that reported the last match carries none and its absence
//!   is how a client learns the enumeration is finished; walking the tokens covers the
//!   selected groups exactly once, in name order, in as many pages as the page size divides
//!   the selection into, never an extra round trip for a set that is already complete. A
//!   `logGroupNamePattern` selection is a substring WALK over the account's log groups
//!   instead — no index over log group names serves a substring — so every page whose walk
//!   examined a group is resumable from the group it stopped on, and only the page whose walk
//!   is past every log group of the account finishes the enumeration.
//! * **`logGroupNamePattern` is LITERAL TEXT for a case-sensitive substring search**, not an
//!   expression: a value carrying a character a log group name can never hold matches no log
//!   group, which is a success whose `logGroups` is the EMPTY LIST rather than a refusal. The
//!   member's length is still a bound on what the service takes.
//!
//! The tables span what the rules generalise over — how the groups were selected, how many
//!   of the account's groups match (none, one, several, all), WHERE those matches sit in the
//!   account's name order (first, middle, last), the page size, and which character outside a
//!   log group name's own class the pattern carries — rather than one recorded query.

use serde_json::{json, Value};

use crate::harness::{error_code, Logs};

/// The operation under test.
const DESCRIBE: &str = "DescribeLogGroups";

/// The one client error this operation declares.
const INVALID_PARAMETER: &str = "InvalidParameterException";

/// The longest `logGroupNamePattern` the member admits, from `LogGroupNamePattern`'s
/// `@length(min: 0, max: 512)`
/// (`smithy_query(query="shape",shape="LogGroupNamePattern")`). Stated here independently of
/// the crate under test.
const PATTERN_MAX_LEN: usize = 512;

/// The log groups every case is answered from, in the ASCII order the listing reports them in
/// ("The results are ASCII-sorted by log group name").
const GROUPS: [&str; 3] = [
    "/aws/lambda/beta",
    "/aws/opensearch/domains/alpha/audit-logs",
    "alpha-app-logs",
];

/// The names one page reports.
fn names(case: &str, page: &Value) -> Vec<String> {
    page.get("logGroups")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: a served listing reports logGroups, got {page}"))
        .iter()
        .map(|group| {
            group
                .get("logGroupName")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {case:?}: every group carries a name, got {group}"))
                .to_string()
        })
        .collect()
}

/// Whether a request selects its log groups by SUBSTRING SEARCH, which the service answers by
/// walking the account's log groups, rather than by a selection it addresses directly.
fn is_substring_search(selector: &Value) -> bool {
    selector.get("logGroupNamePattern").is_some()
}

/// Follow `token` to the end of the walk, reporting every log group name the pages past it
/// carry. The walk ends on the page that carries no token.
async fn drained(
    logs: &Logs,
    case: &str,
    selector: &Value,
    limit: usize,
    token: &str,
) -> Vec<String> {
    let mut token = Some(token.to_string());
    let mut left: Vec<String> = Vec::new();
    let mut pages = 0usize;
    while let Some(next) = token.take() {
        pages += 1;
        assert!(
            pages <= GROUPS.len() + 2,
            "case {case:?}: the walk must terminate, got {pages} pages past the first"
        );
        let mut request = selector.clone();
        request["limit"] = json!(limit);
        request["nextToken"] = json!(next);
        let page = logs.ok(DESCRIBE, &request).await;
        left.extend(names(case, &page));
        token = page
            .get("nextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
    }
    left
}

/// The groups of [`GROUPS`], created.
async fn account_with_groups(logs: &Logs) {
    for group in GROUPS {
        logs.ok("CreateLogGroup", &json!({"logGroupName": group}))
            .await;
    }
}

/// Whether a page carries a token, over a grid of selection sizes and page sizes (page sizes
/// that divide the selection exactly included): for a selection the service addresses
/// directly, exactly while selected groups remain unreported, so the walk ends on the page
/// that reported the last match; for a substring search, on every page that reported a group,
/// so the walk only ends once it is past every log group of the account. Either walk covers
/// the selection exactly once, in name order.
#[tokio::test]
async fn a_page_hands_back_a_continuation_exactly_while_groups_remain() {
    let logs = Logs::new();
    account_with_groups(&logs).await;

    // (case, the selector, the names it selects in order)
    let selections: Vec<(&str, Value, Vec<&str>)> = vec![
        ("the whole account", json!({}), GROUPS.to_vec()),
        (
            "one group under a prefix",
            json!({"logGroupNamePrefix": "alpha"}),
            vec![GROUPS[2]],
        ),
        (
            "two groups under a prefix",
            json!({"logGroupNamePrefix": "/aws/"}),
            vec![GROUPS[0], GROUPS[1]],
        ),
        (
            "a substring pattern selecting one",
            json!({"logGroupNamePattern": "audit"}),
            vec![GROUPS[1]],
        ),
        (
            "a selection that matches nothing",
            json!({"logGroupNamePattern": "nothing-matches-this"}),
            vec![],
        ),
    ];

    for (case, selector, selected) in &selections {
        let scan = is_substring_search(selector);
        // Page sizes from one group per page up to one past the whole selection, so every row
        // is walked with a size that divides it exactly (the page that reports the last match
        // is FULL) as well as with sizes that leave a remainder.
        for limit in 1_usize..=selected.len().saturating_add(1).max(2) {
            let case = format!("{case}, limit {limit}");
            let mut walked: Vec<String> = Vec::new();
            let mut token: Option<String> = None;
            let mut pages = 0usize;
            loop {
                pages += 1;
                assert!(
                    pages <= GROUPS.len() + selected.len() + 2,
                    "case {case:?}: the walk must terminate, got {pages} pages"
                );
                let mut request = selector.clone();
                request["limit"] = json!(limit);
                if let Some(token) = &token {
                    request["nextToken"] = json!(token);
                }
                let page = logs.ok(DESCRIBE, &request).await;
                let reported = names(&case, &page);
                assert!(
                    reported.len() <= limit,
                    "case {case:?}: a page holds at most {limit} groups, got {page}"
                );
                walked.extend(reported.clone());
                let next = page.get("nextToken").and_then(Value::as_str);
                if scan {
                    // A substring search reports where its walk stopped: a page that reported
                    // a group walked over at least that group, so it is resumable, and the
                    // walk only ends once it is past every group of the account — which is a
                    // page that reports nothing.
                    assert!(
                        next.is_some() || reported.is_empty(),
                        "case {case:?}: a page that reported a group is resumable, got {page}"
                    );
                } else {
                    assert_eq!(
                        next.is_some(),
                        walked.len() < selected.len(),
                        "case {case:?}: a page carries a token exactly while selected groups \
                         remain unreported ({} of {} reported so far), got {page}",
                        walked.len(),
                        selected.len()
                    );
                }
                match next {
                    Some(next) => token = Some(next.to_string()),
                    None => break,
                }
            }
            assert_eq!(
                walked,
                selected
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<String>>(),
                "case {case:?}: the walk covers the selection once, in name order"
            );
            if scan {
                // A walk examines each of the account's groups once, so it cannot take more
                // pages than there are groups to examine, plus the page that examines none.
                assert!(
                    pages <= GROUPS.len() + 1,
                    "case {case:?}: a walk over {} group(s) takes at most {} page(s), got \
                     {pages}",
                    GROUPS.len(),
                    GROUPS.len() + 1
                );
            } else {
                // No round trip beyond the pages the selection itself fills: a selection of
                // `n` groups read `limit` at a time is exactly ceil(n / limit) pages, and an
                // empty selection is the one page that reports nothing.
                let expected_pages = selected.len().div_ceil(limit).max(1);
                assert_eq!(
                    pages, expected_pages,
                    "case {case:?}: expected {expected_pages} page(s) for {} group(s), got \
                     {pages}",
                    selected.len()
                );
            }
        }
    }
}

/// Where the page that reported the last selected group leaves the enumeration: for a
/// selection the service addresses directly it carries NO token, whether that page is full or
/// short; for a substring search it carries one, and the pages past it report no further group
/// before the walk ends. Either way a page with selected groups still behind it carries a
/// token that resumes at exactly those groups.
///
/// The rows span the members this listing selects with (none, the name prefix, the substring
/// pattern, the identifier list and the log group class), because the rule is about how the
/// selection was made rather than about which filter left what.
#[tokio::test]
async fn the_page_that_reports_the_last_selected_group_ends_an_addressed_listing() {
    let logs = Logs::new();
    account_with_groups(&logs).await;

    // (case, the selector, the names it selects in order)
    let selections: Vec<(&str, Value, Vec<&str>)> = vec![
        ("every group of the account", json!({}), GROUPS.to_vec()),
        (
            "the groups under a name prefix",
            json!({"logGroupNamePrefix": "/aws/"}),
            vec![GROUPS[0], GROUPS[1]],
        ),
        (
            "the groups holding a substring",
            json!({"logGroupNamePattern": "alpha"}),
            vec![GROUPS[1], GROUPS[2]],
        ),
        (
            "the groups an identifier list names",
            json!({"logGroupIdentifiers": [GROUPS[0], GROUPS[2]]}),
            vec![GROUPS[0], GROUPS[2]],
        ),
        (
            "the groups of one class",
            json!({"logGroupClass": "STANDARD"}),
            GROUPS.to_vec(),
        ),
    ];

    for (case, selector, selected) in &selections {
        let count = selected.len();
        let scan = is_substring_search(selector);
        // A page sized exactly to the selection reports all of it. For a selection the service
        // addresses directly that is the end of the enumeration, and a token would promise a
        // set that does not exist; a substring search can only report where its walk stopped,
        // so the same page is resumable and the pages past it report nothing more.
        let mut exact = selector.clone();
        exact["limit"] = json!(count);
        let page = logs.ok(DESCRIBE, &exact).await;
        assert_eq!(
            names(case, &page),
            selected
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<String>>(),
            "case {case:?}: a page sized to the selection reports all of it, got {page}"
        );
        let token = page
            .get("nextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        match (scan, &token) {
            (false, token) => assert_eq!(
                *token, None,
                "case {case:?}: the page that reported the last of {count} group(s) carries no \
                 token, got {page}"
            ),
            (true, None) => panic!(
                "case {case:?}: a substring search reports where its walk stopped, so the page \
                 that reported the last of {count} group(s) is resumable, got {page}"
            ),
            (true, Some(token)) => {
                let left = drained(&logs, case, selector, count, token).await;
                assert_eq!(
                    left,
                    Vec::<String>::new(),
                    "case {case:?}: the pages past the last match report no further group, got \
                     {left:?}"
                );
            }
        }

        // A page one group short of the selection DOES carry a token, and continuing it
        // reports exactly the group left over — the end of an addressed listing, and the last
        // match of a substring search's walk.
        let mut short = selector.clone();
        short["limit"] = json!(count - 1);
        let page = logs.ok(DESCRIBE, &short).await;
        let token = page
            .get("nextToken")
            .and_then(Value::as_str)
            .unwrap_or_else(|| {
                panic!("case {case:?}: a page leaving a group unreported carries a token, got {page}")
            })
            .to_string();
        let mut resumed = selector.clone();
        resumed["limit"] = json!(count);
        resumed["nextToken"] = json!(token);
        let page = logs.ok(DESCRIBE, &resumed).await;
        assert_eq!(
            names(case, &page),
            vec![selected[count - 1].to_string()],
            "case {case:?}: the continuation reports exactly the group left over, got {page}"
        );
        match page.get("nextToken").and_then(Value::as_str) {
            None => assert!(
                !scan,
                "case {case:?}: a substring search's page that reported a group is resumable, \
                 got {page}"
            ),
            Some(token) => {
                assert!(
                    scan,
                    "case {case:?}: the continuation that reported the last group carries no \
                     token, got {page}"
                );
                let left = drained(&logs, case, selector, count, token).await;
                assert_eq!(
                    left,
                    Vec::<String>::new(),
                    "case {case:?}: the pages past the last match report no further group, got \
                     {left:?}"
                );
            }
        }
    }
}

/// Every member BESIDES the literal-text pattern is still judged by its own constraint, so the
/// relaxation above is ONE member wide: a value outside a length bound, a numeric range, a
/// character class or an enum's value set is still refused, with that constraint stated.
#[tokio::test]
async fn every_other_selector_is_still_judged_by_its_own_constraint() {
    let logs = Logs::new();
    account_with_groups(&logs).await;

    // (case, the request, the constraint clause the refusal states)
    let refused: Vec<(&str, Value, &str)> = vec![
        (
            "a page limit below the range",
            json!({"limit": 0}),
            "Member must have value greater than or equal to 1",
        ),
        (
            "a page limit above the range",
            json!({"limit": 51}),
            "Member must have value less than or equal to 50",
        ),
        (
            "a zero-length continuation token",
            json!({"nextToken": ""}),
            "Member must have length greater than or equal to 1",
        ),
        (
            "an empty identifier list",
            json!({"logGroupIdentifiers": []}),
            "Member must have length greater than or equal to 1",
        ),
        (
            "a log group class the enum does not hold",
            json!({"logGroupClass": "ARCHIVE"}),
            "Member must satisfy enum value set: [DELIVERY, ESSENTIALS, INFREQUENT_ACCESS, STANDARD]",
        ),
        (
            "a name prefix carrying a character a log group name cannot hold",
            json!({"logGroupNamePrefix": "alpha*"}),
            r"Member must satisfy regular expression pattern: [\.\-_/#A-Za-z0-9]+",
        ),
        (
            "an identifier carrying a character an identifier cannot hold",
            json!({"logGroupIdentifiers": ["alpha app logs"]}),
            r"Member must satisfy regular expression pattern: [\w#+=/:,.@-]*",
        ),
    ];
    for (case, request, clause) in &refused {
        let (status, parsed) = logs.call(DESCRIBE, request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: {request} must be refused, got {status} {parsed}"
        );
        assert_eq!(
            error_code(&parsed),
            INVALID_PARAMETER,
            "case {case:?}: expected {INVALID_PARAMETER}, got {parsed}"
        );
        let message = parsed
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case:?}: a refusal carries a message, got {parsed}"));
        assert!(
            message.contains(clause),
            "case {case:?}: expected the clause {clause:?}, got {message:?}"
        );
    }

    // The same members INSIDE their constraints are served, so the rows above are refused for
    // the value they carry rather than for the member they name.
    let served: Vec<(&str, Value)> = vec![
        ("the smallest page limit", json!({"limit": 1})),
        ("the largest page limit", json!({"limit": 50})),
        (
            "the one admitted class of these groups",
            json!({"logGroupClass": "STANDARD"}),
        ),
        (
            "a name prefix inside the class",
            json!({"logGroupNamePrefix": "alpha"}),
        ),
        (
            "an identifier inside the class",
            json!({"logGroupIdentifiers": [GROUPS[2]]}),
        ),
    ];
    for (case, request) in &served {
        let page = logs.ok(DESCRIBE, request).await;
        assert!(
            page.get("logGroups").and_then(Value::as_array).is_some(),
            "case {case:?}: a served listing reports logGroups, got {page}"
        );
    }
}

/// A `logGroupNamePattern` is matched as literal text: a value carrying a character a log
/// group name can never hold selects nothing and is SERVED with an empty list, and a value
/// that is a substring of a name selects exactly the names holding it.
#[tokio::test]
async fn a_pattern_that_cannot_match_is_an_empty_page_and_not_a_refusal() {
    let logs = Logs::new();
    account_with_groups(&logs).await;

    // Each row carries a character outside the class a log group name is built from —
    // `[\.\-_/#A-Za-z0-9]`, the expression this service publishes for `LogGroupName`
    // (`smithy_query(query="shape",shape="LogGroupName")`) — so no name can ever hold it.
    // (case, the pattern)
    let unmatchable: Vec<(&str, String)> = vec![
        ("a trailing wildcard", "alpha*".to_string()),
        ("a question mark", "alpha?logs".to_string()),
        ("a space", "alpha app logs".to_string()),
        (
            "an ARN, which carries colons",
            "arn:aws:logs:eu-west-2:000000000000:log-group:alpha-app-logs".to_string(),
        ),
        (
            "alternatives separated by a pipe",
            "audit-logs|app-logs".to_string(),
        ),
        ("a braced placeholder", "{domain}-app-logs".to_string()),
        ("a plus", "alpha+logs".to_string()),
        // The one row inside the class: the search is case-SENSITIVE, so a name's own text in
        // another case matches nothing either.
        ("the same text in another case", "AUDIT".to_string()),
        // One character past the published length bound is refused, not served, so the bound
        // itself is a served row and its neighbour a refusal (below).
        ("the longest admitted pattern", "z".repeat(PATTERN_MAX_LEN)),
    ];
    for (case, pattern) in &unmatchable {
        let (status, page) = logs
            .call(DESCRIBE, &json!({"logGroupNamePattern": pattern}))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: a pattern that matches nothing is served, got {status} {page}"
        );
        assert_eq!(
            names(case, &page),
            Vec::<String>::new(),
            "case {case:?}: expected an empty list, got {page}"
        );
        assert!(
            page.get("logGroups").and_then(Value::as_array).is_some(),
            "case {case:?}: logGroups is reported as an empty list, not omitted, got {page}"
        );
    }

    // (case, the pattern, the names it selects)
    let matching: Vec<(&str, String, Vec<&str>)> = vec![
        ("a substring of one name", "audit".to_string(), vec![GROUPS[1]]),
        (
            "a substring of two names",
            "alpha".to_string(),
            vec![GROUPS[1], GROUPS[2]],
        ),
        (
            "a whole name",
            GROUPS[0].to_string(),
            vec![GROUPS[0]],
        ),
        // `@length(min: 0)`: a zero-length value is legal, and every name holds the empty
        // string.
        ("the empty pattern", String::new(), GROUPS.to_vec()),
    ];
    for (case, pattern, selected) in &matching {
        let page = logs
            .ok(DESCRIBE, &json!({"logGroupNamePattern": pattern}))
            .await;
        assert_eq!(
            names(case, &page),
            selected
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<String>>(),
            "case {case:?}: expected exactly the names holding the pattern, got {page}"
        );
    }

    // The member's LENGTH is still a bound on what the service takes: one character past the
    // maximum is refused, with the constraint the member declares.
    let (status, refusal) = logs
        .call(
            DESCRIBE,
            &json!({"logGroupNamePattern": "z".repeat(PATTERN_MAX_LEN + 1)}),
        )
        .await;
    assert_eq!(
        status, 400,
        "case \"one character past the length bound\": expected a refusal, got {status} {refusal}"
    );
    assert_eq!(
        error_code(&refusal),
        INVALID_PARAMETER,
        "case \"one character past the length bound\": expected {INVALID_PARAMETER}, got {refusal}"
    );
    assert_eq!(
        refusal.get("message").and_then(Value::as_str),
        Some(
            format!(
                "1 validation error detected: Value at 'logGroupNamePattern' failed to satisfy \
                 constraint: Member must have length less than or equal to {PATTERN_MAX_LEN}"
            )
            .as_str()
        ),
        "case \"one character past the length bound\": expected the member's length \
         constraint, got {refusal}"
    );
}

/// A wider account for the page-sequence table below, in the ASCII order the listing reports
/// it in: the groups the patterns of that table select sit at the START, in the MIDDLE and at
/// the END of the order, and two of them share no name prefix, so a row's page sequence turns
/// on where the walk stopped rather than on which names happened to be adjacent.
const WALK_ACCOUNT: [&str; 5] = [
    "/aws/lambda/alpha-handler",
    "/aws/opensearch/domains/beta/audit-logs",
    "alpha-app-logs",
    "beta-app-logs",
    "gamma-metrics",
];

/// One page of a walk, as a row of the table states it: the names it reported, and whether it
/// was resumable (it carried a `nextToken`).
type Page = (Vec<&'static str>, bool);

/// The account of [`WALK_ACCOUNT`], created.
async fn account_with_walk_groups(logs: &Logs) {
    for group in WALK_ACCOUNT {
        logs.ok("CreateLogGroup", &json!({"logGroupName": group}))
            .await;
    }
}

/// Walk `selector` at page size `limit` to the end, reporting what every page carried: the
/// names it reported and whether it was resumable.
async fn walked_pages(
    logs: &Logs,
    case: &str,
    selector: &Value,
    limit: usize,
) -> Vec<(Vec<String>, bool)> {
    let mut walked: Vec<(Vec<String>, bool)> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        assert!(
            walked.len() <= WALK_ACCOUNT.len() + 2,
            "case {case:?}: the walk must terminate, got {} pages: {walked:?}",
            walked.len()
        );
        let mut request = selector.clone();
        request["limit"] = json!(limit);
        if let Some(token) = &token {
            request["nextToken"] = json!(token);
        }
        let page = logs.ok(DESCRIBE, &request).await;
        let reported = names(case, &page);
        assert!(
            reported.len() <= limit,
            "case {case:?}: a page holds at most {limit} group(s), got {page}"
        );
        let next = page
            .get("nextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        walked.push((reported, next.is_some()));
        match next {
            Some(next) => token = Some(next),
            None => break,
        }
    }
    walked
}

/// The whole page sequence each selection must be walked in: every page's reported names and
/// whether it was resumable, so an extra or a missing round trip fails.
fn page_sequences() -> Vec<(&'static str, Value, usize, Vec<Page>)> {
    vec![
        // A substring search, one match per page: each page stops on the group it matched and
        // is resumable from there; the pages past the last match report nothing, and only the
        // page whose walk is past every group of the account ends the enumeration.
        (
            "a substring held by two names, one per page",
            json!({"logGroupNamePattern": "alpha"}),
            1,
            vec![
                (vec![WALK_ACCOUNT[0]], true),
                (vec![WALK_ACCOUNT[2]], true),
                (vec![], true),
                (vec![], false),
            ],
        ),
        (
            "the same substring, both matches on one page",
            json!({"logGroupNamePattern": "alpha"}),
            2,
            vec![
                (vec![WALK_ACCOUNT[0], WALK_ACCOUNT[2]], true),
                (vec![], true),
                (vec![], false),
            ],
        ),
        // The match is the LAST group of the account, so the page that reported it stopped on
        // the last group there is — and is still resumable, because a substring search reports
        // only how far it got.
        (
            "a substring held by the last name in order",
            json!({"logGroupNamePattern": "gamma"}),
            1,
            vec![(vec![WALK_ACCOUNT[4]], true), (vec![], false)],
        ),
        // A substring no name holds: the walk reports nothing and is still resumable until it
        // is past every group of the account.
        (
            "a substring no name holds",
            json!({"logGroupNamePattern": "zeta"}),
            1,
            vec![(vec![], true), (vec![], false)],
        ),
        // `@length(min: 0)`: the empty value is legal, and every name holds it.
        (
            "the empty substring, which every name holds",
            json!({"logGroupNamePattern": ""}),
            5,
            vec![(WALK_ACCOUNT.to_vec(), true), (vec![], false)],
        ),
        (
            "a substring held by names under two different prefixes",
            json!({"logGroupNamePattern": "beta"}),
            2,
            vec![
                (vec![WALK_ACCOUNT[1], WALK_ACCOUNT[3]], true),
                (vec![], true),
                (vec![], false),
            ],
        ),
        // The selections the service addresses directly: the page that reported the last
        // selected group ends the walk, whether that page is full or short.
        (
            "a name prefix selecting two groups, one per page",
            json!({"logGroupNamePrefix": "/aws/"}),
            1,
            vec![
                (vec![WALK_ACCOUNT[0]], true),
                (vec![WALK_ACCOUNT[1]], false),
            ],
        ),
        (
            "a name prefix selecting two groups on one full page",
            json!({"logGroupNamePrefix": "/aws/"}),
            2,
            vec![(vec![WALK_ACCOUNT[0], WALK_ACCOUNT[1]], false)],
        ),
        (
            "a name prefix no group carries",
            json!({"logGroupNamePrefix": "zeta"}),
            1,
            vec![(vec![], false)],
        ),
        (
            "an identifier list naming the first and the last group",
            json!({"logGroupIdentifiers": [WALK_ACCOUNT[0], WALK_ACCOUNT[4]]}),
            1,
            vec![
                (vec![WALK_ACCOUNT[0]], true),
                (vec![WALK_ACCOUNT[4]], false),
            ],
        ),
        (
            "the class every group of the account is in",
            json!({"logGroupClass": "STANDARD"}),
            2,
            vec![
                (vec![WALK_ACCOUNT[0], WALK_ACCOUNT[1]], true),
                (vec![WALK_ACCOUNT[2], WALK_ACCOUNT[3]], true),
                (vec![WALK_ACCOUNT[4]], false),
            ],
        ),
        (
            "a class no group of the account is in",
            json!({"logGroupClass": "INFREQUENT_ACCESS"}),
            1,
            vec![(vec![], false)],
        ),
        (
            "no filter at all",
            json!({}),
            2,
            vec![
                (vec![WALK_ACCOUNT[0], WALK_ACCOUNT[1]], true),
                (vec![WALK_ACCOUNT[2], WALK_ACCOUNT[3]], true),
                (vec![WALK_ACCOUNT[4]], false),
            ],
        ),
        (
            "no filter, one page holding the whole account",
            json!({}),
            5,
            vec![(WALK_ACCOUNT.to_vec(), false)],
        ),
    ]
}

/// Every row's whole page sequence: what each page reported, and whether it was resumable.
#[tokio::test]
async fn a_walk_reports_where_it_stopped_and_an_addressed_listing_what_is_left() {
    let logs = Logs::new();
    account_with_walk_groups(&logs).await;

    for (case, selector, limit, expected) in page_sequences() {
        let walked = walked_pages(&logs, case, &selector, limit).await;
        let expected: Vec<(Vec<String>, bool)> = expected
            .into_iter()
            .map(|(names, resumable)| {
                (
                    names.into_iter().map(ToString::to_string).collect(),
                    resumable,
                )
            })
            .collect();
        assert_eq!(
            walked, expected,
            "case {case:?} at page size {limit}: expected the pages {expected:?}, got {walked:?}"
        );
    }
}

/// Walking a substring search to its end is a READ: it leaves the account exactly as it was,
/// so the same walk repeated reports the same pages and every group is still listed once.
#[tokio::test]
async fn walking_a_substring_search_leaves_the_account_as_it_was() {
    let logs = Logs::new();
    account_with_walk_groups(&logs).await;

    let selector = json!({"logGroupNamePattern": "alpha"});
    let first = walked_pages(&logs, "the first walk", &selector, 1).await;
    let again = walked_pages(&logs, "the same walk repeated", &selector, 1).await;
    assert_eq!(
        again, first,
        "a repeated walk reports the same pages, got {again:?} after {first:?}"
    );

    let whole = walked_pages(&logs, "the whole account after the walks", &json!({}), 5).await;
    assert_eq!(
        whole,
        vec![(
            WALK_ACCOUNT
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<String>>(),
            false
        )],
        "every group of the account is still listed once, got {whole:?}"
    );
}
