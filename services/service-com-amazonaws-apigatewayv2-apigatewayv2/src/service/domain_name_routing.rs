// SPDX-License-Identifier: CC0-1.0

//! WHICH api stage a request made on a CUSTOM DOMAIN NAME is served by: the record's ROUTING
//! RULES, its API MAPPINGS, and which of the two its `RoutingMode` admits.
//!
//! A custom domain name carries two kinds of statement about what answers under it, and the
//! record's `RoutingMode` says which of them the front door reads
//! (`DomainName$RoutingMode`, whose enum is `API_MAPPING_ONLY | ROUTING_RULE_ONLY |
//! ROUTING_RULE_THEN_API_MAPPING`):
//!
//! * an `ApiMapping` binds one base path of the domain name to one `{ApiId, Stage}`, and is
//!   selected by the LONGEST key the request's path begins with
//!   (`crate::service::domain_name_socket::matching_mapping`, which states that reading);
//! * a `RoutingRule` CONDITIONS the selection on the request itself —
//!   "conditions can contain up to two matchHeaders conditions and one matchBasePaths
//!   conditions … You can only use AND between header and base path conditions"
//!   (`CreateRoutingRule$Conditions`) — and its `invokeApi` action names the
//!   `{ApiId, Stage}` a matched request is invoked on, with `StripBasePath` saying whether
//!   the base path it matched is removed from the path that api is given.
//!
//! Rules are evaluated in the order their `Priority` states ("the order in which API Gateway
//! evaluates a rule", `RoutingRule$Priority`), lowest value first, and the FIRST rule whose
//! every condition holds serves the request; a request no rule matches reaches no api under
//! `ROUTING_RULE_ONLY`, and falls to the api mappings under
//! `ROUTING_RULE_THEN_API_MAPPING`.
//!
//! This is the ONE reading both front doors of a custom domain name make — the socket the
//! record publishes as its `ApiGatewayDomainName` ([`super`]) and this
//! emulator's own endpoint, which a client reaches the record at by sending its name as the
//! `Host` (`crate::service::invoke::custom_domain_arrival`) — so the address a request arrives at
//! cannot change which api answers it.
//!
//! Everything here is a PURE reading of a request plus the stored record: nothing in this
//! module touches state or the clock.
//!
//! What ONE request costs is bounded by what the WRITE admits rather than by the request:
//! the rules a domain name may hold are capped
//! (`crate::state::MAX_ROUTING_RULES_PER_DOMAIN`), each rule carries at most the three
//! conditions its write admits, each condition matches on at most
//! `crate::state::MAX_CONDITION_VALUES` values of at most
//! `crate::state::MAX_CONDITION_VALUE_BYTES` bytes
//! (`crate::state::validate_routing_rule`), a base path is compared in time linear in its own
//! length, and a header glob is translated at most once per request per condition entry — not
//! once per value the client sent — and run under the explicit budgets
//! [`COMPILED_GLOB_LIMIT`] and [`GLOB_DFA_LIMIT`], so neither the fields a client repeats nor
//! the pattern a caller stored can make a request expensive.

use crate::ops::types as wire;
use crate::state::DomainName;

/// The `RoutingMode` values that admit the record's ROUTING RULES, and the one that admits
/// its api mappings after them, from the `RoutingMode` enum
/// (`smithy_query(query="shape", shape="RoutingMode")`: `API_MAPPING_ONLY`,
/// `ROUTING_RULE_ONLY`, `ROUTING_RULE_THEN_API_MAPPING`). The remaining value admits api
/// mappings only, which is what every mode this emulator does not know is read as — the
/// service's own default (`crate::state::DEFAULT_ROUTING_MODE`).
const ROUTING_RULE_ONLY: &str = "ROUTING_RULE_ONLY";
const ROUTING_RULE_THEN_API_MAPPING: &str = "ROUTING_RULE_THEN_API_MAPPING";

