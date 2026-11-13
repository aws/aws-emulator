// SPDX-License-Identifier: CC0-1.0

//! The WCU cost model, end to end over the wire.
//!
//! `CheckCapacity` "returns the web ACL capacity unit (WCU) requirements for a
//! specified scope and set of rules", and "WAF calculates capacity differently for
//! each rule type, to reflect the relative cost of each rule. Simple rules that cost
//! little to run use fewer WCUs than more complex rules that use more processing
//! power". The RULE asserted here is therefore: the reported `Capacity` is a function
//! of the statement KINDS in the rule list and of the modifiers each statement
//! carries — its positional constraint, its sensitivity level, the request component
//! it inspects, the text transformations it applies and its aggregation keys — never
//! of how many rules the list happens to hold.
//!
//! Each suite asserts that over a TABLE spanning the class: every positional
//! constraint `PositionalConstraint` declares, both `SensitivityLevel` values and the
//! absent one, the component modifiers and their compositions, one to the modelled
//! maximum of aggregation keys, and rule lists of one, two and ten rules. The same
//! figure is read back through `WebACL$Capacity`, because "you can use this to check
//! the capacity requirements for the rules you want to use in a RuleGroup or WebACL":
//! the report and the stored figure are one derivation.

use serde_json::{json, Value};

use crate::web_acl_lifecycle::{create_web_acl, ordered, rule, visibility, Waf};

/// The WCUs a `ByteMatchStatement` costs for a positional constraint that compares
/// the component at a FIXED position — `EXACTLY`, `STARTS_WITH`, `ENDS_WITH`
/// (`smithy_query(query="shape",shape="PositionalConstraint")`).
const BYTE_MATCH_ANCHORED: i64 = 2;

/// The WCUs a `ByteMatchStatement` costs for a positional constraint that SCANS the
/// component — `CONTAINS`, `CONTAINS_WORD`. See [`BYTE_MATCH_ANCHORED`].
const BYTE_MATCH_SCANNING: i64 = 10;

/// The WCUs a `SqliMatchStatement` costs at the `LOW` sensitivity its member
/// documents as the default
/// (`smithy_query(query="shape",shape="SqliMatchStatement",member="SensitivityLevel")`
/// → "Default: `LOW`").
const SQLI_LOW: i64 = 20;

/// The WCUs a `SqliMatchStatement` costs at `HIGH` sensitivity. See [`SQLI_LOW`].
const SQLI_HIGH: i64 = 30;

/// The WCUs an `XssMatchStatement` costs.
const XSS: i64 = 40;

/// The WCUs a `SizeConstraintStatement`, a `GeoMatchStatement` or a
/// `LabelMatchStatement` costs: one already-extracted request attribute.
const ATTRIBUTE: i64 = 1;

/// The WCUs a `RegexMatchStatement` costs.
const REGEX_MATCH: i64 = 3;

/// The WCUs a bare `RateBasedStatement` costs before its custom keys and its
/// scope-down statement.
const RATE_BASED: i64 = 2;

/// The WCUs a `RateBasedStatement` adds per custom aggregation key
/// (`RateBasedStatement$CustomKeys`, `length 0..5`).
const PER_CUSTOM_KEY: i64 = 30;

/// The WCUs each text transformation a statement applies adds.
const PER_TRANSFORMATION: i64 = 10;

/// The WCUs inspecting every value of a multi-valued request component adds — all
/// query arguments, all headers, all cookies
/// (`smithy_query(query="shape",shape="FieldToMatch")`: "inspect all query
/// arguments", "inspect the request headers", "inspect the request cookies").
const MULTI_VALUED_COMPONENT: i64 = 10;

/// The WCUs inspecting all query arguments adds. See
/// [`MULTI_VALUED_COMPONENT`].
const ALL_QUERY_ARGUMENTS: i64 = MULTI_VALUED_COMPONENT;

/// `[{ "Priority": i, "Type": t }, …]` for the transformation types named, in the
/// order named.
fn transformations(types: &[&str]) -> Value {
    Value::Array(
        types
            .iter()
            .enumerate()
            .map(|(index, kind)| json!({ "Priority": index, "Type": kind }))
            .collect(),
    )
}

/// A `ByteMatchStatement` over `field`, with `positional_constraint` and the text
/// transformations named.
fn byte_match(positional_constraint: &str, field: Value, types: &[&str]) -> Value {
    json!({
        "ByteMatchStatement": {
            "SearchString": "YQ==",
            "FieldToMatch": field,
            "TextTransformations": transformations(types),
            "PositionalConstraint": positional_constraint,
        }
    })
}