/// The api stage a request on a custom domain name is served by, and the path left for it.
#[derive(Debug, PartialEq, Eq)]
pub(in crate::service) struct Target<'r, 'p> {
    /// The api the selected statement names.
    pub(in crate::service) api_id: &'r str,
    /// The stage of that api the request is invoked on.
    pub(in crate::service) stage: &'r str,
    /// What the request carries UNDER the base path the selection matched, with no leading
    /// separator — the path that follows the stage in the api's own invoke path.
    pub(in crate::service) under: &'p str,
}

/// The request headers a `matchHeaders` condition is read from.
///
/// Stated as a trait because the two request shapes this service's seams are handed carry
/// their headers in different types — `http::HeaderMap` in `Service::can_handle` and on a
/// core-owned socket, the runtime API's own `Headers` everywhere else — and the selection
/// below has to be the SAME reading for both.
pub(in crate::service) trait MatchedHeaders: ::std::fmt::Debug {
    /// Every value the request carries for the field `name`, in the order it sent them. A
    /// field name is compared case-insensitively, as HTTP defines it (RFC 9110 §5.1).
    fn values(&self, name: &str) -> Vec<&str>;
}

impl MatchedHeaders for ::http::HeaderMap {
    fn values(&self, name: &str) -> Vec<&str> {
        let Ok(name) = ::http::header::HeaderName::try_from(name) else {
            return Vec::new();
        };
        self.get_all(&name)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .collect()
    }
}

impl MatchedHeaders for ::aws_smithy_runtime_api::http::Headers {
    fn values(&self, name: &str) -> Vec<&str> {
        self.get_all(name).collect()
    }
}

/// Where a request for `path` carrying `headers` goes on the record `record`, or `None` when
/// it reaches no api of the account at all.
///
/// `path` is the request's path as it was sent, leading separator included.
pub(in crate::service) fn target<'r, 'p>(
    record: &'r DomainName,
    path: &'p str,
    headers: &dyn MatchedHeaders,
) -> Option<Target<'r, 'p>> {
    match record.routing_mode.as_str() {
        ROUTING_RULE_ONLY => matching_rule(record, path, headers),
        ROUTING_RULE_THEN_API_MAPPING => {
            matching_rule(record, path, headers).or_else(|| matching_mapping(record, path))
        }
        // `API_MAPPING_ONLY`, the mode a record that states none carries
        // (`crate::state::DEFAULT_ROUTING_MODE`).
        _ => matching_mapping(record, path),
    }
}

/// The api mapping of `record` that answers `path`, as [`Target`].
///
/// The selection itself is the one statement of it both front doors already share
/// ([`super::matching_mapping`]); this only projects it onto the shape a
/// routing rule's selection answers in, so a caller composes one invoke path for either.
fn matching_mapping<'r, 'p>(record: &'r DomainName, path: &'p str) -> Option<Target<'r, 'p>> {
    let (mapping, under) = super::matching_mapping(record, path)?;
    Some(Target {
        api_id: &mapping.api_id,
        stage: &mapping.stage,
        under,
    })
}