/// A `SqliMatchStatement` over the query string, at `sensitivity` when it names one.
fn sqli_match(sensitivity: Option<&str>) -> Value {
    let mut statement = json!({
        "SqliMatchStatement": {
            "FieldToMatch": { "QueryString": {} },
            "TextTransformations": transformations(&["NONE"]),
        }
    });
    if let Some(sensitivity) = sensitivity {
        statement["SqliMatchStatement"]["SensitivityLevel"] = json!(sensitivity);
    }
    statement
}

/// A `RateBasedStatement` aggregating on `custom_keys` custom keys (or on `IP` when
/// there are none), with `scope_down` when one is given.
///
/// The keys are the ones `RateBasedStatementCustomKey` declares that need no further
/// configuration or one name
/// (`smithy_query(query="shape",shape="RateBasedStatementCustomKey")`).
fn rate_based(custom_keys: usize, scope_down: Option<Value>) -> Value {
    let catalogue = [
        json!({ "HTTPMethod": {} }),
        json!({ "QueryString": { "TextTransformations": transformations(&["NONE"]) } }),
        json!({ "UriPath": { "TextTransformations": transformations(&["NONE"]) } }),
        json!({ "LabelNamespace": { "Namespace": "awswaf:managed:" } }),
        json!({ "Header": { "Name": "x-tenant", "TextTransformations": transformations(&["NONE"]) } }),
    ];
    let mut statement = if custom_keys == 0 {
        json!({ "RateBasedStatement": { "Limit": 2000, "AggregateKeyType": "IP" } })
    } else {
        json!({
            "RateBasedStatement": {
                "Limit": 2000,
                "AggregateKeyType": "CUSTOM_KEYS",
                "CustomKeys": Value::Array(catalogue[..custom_keys].to_vec()),
            }
        })
    };
    if let Some(scope_down) = scope_down {
        statement["RateBasedStatement"]["ScopeDownStatement"] = scope_down;
    }
    statement
}

/// An `XssMatchStatement` over the request body as JSON.
fn xss_over_json_body() -> Value {
    json!({
        "XssMatchStatement": {
            "FieldToMatch": {
                "JsonBody": { "MatchPattern": { "All": {} }, "MatchScope": "ALL" }
            },
            "TextTransformations": transformations(&["NONE"]),
        }
    })
}

/// `CheckCapacity` for `rules` in `scope`; answers the reported `Capacity`.
async fn check_capacity(waf: &Waf, scope: &str, rules: &Value) -> i64 {
    let answered = waf
        .ok("CheckCapacity", json!({ "Scope": scope, "Rules": rules }))
        .await;
    answered["Capacity"]
        .as_i64()
        .unwrap_or_else(|| panic!("CheckCapacity must report a numeric Capacity, got {answered}"))
}

/// The `Capacity` a web ACL created with `rules` reports, read back through
/// `GetWebACL`.
async fn stored_capacity(waf: &Waf, name: &str, rules: &Value) -> i64 {
    let (id, _arn, _lock) = create_web_acl(waf, "REGIONAL", name, rules.clone()).await;
    let got = waf
        .ok(
            "GetWebACL",
            json!({ "Name": name, "Scope": "REGIONAL", "Id": id }),
        )
        .await;
    got["WebACL"]["Capacity"]
        .as_i64()
        .unwrap_or_else(|| panic!("GetWebACL must report a numeric Capacity, got {got}"))
}

/// One priced rule list: what it is called, the rules, and the WCUs the cost model
/// says they consume.
struct PricedRules {
    /// What the row is called in an assertion.
    case: String,
    /// The rules `CheckCapacity` is asked to price.
    rules: Value,
    /// The WCUs the rules consume, written as the arithmetic of the cost model.
    expected: i64,
}

/// `n` rules carrying the same statement, each under a name and priority of its own.
fn repeated(statement: &Value, count: usize) -> Value {
    Value::Array(
        (0..count)
            .map(|index| {
                json!({
                    "Name": format!("r{index}"),
                    "Priority": index,
                    "Statement": statement.clone(),
                    "Action": { "Block": {} },
                    "VisibilityConfig": visibility(&format!("r{index}")),
                })
            })
            .collect(),
    )
}

/// Every statement kind's base cost, one row per kind and per value of the modifier
/// that splits that kind's cost.
fn base_cost_rows() -> Vec<PricedRules> {
    let mut rows: Vec<PricedRules> = Vec::new();
    // Every value `PositionalConstraint` declares, including the ones the issue that
    // drove this table never cited.
    for (constraint, expected) in [
        ("EXACTLY", BYTE_MATCH_ANCHORED),
        ("STARTS_WITH", BYTE_MATCH_ANCHORED),
        ("ENDS_WITH", BYTE_MATCH_ANCHORED),
        ("CONTAINS", BYTE_MATCH_SCANNING),
        ("CONTAINS_WORD", BYTE_MATCH_SCANNING),
    ] {
        rows.push(PricedRules {
            case: format!("a byte match with PositionalConstraint {constraint}"),
            rules: json!([rule(
                "a",
                byte_match(constraint, json!({ "UriPath": {} }), &["NONE"])
            )]),
            expected,
        });
    }
    // Both values `SensitivityLevel` declares, and the statement that names none.
    for (sensitivity, expected) in [
        (None, SQLI_LOW),
        (Some("LOW"), SQLI_LOW),
        (Some("HIGH"), SQLI_HIGH),
    ] {
        rows.push(PricedRules {
            case: format!("a SQLi match at SensitivityLevel {sensitivity:?}"),
            rules: json!([rule("a", sqli_match(sensitivity))]),
            expected,
        });
    }
    rows.push(PricedRules {
        case: "an XSS match".to_string(),
        rules: json!([rule(
            "a",
            json!({
                "XssMatchStatement": {
                    "FieldToMatch": { "QueryString": {} },
                    "TextTransformations": transformations(&["NONE"]),
                }
            })
        )]),
        expected: XSS,
    });
    rows.push(PricedRules {
        case: "a regex match".to_string(),
        rules: json!([rule(
            "a",
            json!({
                "RegexMatchStatement": {
                    "RegexString": "^/admin",
                    "FieldToMatch": { "UriPath": {} },
                    "TextTransformations": transformations(&["NONE"]),
                }
            })
        )]),
        expected: REGEX_MATCH,
    });
    rows.push(PricedRules {
        case: "a size constraint".to_string(),
        rules: json!([rule(
            "a",
            json!({
                "SizeConstraintStatement": {
                    "FieldToMatch": { "QueryString": {} },
                    "ComparisonOperator": "GT",
                    "Size": 100,
                    "TextTransformations": transformations(&["NONE"]),
                }
            })
        )]),
        expected: ATTRIBUTE,
    });
    rows.push(PricedRules {
        case: "a geo match".to_string(),
        rules: json!([rule(
            "a",
            json!({ "GeoMatchStatement": { "CountryCodes": ["US", "CN"] } })
        )]),
        expected: ATTRIBUTE,
    });
    rows.push(PricedRules {
        case: "a label match".to_string(),
        rules: json!([rule(
            "a",
            json!({ "LabelMatchStatement": { "Scope": "LABEL", "Key": "upstream" } })
        )]),
        expected: ATTRIBUTE,
    });
    rows
}