/// The routing rule of `record` that serves `path` with `headers`, as [`Target`], or `None`
/// when no rule of the record matches the request.
///
/// Rules are evaluated in `Priority` order, lowest value first, and the first match serves —
/// so a rule of a lower priority value takes a request from one of a higher value whose
/// conditions also hold. Two rules carrying the SAME priority are evaluated in the order
/// their ids order them, which is the order every reader of the collection reports them in
/// (`ListRoutingRules`), so one request always resolves to one rule.
///
/// A path carrying a DOT SEGMENT (`.` or `..`) matches no rule, for the same reason it
/// answers to no api mapping: such a segment is a relative reference, and a remainder that
/// could climb back out of the base path would select an api the rule does not name.
fn matching_rule<'r, 'p>(
    record: &'r DomainName,
    path: &'p str,
    headers: &dyn MatchedHeaders,
) -> Option<Target<'r, 'p>> {
    if ::service_common::data_plane_addressing::carries_a_dot_segment(path) {
        return None;
    }
    let requested = path.trim_start_matches('/');
    let mut best: Option<(i32, Target<'r, 'p>)> = None;
    for rule in record.routing_rules.values() {
        // "The only supported action is invokeApi" (`CreateRoutingRule$Actions`), and a rule
        // is stored with at least one action, so the first action is the one a match is
        // served by.
        let Some(action) = rule.actions.first().map(|action| &action.invoke_api) else {
            continue;
        };
        let Some(under_base_path) = matched_conditions(&rule.conditions, requested, headers) else {
            continue;
        };
        // "The strip base path setting" (`RoutingRuleActionInvokeApi$StripBasePath`): the
        // base path the rule matched is removed from the path the api is given only where the
        // action asks for it, and the stored action always carries the setting the rule is
        // served under (`crate::state::resolve_routing_rule_actions`). A rule that matched on
        // headers alone matched no base path, so there is nothing to remove for it.
        let under = match action
            .strip_base_path
            .unwrap_or(crate::state::DEFAULT_STRIP_BASE_PATH)
        {
            true => under_base_path.unwrap_or(requested),
            false => requested,
        };
        if best.as_ref().is_none_or(|(held, _)| rule.priority < *held) {
            best = Some((
                rule.priority,
                Target {
                    api_id: &action.api_id,
                    stage: &action.stage,
                    under,
                },
            ));
        }
    }
    best.map(|(_priority, target)| target)
}

/// Whether EVERY condition of a rule holds for the request `requested` (its path with no
/// leading separator) carrying `headers`, and — when one of them matched a base path — what
/// the request carries under that base path.
///
/// `Some(None)` is a rule that matched with no base-path condition at all; `None` is a rule
/// that does not match. The conditions are ANDed, which is the one combination the service
/// admits: "API Gateway evaluates header conditions and base path conditions together. You
/// can only use AND between header and base path conditions"
/// (`CreateRoutingRule$Conditions`).
fn matched_conditions<'p>(
    conditions: &[wire::RoutingRuleCondition],
    requested: &'p str,
    headers: &dyn MatchedHeaders,
) -> Option<Option<&'p str>> {
    let mut under_base_path = None;
    for condition in conditions {
        if let Some(base_paths) = &condition.match_base_paths {
            under_base_path = Some(matched_base_path(&base_paths.any_of, requested)?);
        }
        if let Some(match_headers) = &condition.match_headers {
            if !matched_header(&match_headers.any_of, headers) {
                return None;
            }
        }
    }
    Some(under_base_path)
}

/// What the request carries under the base path of `any_of` that it begins with, or `None`
/// when it begins with none of them.
///
/// "The string of the case sensitive base path to be matched"
/// (`RoutingRuleMatchBasePaths$AnyOf`): a base path is made of path segments rather than of
/// characters, so it matches on a segment boundary — a request for `/widgets/42` is under the
/// base path `widgets`, and one for `/widgetsmith` is not — and the comparison is exact, with
/// no case folding. The LONGEST of several matching values is the one the request is under,
/// since it is the most specific statement of the base path the request carried.
fn matched_base_path<'p>(any_of: &[String], requested: &'p str) -> Option<&'p str> {
    let mut best: Option<(usize, &'p str)> = None;
    for candidate in any_of {
        let base = candidate.trim_matches('/');
        let under = match base.is_empty() {
            // An empty value is the root of the domain name: every path is under it.
            true => Some(requested),
            false => requested.strip_prefix(base).and_then(|rest| {
                match rest.strip_prefix('/') {
                    Some(under) => Some(under),
                    // The path IS the base path, with nothing under it.
                    None if rest.is_empty() => Some(rest),
                    // A longer segment that merely starts with the base path is a different
                    // path.
                    None => None,
                }
            }),
        };
        let Some(under) = under else { continue };
        if best.is_none_or(|(held, _)| held < base.len()) {
            best = Some((base.len(), under));
        }
    }
    best.map(|(_length, under)| under)
}

/// Whether the request carries a header ANY entry of `any_of` names with a value that entry's
/// glob matches.
///
/// "The header name and header value glob to be matched. The matchHeaders condition is
/// matched if any of the header name and header value globs are matched"
/// (`RoutingRuleMatchHeaders$AnyOf`), so one entry holding is the condition holding. A field
/// the request repeats holds when ANY of its values matches, which is the same reading as a
/// single field value carrying them in a list (RFC 9110 §5.3).
///
/// An entry's glob is translated at most ONCE per request — after the field it names is found
/// in the request, and then tried against every value that field carries — so neither the
/// values a client repeats nor the fields it sends can multiply the translation.
fn matched_header(
    any_of: &[wire::RoutingRuleMatchHeaderValue],
    headers: &dyn MatchedHeaders,
) -> bool {
    any_of.iter().any(|candidate| {
        let values = headers.values(&candidate.header);
        if values.is_empty() {
            return false;
        }
        let Some(matcher) = compiled_glob(&candidate.value_glob) else {
            // A glob neither the grammar nor the budgets below admit selects nothing, the
            // same answer this service's other caller-stored pattern gives for a pattern it
            // cannot evaluate (`crate::state::integration_response_key`): a condition that
            // cannot be evaluated does not hold.
            return false;
        };
        values
            .iter()
            .any(|value| matcher.is_match(value.as_bytes()))
    })
}

/// The glob `pattern` as the expression a field value is matched by, compiled under the
/// budgets below, or `None` for a pattern the grammar or those budgets refuse.
///
/// `RoutingRuleMatchHeaderValue$ValueGlob` is a GLOB over the field's value, so it is read by
/// the maintained glob matcher this tree already matches caller-written globs with rather than
/// by a comparison written here — and it is RUN by the same engine the other caller-stored
/// pattern of this service is run by, under the same kind of explicit budgets
/// (`crate::state::integration_response_key`), which is what keeps a stored pattern from
/// deciding what a request costs. A field value is matched as BYTES because that is the
/// alphabet a glob's expression is written over.
fn compiled_glob(pattern: &str) -> Option<::regex::bytes::Regex> {
    let glob = ::globset::GlobBuilder::new(pattern)
        // A header value is not a path: `*` stands for any run of characters in it, separator
        // or not.
        .literal_separator(false)
        .build()
        .ok()?;
    ::regex::bytes::RegexBuilder::new(glob.regex())
        .size_limit(COMPILED_GLOB_LIMIT)
        .dfa_size_limit(GLOB_DFA_LIMIT)
        .build()
        .ok()
}

/// The compiled-program budget one caller-STORED header glob may take, so a glob inside the
/// length its write admits (`crate::state::MAX_CONDITION_VALUE_BYTES`) still cannot compile
/// into an arbitrarily large automaton. The same budget the other caller-stored pattern of
/// this service is compiled under (`crate::state::integration_response_key`).
const COMPILED_GLOB_LIMIT: usize = 64 * 1024;

/// See [`COMPILED_GLOB_LIMIT`]: the budget for the lazy DFA a match runs through.
const GLOB_DFA_LIMIT: usize = 64 * 1024;

#[cfg(test)]
mod tests {
    use super::*;

    /// A record of `mode` holding `rules`, keyed as the store keys them.
    fn domain(mode: &str, rules: Vec<(&str, crate::state::RoutingRule)>) -> DomainName {
        DomainName {
            domain_name_arn: "arn:aws:apigateway:us-east-1::/domainnames/api.example.com"
                .to_string(),
            domain_name_id: "dn0000000000".to_string(),
            api_gateway_domain_name: "127.0.0.1:0".to_string(),
            region: "us-east-1".to_string(),
            domain_name: "api.example.com".to_string(),
            configurations: Vec::new(),
            mutual_tls_authentication: None,
            routing_mode: mode.to_string(),
            tags: Default::default(),
            migration_due_at: None,
            api_mappings: Default::default(),
            api_mapping_id_seed: 0,
            routing_rules: rules
                .into_iter()
                .map(|(id, rule)| (id.to_string(), rule))
                .collect(),
            routing_rule_id_seed: 0,
        }
    }