/// The modifier arithmetic on one base cost: each transformation adds a flat charge,
/// inspecting every query argument is a surcharge, inspecting the body as JSON
/// doubles the base, and the three compose.
fn modifier_rows() -> Vec<PricedRules> {
    let anchored = "STARTS_WITH";
    let uri_path = json!({ "UriPath": {} });
    let all_query_arguments = json!({ "AllQueryArguments": {} });
    let json_body = json!({
        "JsonBody": { "MatchPattern": { "All": {} }, "MatchScope": "ALL" }
    });
    // Transformation types from `TextTransformationType`
    // (`smithy_query(query="shape",shape="TextTransformationType")`).
    let chain = ["LOWERCASE", "URL_DECODE", "COMPRESS_WHITE_SPACE", "TRIM", "SHA256"];
    let mut rows: Vec<PricedRules> = Vec::new();
    for count in 1..=chain.len() {
        rows.push(PricedRules {
            case: format!("an anchored byte match with {count} text transformations"),
            rules: json!([rule(
                "a",
                byte_match(anchored, uri_path.clone(), &chain[..count])
            )]),
            expected: BYTE_MATCH_ANCHORED
                + i64::try_from(count).unwrap_or_default() * PER_TRANSFORMATION,
        });
    }
    rows.push(PricedRules {
        case: "a byte match over every query argument".to_string(),
        rules: json!([rule(
            "a",
            byte_match(anchored, all_query_arguments.clone(), &["NONE"])
        )]),
        expected: BYTE_MATCH_ANCHORED + ALL_QUERY_ARGUMENTS,
    });
    rows.push(PricedRules {
        case: "a byte match over every query argument, with one transformation".to_string(),
        rules: json!([rule(
            "a",
            byte_match(anchored, all_query_arguments, &["LOWERCASE"])
        )]),
        expected: BYTE_MATCH_ANCHORED + ALL_QUERY_ARGUMENTS + PER_TRANSFORMATION,
    });
    rows.push(PricedRules {
        case: "a byte match over the body as JSON".to_string(),
        rules: json!([rule(
            "a",
            byte_match(anchored, json_body.clone(), &["NONE"])
        )]),
        expected: 2 * BYTE_MATCH_ANCHORED,
    });
    rows.push(PricedRules {
        case: "a byte match over the body as JSON, with one transformation".to_string(),
        rules: json!([rule(
            "a",
            byte_match(anchored, json_body, &["LOWERCASE"])
        )]),
        expected: 2 * BYTE_MATCH_ANCHORED + PER_TRANSFORMATION,
    });
    rows.push(PricedRules {
        case: "a scanning byte match over the body as JSON".to_string(),
        rules: json!([rule(
            "a",
            byte_match(
                "CONTAINS",
                json!({ "JsonBody": { "MatchPattern": { "All": {} }, "MatchScope": "ALL" } }),
                &["NONE"]
            )
        )]),
        expected: 2 * BYTE_MATCH_SCANNING,
    });
    rows.push(PricedRules {
        case: "an XSS match over the body as JSON".to_string(),
        rules: json!([rule("a", xss_over_json_body())]),
        expected: 2 * XSS,
    });
    rows.push(PricedRules {
        case: "a size constraint over every query argument".to_string(),
        rules: json!([rule(
            "a",
            json!({
                "SizeConstraintStatement": {
                    "FieldToMatch": { "AllQueryArguments": {} },
                    "ComparisonOperator": "GT",
                    "Size": 100,
                    "TextTransformations": transformations(&["NONE"]),
                }
            })
        )]),
        expected: ATTRIBUTE + ALL_QUERY_ARGUMENTS,
    });
    rows.push(PricedRules {
        case: "a byte match over every header".to_string(),
        rules: json!([rule(
            "a",
            byte_match(
                anchored,
                json!({
                    "Headers": {
                        "MatchPattern": { "All": {} },
                        "MatchScope": "ALL",
                        "OversizeHandling": "CONTINUE",
                    }
                }),
                &["NONE"]
            )
        )]),
        expected: BYTE_MATCH_ANCHORED + MULTI_VALUED_COMPONENT,
    });
    rows.push(PricedRules {
        case: "a scanning byte match over every cookie, with one transformation"
            .to_string(),
        rules: json!([rule(
            "a",
            byte_match(
                "CONTAINS",
                json!({
                    "Cookies": {
                        "MatchPattern": { "All": {} },
                        "MatchScope": "KEY",
                        "OversizeHandling": "MATCH",
                    }
                }),
                &["LOWERCASE"]
            )
        )]),
        expected: BYTE_MATCH_SCANNING + MULTI_VALUED_COMPONENT + PER_TRANSFORMATION,
    });
    rows.push(PricedRules {
        case: "a byte match over ONE named header, which is not multi-valued".to_string(),
        rules: json!([rule(
            "a",
            byte_match(
                anchored,
                json!({ "SingleHeader": { "Name": "user-agent" } }),
                &["NONE"]
            )
        )]),
        expected: BYTE_MATCH_ANCHORED,
    });
    rows.push(PricedRules {
        case: "a SQLi match at HIGH sensitivity with two transformations".to_string(),
        rules: json!([rule(
            "a",
            json!({
                "SqliMatchStatement": {
                    "FieldToMatch": { "QueryString": {} },
                    "TextTransformations": transformations(&["LOWERCASE", "URL_DECODE"]),
                    "SensitivityLevel": "HIGH",
                }
            })
        )]),
        expected: SQLI_HIGH + 2 * PER_TRANSFORMATION,
    });
    rows
}