    /// One rule: its priority, the api stage its action invokes, whether it strips the base
    /// path, and its conditions.
    fn rule(
        priority: i32,
        api_id: &str,
        stage: &str,
        strip_base_path: bool,
        conditions: Vec<wire::RoutingRuleCondition>,
    ) -> crate::state::RoutingRule {
        crate::state::RoutingRule {
            routing_rule_id: "rule0000000".to_string(),
            routing_rule_arn: "arn:aws:apigateway:us-east-1::/domainnames/api.example.com\
                 /routingrules/rule0000000"
                .to_string(),
            priority,
            actions: vec![wire::RoutingRuleAction {
                invoke_api: wire::RoutingRuleActionInvokeApi {
                    api_id: api_id.to_string(),
                    stage: stage.to_string(),
                    strip_base_path: Some(strip_base_path),
                },
            }],
            conditions,
        }
    }

    /// A `matchBasePaths` condition over `any_of`.
    fn base_paths(any_of: &[&str]) -> wire::RoutingRuleCondition {
        wire::RoutingRuleCondition {
            match_base_paths: Some(wire::RoutingRuleMatchBasePaths {
                any_of: any_of.iter().map(|value| (*value).to_string()).collect(),
            }),
            match_headers: None,
        }
    }

    /// A `matchHeaders` condition over `any_of`.
    fn match_headers(any_of: &[(&str, &str)]) -> wire::RoutingRuleCondition {
        wire::RoutingRuleCondition {
            match_base_paths: None,
            match_headers: Some(wire::RoutingRuleMatchHeaders {
                any_of: any_of
                    .iter()
                    .map(|(header, value_glob)| wire::RoutingRuleMatchHeaderValue {
                        header: (*header).to_string(),
                        value_glob: (*value_glob).to_string(),
                    })
                    .collect(),
            }),
        }
    }