/// A rate-based statement's aggregation keys and scope-down statement: one row per
/// key count up to the modelled maximum, and one per nested cost.
fn rate_based_rows() -> Vec<PricedRules> {
    let mut rows: Vec<PricedRules> = vec![PricedRules {
        case: "a rate-based statement aggregating on the source IP".to_string(),
        rules: json!([rule("a", rate_based(0, None))]),
        expected: RATE_BASED,
    }];
    // `RateBasedStatementCustomKeys` is `length 0..5`, so every count the model
    // allows is a row.
    for count in 1..=5 {
        rows.push(PricedRules {
            case: format!("a rate-based statement aggregating on {count} custom keys"),
            rules: json!([rule("a", rate_based(count, None))]),
            expected: RATE_BASED + i64::try_from(count).unwrap_or_default() * PER_CUSTOM_KEY,
        });
    }
    rows.push(PricedRules {
        case: "a rate-based statement with an XSS scope-down".to_string(),
        rules: json!([rule(
            "a",
            rate_based(
                0,
                Some(json!({
                    "XssMatchStatement": {
                        "FieldToMatch": { "QueryString": {} },
                        "TextTransformations": transformations(&["NONE"]),
                    }
                }))
            )
        )]),
        expected: RATE_BASED + XSS,
    });
    rows.push(PricedRules {
        case: "two custom keys and a scanning byte-match scope-down".to_string(),
        rules: json!([rule(
            "a",
            rate_based(
                2,
                Some(byte_match("CONTAINS", json!({ "UriPath": {} }), &["NONE"]))
            )
        )]),
        expected: RATE_BASED + 2 * PER_CUSTOM_KEY + BYTE_MATCH_SCANNING,
    });
    rows
}

/// The rule-count rows: a transformation is applied to a request component once,
/// whatever number of rules then inspect it, so its charge is paid once for the rule
/// list — while every rule's own base cost is paid per rule.
fn shared_transformation_rows() -> Vec<PricedRules> {
    let chain = ["LOWERCASE", "URL_DECODE", "COMPRESS_WHITE_SPACE", "TRIM", "SHA256"];
    let transformed = byte_match("STARTS_WITH", json!({ "UriPath": {} }), &chain);
    let charged = i64::try_from(chain.len()).unwrap_or_default() * PER_TRANSFORMATION;
    let mut rows: Vec<PricedRules> = [1_usize, 2, 10]
        .into_iter()
        .map(|count| PricedRules {
            case: format!("{count} identical five-transformation byte-match rules"),
            rules: repeated(&transformed, count),
            expected: i64::try_from(count).unwrap_or_default() * BYTE_MATCH_ANCHORED + charged,
        })
        .collect();
    rows.push(PricedRules {
        case: "two rules transforming the SAME component the same way".to_string(),
        rules: ordered(json!([
            rule(
                "a",
                byte_match("STARTS_WITH", json!({ "UriPath": {} }), &["LOWERCASE"])
            ),
            rule(
                "b",
                byte_match("ENDS_WITH", json!({ "UriPath": {} }), &["LOWERCASE"])
            ),
        ])),
        expected: 2 * BYTE_MATCH_ANCHORED + PER_TRANSFORMATION,
    });
    rows.push(PricedRules {
        case: "two rules transforming DIFFERENT components the same way".to_string(),
        rules: ordered(json!([
            rule(
                "a",
                byte_match("STARTS_WITH", json!({ "UriPath": {} }), &["LOWERCASE"])
            ),
            rule(
                "b",
                byte_match("STARTS_WITH", json!({ "QueryString": {} }), &["LOWERCASE"])
            ),
        ])),
        expected: 2 * BYTE_MATCH_ANCHORED + 2 * PER_TRANSFORMATION,
    });
    rows.push(PricedRules {
        case: "two rules over one component with different transformation chains".to_string(),
        rules: ordered(json!([
            rule(
                "a",
                byte_match("STARTS_WITH", json!({ "UriPath": {} }), &["LOWERCASE"])
            ),
            rule(
                "b",
                byte_match(
                    "STARTS_WITH",
                    json!({ "UriPath": {} }),
                    &["LOWERCASE", "URL_DECODE"]
                )
            ),
        ])),
        expected: 2 * BYTE_MATCH_ANCHORED + 2 * PER_TRANSFORMATION,
    });
    rows.push(PricedRules {
        case: "two rules over one component, one transforming nothing".to_string(),
        rules: ordered(json!([
            rule(
                "a",
                byte_match("STARTS_WITH", json!({ "UriPath": {} }), &["NONE"])
            ),
            rule(
                "b",
                byte_match("STARTS_WITH", json!({ "UriPath": {} }), &["LOWERCASE"])
            ),
        ])),
        expected: 2 * BYTE_MATCH_ANCHORED + PER_TRANSFORMATION,
    });
    rows.push(PricedRules {
        case: "two rules over one component, one listing NONE beside the same chain"
            .to_string(),
        rules: ordered(json!([
            rule(
                "a",
                byte_match(
                    "STARTS_WITH",
                    json!({ "UriPath": {} }),
                    &["NONE", "LOWERCASE"]
                )
            ),
            rule(
                "b",
                byte_match("STARTS_WITH", json!({ "UriPath": {} }), &["LOWERCASE"])
            ),
        ])),
        // NONE transforms nothing, so both rules apply the same chain to the same
        // component and share its one charge.
        expected: 2 * BYTE_MATCH_ANCHORED + PER_TRANSFORMATION,
    });
    rows.push(PricedRules {
        case: "a NONE between two real transformations".to_string(),
        rules: json!([rule(
            "a",
            byte_match(
                "STARTS_WITH",
                json!({ "UriPath": {} }),
                &["LOWERCASE", "NONE", "URL_DECODE"]
            )
        )]),
        expected: BYTE_MATCH_ANCHORED + 2 * PER_TRANSFORMATION,
    });
    rows
}