    /// One row of the base-path table: the values a condition names, the path a client asks
    /// for, and what the api is given under each `StripBasePath` setting (`None` where the
    /// condition does not hold at all).
    struct BasePathCase {
        case: &'static str,
        any_of: &'static [&'static str],
        path: &'static str,
        kept: Option<&'static str>,
        stripped: Option<&'static str>,
    }

    /// One row of the header table: the entries a `matchHeaders` condition names, the fields
    /// the request sends, the path it asks for, and whether the rule matches.
    struct HeaderCase {
        case: &'static str,
        any_of: &'static [(&'static str, &'static str)],
        sent: &'static [(&'static str, &'static str)],
        path: &'static str,
        matches: bool,
    }

    /// The headers a row sends.
    fn headers(sent: &[(&str, &str)]) -> ::http::HeaderMap {
        let mut headers = ::http::HeaderMap::new();
        for (name, value) in sent {
            headers.append(
                ::http::header::HeaderName::try_from(*name).expect("the row's field name is one"),
                ::http::HeaderValue::from_str(value).expect("the row's field value is one"),
            );
        }
        headers
    }

    /// RULE: a base-path condition holds for a path that begins with any of its values on a
    /// segment boundary, and the path the api is given keeps that base path unless the action
    /// strips it.
    #[test]
    fn a_base_path_condition_matches_on_a_segment_boundary_and_strips_only_when_asked() {
        // The rows span what the rule generalises over: the base path with a path under it,
        // with nothing under it, a nested path, a segment that merely starts with the base
        // path, another case of it, a path the condition does not name, and the longer of two
        // matching values — each under both strip settings.
        let cases = [
            BasePathCase {
                case: "a path under the base path",
                any_of: &["widgets"],
                path: "/widgets/42",
                kept: Some("widgets/42"),
                stripped: Some("42"),
            },
            BasePathCase {
                case: "the base path itself",
                any_of: &["widgets"],
                path: "/widgets",
                kept: Some("widgets"),
                stripped: Some(""),
            },
            BasePathCase {
                case: "a two-segment base path",
                any_of: &["widgets/eu"],
                path: "/widgets/eu/42",
                kept: Some("widgets/eu/42"),
                stripped: Some("42"),
            },
            BasePathCase {
                case: "a segment that merely starts with it",
                any_of: &["widgets"],
                path: "/widgetsmith",
                kept: None,
                stripped: None,
            },
            BasePathCase {
                case: "another case of the base path",
                any_of: &["widgets"],
                path: "/Widgets",
                kept: None,
                stripped: None,
            },
            BasePathCase {
                case: "a path the condition does not name",
                any_of: &["widgets"],
                path: "/gadgets",
                kept: None,
                stripped: None,
            },
            BasePathCase {
                case: "the longer of two matching values",
                any_of: &["widgets", "widgets/eu"],
                path: "/widgets/eu/42",
                kept: Some("widgets/eu/42"),
                stripped: Some("42"),
            },
        ];
        let sent = headers(&[]);
        for row in cases {
            let case = row.case;
            for (strip, expected) in [(false, row.kept), (true, row.stripped)] {
                let record = domain(
                    ROUTING_RULE_ONLY,
                    vec![(
                        "r1",
                        rule(100, "api00000000", "prod", strip, vec![base_paths(row.any_of)]),
                    )],
                );
                let actual = target(&record, row.path, &sent);
                let expected = expected.map(|under| Target {
                    api_id: "api00000000",
                    stage: "prod",
                    under,
                });
                assert_eq!(
                    actual, expected,
                    "case {case:?} with stripBasePath {strip}: expected {expected:?}, got \
                     {actual:?}"
                );
            }
        }
    }

    /// RULE: a header condition holds when any of its entries names a field the request
    /// carries whose value its glob matches, and conditions of both kinds are ANDed.
    #[test]
    fn a_header_condition_matches_any_of_its_globs_and_is_anded_with_the_base_path() {
        // The rows span what the rule generalises over: an exact value, another case of the
        // field NAME (which HTTP compares case-insensitively) and of its VALUE (which it does
        // not), a trailing-wildcard glob, a single-character glob, a field the request does
        // not carry, a second entry matching where the first does not, a repeated field, and
        // a base path that does not hold beside a header that does.
        let cases = [
            HeaderCase {
                case: "an exact value",
                any_of: &[("x-tier", "gold")],
                sent: &[("x-tier", "gold")],
                path: "/widgets",
                matches: true,
            },
            HeaderCase {
                case: "another case of the field name",
                any_of: &[("X-Tier", "gold")],
                sent: &[("x-tier", "gold")],
                path: "/widgets",
                matches: true,
            },
            HeaderCase {
                case: "another case of the value",
                any_of: &[("x-tier", "gold")],
                sent: &[("x-tier", "GOLD")],
                path: "/widgets",
                matches: false,
            },
            HeaderCase {
                case: "a trailing wildcard",
                any_of: &[("x-tier", "go*")],
                sent: &[("x-tier", "gold")],
                path: "/widgets",
                matches: true,
            },
            HeaderCase {
                case: "a single-character wildcard",
                any_of: &[("x-tier", "gol?")],
                sent: &[("x-tier", "gold")],
                path: "/widgets",
                matches: true,
            },
            HeaderCase {
                case: "a field the request does not carry",
                any_of: &[("x-tier", "gold")],
                sent: &[("x-other", "gold")],
                path: "/widgets",
                matches: false,
            },
            HeaderCase {
                case: "a second entry matching where the first does not",
                any_of: &[("x-tier", "silver"), ("x-tier", "gold")],
                sent: &[("x-tier", "gold")],
                path: "/widgets",
                matches: true,
            },
            HeaderCase {
                case: "one value of a repeated field",
                any_of: &[("x-tier", "gold")],
                sent: &[("x-tier", "silver"), ("x-tier", "gold")],
                path: "/widgets",
                matches: true,
            },
            HeaderCase {
                case: "the header holds but the base path does not",
                any_of: &[("x-tier", "gold")],
                sent: &[("x-tier", "gold")],
                path: "/gadgets",
                matches: false,
            },
        ];
        for row in cases {
            let case = row.case;
            let record = domain(
                ROUTING_RULE_ONLY,
                vec![(
                    "r1",
                    rule(
                        100,
                        "api00000000",
                        "prod",
                        false,
                        vec![base_paths(&["widgets"]), match_headers(row.any_of)],
                    ),
                )],
            );
            let matched = target(&record, row.path, &headers(row.sent)).is_some();
            assert_eq!(
                matched, row.matches,
                "case {case:?}: expected the rule to match: {}, got {matched}",
                row.matches
            );
        }
    }

    /// RULE: the matching rule of the LOWEST `Priority` value serves the request.
    #[test]
    fn the_lowest_priority_value_among_the_matching_rules_serves() {
        // Both rules match every path under the base path; only the priority differs, and the
        // rows put each of the two orders of declaration in front.
        for (case, first, second) in [
            ("the lower value declared first", (10, "lowapi00000"), (20, "highapi0000")),
            ("the lower value declared second", (20, "highapi0000"), (10, "lowapi00000")),
        ] {
            let record = domain(
                ROUTING_RULE_ONLY,
                vec![
                    ("r1", rule(first.0, first.1, "prod", false, vec![base_paths(&["widgets"])])),
                    ("r2", rule(second.0, second.1, "prod", false, vec![base_paths(&["widgets"])])),
                ],
            );
            let actual = target(&record, "/widgets", &headers(&[]));
            assert_eq!(
                actual.as_ref().map(|target| target.api_id),
                Some("lowapi00000"),
                "case {case:?}: expected the lowest priority value to serve, got {actual:?}"
            );
        }
    }

    /// RULE: the record's `RoutingMode` decides which statements the front door reads.
    #[test]
    fn the_routing_mode_decides_whether_rules_or_mappings_are_read() {
        let mapping = crate::state::ApiMapping {
            api_mapping_id: "map00000000".to_string(),
            api_id: "mapapi00000".to_string(),
            api_mapping_key: None,
            stage: "live".to_string(),
        };
        // The rows span the three modes the enum declares, each with a path the rule matches
        // and one it does not.
        let cases: [(&str, &str, &str, Option<&str>); 6] = [
            ("mappings only, on the rule's path", "API_MAPPING_ONLY", "/widgets", Some("mapapi00000")),
            ("mappings only, elsewhere", "API_MAPPING_ONLY", "/gadgets", Some("mapapi00000")),
            ("rules only, on the rule's path", ROUTING_RULE_ONLY, "/widgets", Some("ruleapi0000")),
            ("rules only, elsewhere", ROUTING_RULE_ONLY, "/gadgets", None),
            (
                "rules then mappings, on the rule's path",
                ROUTING_RULE_THEN_API_MAPPING,
                "/widgets",
                Some("ruleapi0000"),
            ),
            (
                "rules then mappings, elsewhere",
                ROUTING_RULE_THEN_API_MAPPING,
                "/gadgets",
                Some("mapapi00000"),
            ),
        ];
        for (case, mode, path, expected) in cases {
            let mut record = domain(
                mode,
                vec![("r1", rule(100, "ruleapi0000", "prod", false, vec![base_paths(&["widgets"])]))],
            );
            record
                .api_mappings
                .insert(mapping.api_mapping_id.clone(), mapping.clone());
            let actual = target(&record, path, &headers(&[]));
            assert_eq!(
                actual.as_ref().map(|target| target.api_id),
                expected,
                "case {case:?}: expected {expected:?} to serve, got {actual:?}"
            );
        }
    }

    /// RULE: a path carrying a dot segment matches no rule, whichever form it is written in.
    #[test]
    fn a_path_carrying_a_dot_segment_matches_no_rule() {
        let record = domain(
            ROUTING_RULE_ONLY,
            vec![("r1", rule(100, "api00000000", "prod", false, vec![base_paths(&["widgets"])]))],
        );
        for (case, path) in [
            ("a dot segment written plainly", "/widgets/../gadgets"),
            ("a dot segment hidden in an escape", "/widgets/%2e%2e%2fgadgets"),
        ] {
            let actual = target(&record, path, &headers(&[]));
            assert!(
                actual.is_none(),
                "case {case:?}: expected no rule to match, got {actual:?}"
            );
        }
    }
}