/// Every row of the cost model, priced through `CheckCapacity` in both scopes: the
/// price of a rule list that references nothing is a property of the rules.
#[tokio::test]
async fn check_capacity_prices_each_statement_kind_and_its_modifiers() {
    let waf = Waf::new();
    let rows: Vec<PricedRules> = base_cost_rows()
        .into_iter()
        .chain(modifier_rows())
        .chain(rate_based_rows())
        .chain(shared_transformation_rows())
        .collect();
    for scope in ["REGIONAL", "CLOUDFRONT"] {
        for row in &rows {
            let case = format!("{scope}: {}", row.case);
            let actual = check_capacity(&waf, scope, &row.rules).await;
            assert_eq!(
                actual, row.expected,
                "case {case:?}: expected Capacity {}, got {actual}",
                row.expected
            );
        }
    }
}

/// `WebACL$Capacity` is the same derivation: "you can use this to check the capacity
/// requirements for the rules you want to use in a RuleGroup or WebACL", so a web ACL
/// holding a priced rule list reports exactly what `CheckCapacity` reported for it.
#[tokio::test]
async fn a_web_acl_stores_the_capacity_check_capacity_reports() {
    let waf = Waf::new();
    let rows: Vec<PricedRules> = base_cost_rows()
        .into_iter()
        .chain(modifier_rows())
        .chain(rate_based_rows())
        .chain(shared_transformation_rows())
        .collect();
    for (index, row) in rows.iter().enumerate() {
        let case = &row.case;
        let reported = check_capacity(&waf, "REGIONAL", &row.rules).await;
        let stored = stored_capacity(&waf, &format!("priced-acl-{index}"), &row.rules).await;
        assert_eq!(
            reported, row.expected,
            "case {case:?}: expected CheckCapacity {}, got {reported}",
            row.expected
        );
        assert_eq!(
            stored, reported,
            "case {case:?}: WebACL.Capacity must equal CheckCapacity, got {stored} and \
             {reported}"
        );
    }
}

/// The cost of a rule list is not a multiple of its rule count: the marginal cost of
/// one more rule carrying a transformed statement is strictly below the cost of that
/// rule alone, and the list's cost is linear in the count with that marginal.
///
/// This is what a per-rule-multiplicative model, or one keyed on the number of rules,
/// cannot reproduce.
#[tokio::test]
async fn the_marginal_cost_of_another_transformed_rule_is_below_the_first_rules_cost() {
    let waf = Waf::new();
    let chain = ["LOWERCASE", "URL_DECODE", "COMPRESS_WHITE_SPACE", "TRIM", "SHA256"];
    for count in 1..=chain.len() {
        let case = format!("a byte match carrying {count} transformations");
        let statement = byte_match("STARTS_WITH", json!({ "UriPath": {} }), &chain[..count]);
        let one = check_capacity(&waf, "REGIONAL", &repeated(&statement, 1)).await;
        let two = check_capacity(&waf, "REGIONAL", &repeated(&statement, 2)).await;
        let ten = check_capacity(&waf, "REGIONAL", &repeated(&statement, 10)).await;
        let marginal = two - one;
        assert!(
            marginal < one,
            "case {case:?}: the marginal cost per extra rule must be strictly below the \
             single-rule cost, got marginal {marginal} and single {one}"
        );
        assert_eq!(
            ten,
            one + 9 * marginal,
            "case {case:?}: the cost must be linear in the rule count with that marginal, \
             got {ten} for ten rules against {one} and {two}"
        );
    }
}
