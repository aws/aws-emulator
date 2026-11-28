// SPDX-License-Identifier: CC0-1.0

//! The money an agreement request calculates, end to end over HTTP.
//!
//! The rule under test: `CreateAgreementRequest` "captures the requested terms,
//! calculates charges, and returns a summary"
//! (`smithy_query(query="operation", operation="CreateAgreementRequest")`), so
//! every amount member of the summary is COMPUTED from the rates the emulator
//! publishes and the configuration the acceptor chose — a dimension of a
//! configured prepaid term at the published per-unit rate for its contract
//! length (`RateCardItem$price` is the "per unit price for the product dimension
//! that's used for calculating the amount to be charged"), a term that carries
//! one price at that price — and every aggregate is the sum of the line items
//! under it: `ExpectedCharge$amount` of a configured term is the sum of its
//! dimensions' `ItemizedCharge$incrementalChargeAmount`, and
//! `ChargeSummary$newAgreementValue` is "the total value of the agreement" over
//! the terms that commit one, or the documented `0` when none does. The quote
//! prices each charge ONCE: the agreement that accepting it mints
//! (`EstimatedCharges$agreementValue`) and that agreement's charges
//! (`Charge$amount`) report the amounts the quote calculated.
//!
//! What is NOT priced carries no amount, which `ExpectedCharge$amount` ("Only
//! available when the charge amount is known") states: a usage-based charge,
//! which follows "product usage"; a variable payment, whose amount no seller has
//! requested yet; and anything the published rates do not carry — a dimension or
//! a contract length they do not publish. A total that would be missing one of
//! its line items is absent rather than reported short, and no tax member is
//! reported at all (`ChargeSummary` returns them "on a best-effort basis", a
//! null tax amount meaning "tax estimation was unavailable at the time of the
//! request").
//!
//! The rows differ in what the rule generalises over: which term kinds were
//! requested and how many, how many dimensions one term configures and at what
//! quantities (including the zero `ZeroValueInteger` admits), which contract
//! length the dimension is bought for, whether the dimension or the length is
//! one the published rates carry, whether the request amends an agreement that
//! already holds units of a dimension, and what tax estimate it asked for. Every
//! id and every amount is read back from the call that produced it, so no row
//! can agree with a handler that invents one.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::agreement_lifecycle::{
    accept, call, call_as, create, emulator, new_request, term, upfront_term,
    variable_payment_term, Answer,
};
use crate::purchase_order_updates::{charges, items};

/// The currency `ChargeSummary$currencyCode` reports for "all charges", spelled
/// out here rather than imported so a handler quoting a different currency fails
/// these rows. `CurrencyCode` is `@length(3, 3)` with `^[A-Z]+$`
/// (`smithy_query(query="shape", shape="CurrencyCode")`).
const CURRENCY: &str = "USD";

/// `ChargeSummary$newAgreementValue` for terms that commit the acceptor to
/// nothing: `EstimatedCharges$agreementValue` documents "This is `0` if the
/// accepted terms contain `UsageBasedPricingTerm` without
/// `ConfigurableUpfrontPricingTerm` or `RecurringPaymentTerm`".
const ZERO_VALUE: &str = "0";

/// The `Timing` of a charge that is "charged immediately when the agreement
/// request is accepted" (`smithy_query(query="shape", shape="Timing")`).
const ON_ACCEPTANCE: &str = "ON_ACCEPTANCE";

/// A product dimension the emulator's rate card publishes no rate for, so a
/// quote buying it carries no amount.
const UNPUBLISHED_DIMENSION: &str = "Widgets";

/// A contract length the rate card publishes no rate at — a six-month term,
/// which contract pricing is not bought in — so a quote at it carries no amount.
const UNPUBLISHED_SELECTOR: &str = "P6M";

/// The contract lengths the published rates carry, as the emulator's rate card
/// (`crate::state::RATE_CARD`) lists them: the ISO-8601 durations AWS
/// Marketplace contract pricing is bought in.
const PUBLISHED_SELECTORS: &[&str] = &["P1M", "P12M", "P24M", "P36M"];

/// Product dimensions the published rates carry, from the same rate card. The
/// issue this suite answers cited only `Users`, so the rows below buy the others
/// too.
const PUBLISHED_DIMENSIONS: &[&str] = &["Users", "Admins", "Hosts", "Nodes", "Storage"];

/// One published per-unit rate, read off `crate::state::RATE_CARD`: buying one
/// `Storage` unit for a 36-month contract costs this much. Spelled out here so a
/// handler that reports some other number — or that prices from something other
/// than the published rate card — fails this suite.
const STORAGE_P36M_UNIT_RATE: &str = "2.88";

/// The minor units the amount in `value` spells, asserted to be in the form
/// every money member of this service carries: decimal units, a point, and
/// exactly two minor digits.
fn minor(case: &str, member: &str, value: &Value) -> u128 {
    let text = value
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: {member} must carry an amount, got {value:?}"));
    let (units, minor_digits) = text.split_once('.').unwrap_or_else(|| {
        panic!("case {case:?}: {member} must carry a decimal amount, got {text:?}")
    });
    assert_eq!(
        minor_digits.len(),
        2,
        "case {case:?}: {member} must carry two minor digits, got {text:?}"
    );
    let units: u128 = units.parse().unwrap_or_else(|_| {
        panic!("case {case:?}: {member} must carry decimal units, got {text:?}")
    });
    let minor_digits: u128 = minor_digits.parse().unwrap_or_else(|_| {
        panic!("case {case:?}: {member} must carry decimal minor units, got {text:?}")
    });
    units * 100 + minor_digits
}

/// `minor` minor units in the form the service reports an amount in, for
/// comparing a computed total against a reported one.
fn as_amount(minor: u128) -> String {
    format!("{}.{:02}", minor / 100, minor % 100)
}

/// The quote over `terms`, asserted to have succeeded.
async fn quote_over(
    router: &MultiServiceRouter,
    case: &str,
    proposal: &str,
    terms: Vec<Value>,
) -> Answer {
    let answer = create(router, new_request(proposal, terms)).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: the quote must succeed, got {} with {:?}",
        answer.status, answer.body
    );
    answer
}

/// `chargeSummary.expectedCharges`, in the order the quote reported them.
fn expected_charges(answer: &Answer) -> Vec<Value> {
    answer.body["chargeSummary"]["expectedCharges"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// `chargeSummary.itemizedCharges`, in the order the quote reported them.
fn itemized_charges(answer: &Answer) -> Vec<Value> {
    answer.body["chargeSummary"]["itemizedCharges"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// The amount the quote over one single-dimension prepaid term charges, absent
/// when the published rates do not price that dimension at that length.
async fn single_dimension_amount(
    router: &MultiServiceRouter,
    case: &str,
    key: &str,
    selector: &str,
    quantity: i32,
) -> Option<u128> {
    let answer = quote_over(
        router,
        case,
        "ap-amountRates",
        vec![upfront_term("cfg", selector, &[(key, quantity)])],
    )
    .await;
    let charges = expected_charges(&answer);
    assert_eq!(
        charges.len(),
        1,
        "case {case:?}: one charged term implies one charge, got {:?}",
        answer.body
    );
    (!charges[0]["amount"].is_null())
        .then(|| minor(case, "expectedCharges[0].amount", &charges[0]["amount"]))
}

/// What a row expects `ChargeSummary$newAgreementValue` to carry.
#[derive(Debug, PartialEq, Eq)]
enum AgreementValue {
    /// No requested term commits an amount: the documented `0`.
    Zero,
    /// Every committed term is priced: the total of the priced charges.
    Total,
    /// A term commits an amount the published rates do not price, so the total
    /// is not known and the member is absent.
    Unknown,
}

/// One row of the quote table: the terms requested, whether each charge they
/// imply is priced, and what the agreement value comes to.
struct Quote {
    case: &'static str,
    terms: Vec<Value>,
    /// One entry per expected charge, in the order the terms were requested:
    /// whether that charge carries an amount.
    priced: Vec<bool>,
    value: AgreementValue,
}

fn quotes() -> Vec<Quote> {
    vec![
        Quote {
            case: "a legal term carries no charge at all",
            terms: vec![term("eula.v1")],
            priced: vec![],
            value: AgreementValue::Zero,
        },
        Quote {
            case: "usage-based pricing is charged, but not at a known amount",
            terms: vec![term("UsageBasedPricingTerm")],
            priced: vec![false],
            value: AgreementValue::Zero,
        },
        Quote {
            case: "a variable payment is an amount nobody has requested yet",
            terms: vec![variable_payment_term("vpt-1")],
            priced: vec![false],
            value: AgreementValue::Zero,
        },
        Quote {
            case: "a fixed upfront term carries one published price",
            terms: vec![term("FixedUpfrontPricingTerm")],
            priced: vec![true],
            value: AgreementValue::Total,
        },
        Quote {
            case: "a recurring term is priced per billing period",
            terms: vec![term("RecurringPaymentTerm")],
            priced: vec![true],
            value: AgreementValue::Total,
        },
        Quote {
            case: "a payment schedule is priced at the total of its installments",
            terms: vec![term("PaymentScheduleTerm")],
            priced: vec![true],
            value: AgreementValue::Total,
        },
        Quote {
            case: "a configured prepaid term is priced per dimension",
            terms: vec![upfront_term("cfg", "P12M", &[("Users", 5)])],
            priced: vec![true],
            value: AgreementValue::Total,
        },
        Quote {
            case: "three dimensions, each priced on its own key",
            terms: vec![upfront_term(
                "cfg",
                "P36M",
                &[("Users", 10), ("Admins", 2), ("Storage", 4096)],
            )],
            priced: vec![true],
            value: AgreementValue::Total,
        },
        Quote {
            case: "a dimension bought at the zero quantity the model admits",
            terms: vec![upfront_term("cfg", "P1M", &[("Users", 0)])],
            priced: vec![true],
            value: AgreementValue::Total,
        },
        Quote {
            case: "two configured terms, bought for different contract lengths",
            terms: vec![
                upfront_term("cfg-a", "P1M", &[("Nodes", 3)]),
                upfront_term("cfg-b", "P24M", &[("Storage", 500), ("Users", 1)]),
            ],
            priced: vec![true, true],
            value: AgreementValue::Total,
        },
        Quote {
            case: "every kind at once: unpriced, priced and uncharged",
            terms: vec![
                term("eula.v1"),
                term("UsageBasedPricingTerm"),
                term("FixedUpfrontPricingTerm"),
                upfront_term("cfg", "P12M", &[("Hosts", 7), ("Admins", 1)]),
                variable_payment_term("vpt-2"),
            ],
            priced: vec![false, true, true, false],
            value: AgreementValue::Total,
        },
        Quote {
            case: "a dimension the published rates do not carry",
            terms: vec![upfront_term("cfg", "P12M", &[(UNPUBLISHED_DIMENSION, 5)])],
            priced: vec![false],
            value: AgreementValue::Unknown,
        },
        Quote {
            case: "a published dimension at a contract length nothing is published for",
            terms: vec![upfront_term("cfg", UNPUBLISHED_SELECTOR, &[("Users", 5)])],
            priced: vec![false],
            value: AgreementValue::Unknown,
        },
        Quote {
            case: "one published and one unpublished dimension in the same term",
            terms: vec![upfront_term(
                "cfg",
                "P12M",
                &[("Users", 2), (UNPUBLISHED_DIMENSION, 1)],
            )],
            priced: vec![false],
            value: AgreementValue::Unknown,
        },
        Quote {
            case: "a priced term beside one the rates do not price",
            terms: vec![
                term("FixedUpfrontPricingTerm"),
                upfront_term("cfg", "P12M", &[(UNPUBLISHED_DIMENSION, 2)]),
            ],
            priced: vec![true, false],
            value: AgreementValue::Unknown,
        },
    ]
}

/// A quote prices every charge the published rates carry, leaves the rest
/// unpriced, and totals what it priced into the agreement value — with every
/// charge in one currency and no tax member at all.
#[tokio::test]
async fn a_quote_prices_every_charge_the_published_rates_carry() {
    let router = emulator();
    for row in quotes() {
        let case = row.case;
        let answer = quote_over(&router, case, "ap-amounts1", row.terms.clone()).await;
        let summary = &answer.body["chargeSummary"];
        assert_eq!(
            summary["currencyCode"].as_str(),
            Some(CURRENCY),
            "case {case:?}: every charge is priced in one currency, got {summary:?}"
        );

        let charges = expected_charges(&answer);
        assert_eq!(
            charges.len(),
            row.priced.len(),
            "case {case:?}: expected {} charges, got {summary:?}",
            row.priced.len()
        );
        let itemized = itemized_charges(&answer);

        let mut total: u128 = 0;
        for (position, (charge, priced)) in charges.iter().zip(&row.priced).enumerate() {
            let member = format!("expectedCharges[{position}].amount");
            assert_eq!(
                !charge["amount"].is_null(),
                *priced,
                "case {case:?}: {member} is reported exactly when the published rates price it, \
                 got {charge:?}"
            );
            if *priced {
                total += minor(case, &member, &charge["amount"]);
            }
            assert!(
                charge["amountAfterTax"].is_null() && charge["estimatedTaxes"].is_null(),
                "case {case:?}: no tax estimate is reported, got {charge:?}"
            );

            // A charge of a configured term is the sum of its own itemized
            // charges — and carries an amount exactly when every one of them
            // does, because a total missing a line item is not the amount of the
            // charge.
            let id = charge["id"].as_str().unwrap_or_default();
            let lines: Vec<&Value> = itemized
                .iter()
                .filter(|item| item["chargeReference"].as_str() == Some(id))
                .collect();
            if lines.is_empty() {
                continue;
            }
            let all_priced = lines
                .iter()
                .all(|item| !item["incrementalChargeAmount"].is_null());
            assert_eq!(
                all_priced, *priced,
                "case {case:?}: charge {id} carries an amount exactly when every itemized charge \
                 under it does, got {:?}",
                answer.body
            );
            if all_priced {
                let lined: u128 = lines
                    .iter()
                    .map(|item| {
                        minor(
                            case,
                            "itemizedCharges[].incrementalChargeAmount",
                            &item["incrementalChargeAmount"],
                        )
                    })
                    .sum();
                assert_eq!(
                    lined,
                    minor(case, &member, &charge["amount"]),
                    "case {case:?}: charge {id} must be the sum {} of its itemized charges, got \
                     {charge:?}",
                    as_amount(lined)
                );
            }
        }

        match row.value {
            AgreementValue::Zero => assert_eq!(
                summary["newAgreementValue"].as_str(),
                Some(ZERO_VALUE),
                "case {case:?}: terms that commit nothing are worth {ZERO_VALUE}, got {summary:?}"
            ),
            AgreementValue::Total => assert_eq!(
                summary["newAgreementValue"].as_str(),
                Some(as_amount(total).as_str()),
                "case {case:?}: expected the total {} of the priced charges, got {summary:?}",
                as_amount(total)
            ),
            AgreementValue::Unknown => assert!(
                summary["newAgreementValue"].is_null(),
                "case {case:?}: a committed term the rates do not price leaves the total unknown, \
                 got {summary:?}"
            ),
        }
        assert!(
            summary["newAgreementValueAfterTax"].is_null() && summary["estimatedTaxes"].is_null(),
            "case {case:?}: no tax estimate is reported, got {summary:?}"
        );
    }
}

/// A dimension is priced at the published per-unit rate for the contract length
/// it is bought for: the amount is proportional to the quantity, dimensions of
/// one term are priced on their own keys and add up, the same request is always
/// priced the same, and a dimension or a length the rates do not publish is
/// quoted with no amount.
#[tokio::test]
async fn a_dimension_is_priced_at_the_published_rate_for_every_unit_it_buys() {
    let router = emulator();

    // The quantities span one unit, a few, and a large order; the dimensions and
    // contract lengths span the published rate card.
    let quantities = [1i32, 2, 5, 100, 1000];
    let mut unit_rates: BTreeMap<(&str, &str), u128> = BTreeMap::new();
    for key in PUBLISHED_DIMENSIONS {
        for selector in PUBLISHED_SELECTORS {
            let case = format!("{key} for {selector}");
            let unit = single_dimension_amount(&router, &case, key, selector, 1)
                .await
                .unwrap_or_else(|| {
                    panic!("case {case:?}: the rate card publishes a rate for this dimension")
                });
            assert!(
                unit > 0,
                "case {case:?}: a unit of a purchased dimension costs something, got {unit}"
            );
            unit_rates.insert((key, selector), unit);

            for quantity in quantities {
                let case = format!("{case} x{quantity}");
                let amount = single_dimension_amount(&router, &case, key, selector, quantity)
                    .await
                    .unwrap_or_else(|| panic!("case {case:?}: this dimension is published"));
                let expected = unit * u128::try_from(quantity).unwrap_or_default();
                assert_eq!(
                    amount,
                    expected,
                    "case {case:?}: {quantity} units at {} must cost {}, got {}",
                    as_amount(unit),
                    as_amount(expected),
                    as_amount(amount)
                );
            }
        }
    }

    // One published rate, read off the rate card rather than through it: the
    // quote reports the data the emulator publishes, not a number of its own.
    assert_eq!(
        as_amount(unit_rates[&("Storage", "P36M")]),
        STORAGE_P36M_UNIT_RATE,
        "one Storage unit for a 36-month contract is published at \
         {STORAGE_P36M_UNIT_RATE}, got {unit_rates:?}"
    );

    // The rate follows the dimension and the contract length: a longer contract
    // and a different dimension are different rates.
    let distinct: BTreeSet<u128> = unit_rates.values().copied().collect();
    assert_eq!(
        distinct.len(),
        unit_rates.len(),
        "each dimension and contract length is published at its own rate, got {unit_rates:?}"
    );

    // Two dimensions of one term are priced on their own keys and add up.
    let case = "two dimensions of one term";
    let both = quote_over(
        &router,
        case,
        "ap-amountRates",
        vec![upfront_term("cfg", "P12M", &[("Users", 4), ("Storage", 3)])],
    )
    .await;
    let charges = expected_charges(&both);
    let amount = minor(case, "expectedCharges[0].amount", &charges[0]["amount"]);
    let users = unit_rates[&("Users", "P12M")] * 4;
    let storage = unit_rates[&("Storage", "P12M")] * 3;
    assert_eq!(
        amount,
        users + storage,
        "case {case:?}: expected {} for 4 Users and 3 Storage, got {}",
        as_amount(users + storage),
        as_amount(amount)
    );

    // The same request, quoted again, is quoted at the same price.
    let again = single_dimension_amount(&router, "repriced", "Users", "P12M", 100).await;
    assert_eq!(
        again,
        Some(unit_rates[&("Users", "P12M")] * 100),
        "a second quote over the same terms must carry the same amount, got {again:?}"
    );

    // What the rate card does not publish is not priced.
    for (case, key, selector) in [
        ("a dimension nothing is published for", UNPUBLISHED_DIMENSION, "P12M"),
        ("a contract length nothing is published for", "Users", UNPUBLISHED_SELECTOR),
        (
            "neither the dimension nor the length is published",
            UNPUBLISHED_DIMENSION,
            UNPUBLISHED_SELECTOR,
        ),
    ] {
        let amount = single_dimension_amount(&router, case, key, selector, 3).await;
        assert_eq!(
            amount, None,
            "case {case:?}: an unpublished rate is quoted with no amount, got {amount:?}"
        );
    }
}

/// The amounts a quote calculated are the ones the agreement it is accepted into
/// and that agreement's charges report: one price, read back through
/// `DescribeAgreement` and `ListAgreementCharges`.
#[tokio::test]
async fn the_amounts_a_quote_calculated_are_what_the_agreement_reports() {
    let cases = vec![
        (
            "a prepaid purchase of two dimensions",
            vec![upfront_term("cfg", "P12M", &[("Users", 6), ("Admins", 2)])],
        ),
        (
            "a fixed upfront term beside metered pricing",
            vec![
                term("FixedUpfrontPricingTerm"),
                term("UsageBasedPricingTerm"),
            ],
        ),
        (
            "a payment schedule and a recurring price",
            vec![term("PaymentScheduleTerm"), term("RecurringPaymentTerm")],
        ),
        (
            "terms that commit nothing at all",
            vec![term("eula.v1"), term("UsageBasedPricingTerm")],
        ),
        (
            "a purchase the published rates do not price",
            vec![upfront_term("cfg", "P12M", &[(UNPUBLISHED_DIMENSION, 4)])],
        ),
    ];

    for (case, terms) in cases {
        let router = emulator();
        let created = quote_over(&router, case, "ap-amountsRoundTrip", terms).await;
        let quoted_value = created.body["chargeSummary"]["newAgreementValue"].clone();
        let quoted: BTreeMap<String, Value> = expected_charges(&created)
            .iter()
            .map(|charge| {
                (
                    charge["id"].as_str().unwrap_or_default().to_string(),
                    charge.clone(),
                )
            })
            .collect();

        let request_id = created.str_at("/agreementRequestId");
        let accepted = accept(&router, json!({"agreementRequestId": request_id})).await;
        assert_eq!(
            accepted.status, 200,
            "case {case:?}: accepting the quote must succeed, got {} with {:?}",
            accepted.status, accepted.body
        );
        let agreement = accepted.str_at("/agreementId");

        let described = call(
            &router,
            "DescribeAgreement",
            json!({"agreementId": agreement}),
        )
        .await;
        assert_eq!(
            described.status, 200,
            "case {case:?}: describing {agreement} must succeed, got {} with {:?}",
            described.status, described.body
        );
        assert_eq!(
            described.body["estimatedCharges"]["agreementValue"], quoted_value,
            "case {case:?}: the agreement is worth what its quote calculated ({quoted_value:?}), \
             got {:?}",
            described.body["estimatedCharges"]
        );
        assert_eq!(
            described.str_at("/estimatedCharges/currencyCode"),
            CURRENCY,
            "case {case:?}: the agreement is priced in {CURRENCY}, got {:?}",
            described.body["estimatedCharges"]
        );

        let reported = charges(&router, json!({"agreementId": agreement})).await;
        assert_eq!(
            reported.status, 200,
            "case {case:?}: listing the charges of {agreement} must succeed, got {} with {:?}",
            reported.status, reported.body
        );
        let listed = items(&reported);
        assert_eq!(
            listed.len(),
            quoted.len(),
            "case {case:?}: the agreement holds the {} charges its quote minted, got {:?}",
            quoted.len(),
            reported.body
        );
        for charge in &listed {
            let id = charge["id"].as_str().unwrap_or_default().to_string();
            let quote = quoted.get(&id).unwrap_or_else(|| {
                panic!(
                    "case {case:?}: charge {id} was never quoted: {:?}",
                    reported.body
                )
            });
            assert_eq!(
                charge["amount"], quote["amount"],
                "case {case:?}: charge {id} must carry the amount its quote calculated ({:?}), \
                 got {charge:?}",
                quote["amount"]
            );
            assert_eq!(
                charge["currencyCode"].as_str(),
                Some(CURRENCY),
                "case {case:?}: charge {id} is priced in {CURRENCY}, got {charge:?}"
            );
            // "The date and time when the charge will be incurred. This is
            // available only when the charge date is known": a charge timed
            // ON_ACCEPTANCE is incurred when the agreement was accepted.
            if quote["timing"].as_str() == Some(ON_ACCEPTANCE) {
                assert_eq!(
                    charge["time"], described.body["acceptanceTime"],
                    "case {case:?}: charge {id} is incurred on acceptance, got {charge:?}"
                );
            } else {
                assert!(
                    charge["time"].is_null(),
                    "case {case:?}: charge {id} has no known charge date, got {charge:?}"
                );
            }
        }
    }
}

/// An amendment is charged for the units it ADDS, at the published rate of the
/// dimension it adds them to — and its agreement value is the whole agreement,
/// "which includes any amendments".
#[tokio::test]
async fn an_amendment_is_charged_for_the_units_it_adds() {
    let router = emulator();
    let case = "an amendment over an agreement holding 4 Users";

    // The rates the amendment will be measured against, read back from quotes
    // that buy one unit of each dimension for the same contract length.
    let users_rate = single_dimension_amount(&router, case, "Users", "P12M", 1)
        .await
        .unwrap_or_else(|| panic!("case {case:?}: Users is published for P12M"));
    let storage_rate = single_dimension_amount(&router, case, "Storage", "P12M", 1)
        .await
        .unwrap_or_else(|| panic!("case {case:?}: Storage is published for P12M"));

    let source_quote = quote_over(
        &router,
        case,
        "ap-amend1",
        vec![upfront_term("cfg", "P12M", &[("Users", 4)])],
    )
    .await;
    let request_id = source_quote.str_at("/agreementRequestId");
    let accepted = accept(&router, json!({"agreementRequestId": request_id})).await;
    assert_eq!(
        accepted.status, 200,
        "case {case:?}: accepting the source quote must succeed, got {} with {:?}",
        accepted.status, accepted.body
    );
    let source = accepted.str_at("/agreementId");

    // The amendment buys 5 more Users and 100 of a dimension the source does
    // not hold at all.
    let amended = call(
        &router,
        "CreateAgreementRequest",
        json!({
            "intent": "AMEND",
            "sourceAgreementIdentifier": source,
            "requestedTerms": [upfront_term("cfg", "P12M", &[("Users", 9), ("Storage", 100)])],
        }),
    )
    .await;
    assert_eq!(
        amended.status, 200,
        "case {case:?}: the amendment must succeed, got {} with {:?}",
        amended.status, amended.body
    );

    let lines = itemized_charges(&amended);
    assert_eq!(
        lines.len(),
        2,
        "case {case:?}: both dimensions are itemized, got {:?}",
        amended.body
    );
    let added_users = minor(
        case,
        "itemizedCharges[0].incrementalChargeAmount",
        &lines[0]["incrementalChargeAmount"],
    );
    assert_eq!(
        added_users,
        users_rate * 5,
        "case {case:?}: 9 Users over an agreement holding 4 adds 5 units at {}, so {} — got {}",
        as_amount(users_rate),
        as_amount(users_rate * 5),
        as_amount(added_users)
    );
    let added_storage = minor(
        case,
        "itemizedCharges[1].incrementalChargeAmount",
        &lines[1]["incrementalChargeAmount"],
    );
    assert_eq!(
        added_storage,
        storage_rate * 100,
        "case {case:?}: a dimension the source does not hold is charged for all 100 units, so {} \
         — got {}",
        as_amount(storage_rate * 100),
        as_amount(added_storage)
    );

    let charges = expected_charges(&amended);
    let charged = minor(case, "expectedCharges[0].amount", &charges[0]["amount"]);
    assert_eq!(
        charged,
        added_users + added_storage,
        "case {case:?}: the amendment is charged for the units it adds ({}), got {}",
        as_amount(added_users + added_storage),
        as_amount(charged)
    );

    // "The total value of the agreement, which includes any amendments": the 9
    // Users and the 100 Storage the amended agreement holds, not just the units
    // this request added.
    let whole = users_rate * 9 + storage_rate * 100;
    assert_eq!(
        amended.body["chargeSummary"]["newAgreementValue"].as_str(),
        Some(as_amount(whole).as_str()),
        "case {case:?}: the amended agreement is worth {}, got {:?}",
        as_amount(whole),
        amended.body["chargeSummary"]
    );
}

/// No tax estimate is reported, whatever the request asked for: AWS estimates
/// tax from the acceptor's tax registration and address, which this service does
/// not model, and `ChargeSummary` returns its tax members "on a best-effort
/// basis", a null tax amount meaning "tax estimation was unavailable at the time
/// of the request". Asking for one is still served, never refused.
#[tokio::test]
async fn no_tax_estimate_is_reported_whatever_the_request_asked_for() {
    let router = emulator();
    let terms = vec![
        upfront_term("cfg", "P12M", &[("Users", 3)]),
        term("UsageBasedPricingTerm"),
    ];

    // `TaxEstimation`'s two values (`smithy_query(query="shape",
    // shape="TaxEstimation")`), and the request that names neither — for which
    // the member documents the default `DISABLED`.
    for (case, estimation) in [
        ("no tax configuration at all", None),
        ("tax estimation disabled", Some("DISABLED")),
        ("tax estimation enabled", Some("ENABLED")),
    ] {
        let mut request = new_request("ap-tax1", terms.clone());
        if let Some(estimation) = estimation {
            request["taxConfiguration"] = json!({"taxEstimation": estimation});
        }
        let answer = create(&router, request).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: the quote must succeed, got {} with {:?}",
            answer.status, answer.body
        );
        let summary = &answer.body["chargeSummary"];
        assert!(
            !summary["newAgreementValue"].is_null(),
            "case {case:?}: the quote still calculates the amounts it can, got {summary:?}"
        );
        assert!(
            summary["estimatedTaxes"].is_null()
                && summary["newAgreementValueAfterTax"].is_null()
                && summary["invoicingEntity"].is_null(),
            "case {case:?}: no tax or invoicing member is reported, got {summary:?}"
        );
        for charge in expected_charges(&answer) {
            assert!(
                charge["estimatedTaxes"].is_null() && charge["amountAfterTax"].is_null(),
                "case {case:?}: no tax member is reported on a charge, got {charge:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Per-Region price overrides
// ---------------------------------------------------------------------------
//
// The rule these rows add: a rate card item carries the offer's per-Region
// price overrides for its dimension, and the amount a purchase is charged is
// the override of the Region it is quoted in where the card lists one —
// `RateCardItem$regionalPrices` is "the per-Region price overrides for the
// product dimension. When a Regional price is present for an AWS Region, it
// replaces the default `price`. This field is absent when the offer doesn't
// define regional price overrides"
// (`smithy_query(query="shape", shape="RateCardItem,RegionalPrice")`) — so
// everywhere else the card's own `price` applies.

/// The Region an amount is priced in when the caller's credential scope carries
/// no Region label at all — the emulator's documented default, spelled out here
/// rather than imported.
const DEFAULT_REGION: &str = "us-east-1";

/// Regions a row signs for when it wants a Region the item under test publishes
/// no override in: the first of these the item does not override is used, so a
/// row stays about the default price however the offer's overrides change.
///
/// SOURCE: AWS Region labels, as `AWSRegion`
/// (`smithy_query(query="shape", shape="RegionalPrice", member="region")`)
/// spells them.
const CANDIDATE_REGIONS: &[&str] = &["us-east-1", "us-west-2", "sa-east-1", "eu-west-3"];

/// The longest a Region a `RegionalPrice` names may be: `AWSRegion` is
/// `@length(1, 50)` with `^[a-z]{2,3}(-[a-z]+)+-\d+$`.
const LONGEST_REGION: usize = 50;

/// Whether `value` satisfies `AWSRegion`'s modelled pattern and length —
/// spelled out from the model so an override naming text no AWS Region label
/// admits fails these rows.
fn is_aws_region(value: &str) -> bool {
    if value.is_empty() || value.len() > LONGEST_REGION {
        return false;
    }
    let mut parts = value.split('-');
    let Some(first) = parts.next() else {
        return false;
    };
    if !(2..=3).contains(&first.len()) || !first.chars().all(|c| c.is_ascii_lowercase()) {
        return false;
    }
    let rest: Vec<&str> = parts.collect();
    // At least one `-<letters>` group, then the trailing `-<digits>`.
    if rest.len() < 2 {
        return false;
    }
    let (digits, letters) = rest.split_last().expect("at least two groups");
    !digits.is_empty()
        && digits.chars().all(|c| c.is_ascii_digit())
        && letters
            .iter()
            .all(|group| !group.is_empty() && group.chars().all(|c| c.is_ascii_lowercase()))
}

/// A credential scope naming this service's signing name — which is what routes
/// the request — with `region` in its Region field, exactly as a client wrote
/// it. The Region a purchase is quoted in is the one its caller signed for.
fn signed_for(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 \
         Credential=AKIDEXAMPLE/20260101/{region}/aws-marketplace/aws4_request, \
         SignedHeaders=host, Signature=abc"
    )
}

/// The quote over one configured prepaid term, created by a caller signing for
/// `region`.
async fn quote_in(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    selector: &str,
    dimensions: &[(&str, i32)],
) -> Answer {
    let answer = call_as(
        router,
        "CreateAgreementRequest",
        new_request(
            "ap-regionalRates",
            vec![upfront_term("cfg", selector, dimensions)],
        ),
        &signed_for(region),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: the quote must succeed, got {} with {:?}",
        answer.status, answer.body
    );
    answer
}

/// The rate cards the quote `request_id` proposes for its configured prepaid
/// term, as `GetAgreementRequestTerms` reports them — the offer's own card,
/// read back through the API rather than from the emulator's catalog.
async fn proposed_rate_cards(
    router: &MultiServiceRouter,
    case: &str,
    request_id: &str,
    authorization: &str,
) -> Vec<Value> {
    let answer = call_as(
        router,
        "GetAgreementRequestTerms",
        json!({"agreementRequestId": request_id}),
        authorization,
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: reporting the proposed terms must succeed, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body["terms"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|term| term["configurableUpfrontPricingTerm"]["rateCards"].as_array().cloned())
        .flatten()
        .collect()
}

/// A rate card item carries the per-Region overrides the offer defines for its
/// dimension and nothing else: the member is absent for a dimension the offer
/// does not override, every override names an AWS Region at a money price, and
/// the card is the OFFER's — the same list whatever Region the caller reading it
/// signed for.
#[tokio::test]
async fn a_rate_card_item_reports_the_offers_per_region_price_overrides() {
    let router = emulator();
    // The rows differ in the Region the reading caller signed for: the default
    // one, one the offer publishes overrides in, and a scope whose Region field
    // is no Region label at all.
    let scopes = [
        ("read by a caller in the default Region", DEFAULT_REGION),
        ("read by a caller in an overridden Region", "eu-central-1"),
        ("read by a caller signing for no Region", "not a region"),
    ];
    let mut reported: Vec<Vec<Value>> = Vec::new();
    for (case, region) in scopes {
        let quote = quote_in(&router, case, region, "P12M", &[("Users", 1)]).await;
        let cards = proposed_rate_cards(
            &router,
            case,
            &quote.str_at("/agreementRequestId"),
            &signed_for(region),
        )
        .await;
        assert!(
            !cards.is_empty(),
            "case {case:?}: the prepaid term proposes rate cards, got {:?}",
            quote.body
        );
        let mut overridden = 0usize;
        let mut plain = 0usize;
        for card in &cards {
            let selector = card["selector"]["value"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            for item in card["rateCard"].as_array().cloned().unwrap_or_default() {
                let dimension = item["dimensionKey"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                let row = format!("{case}, {dimension} at {selector}");
                let default = minor(&row, "price", &item["price"]);
                if item["regionalPrices"].is_null() {
                    plain += 1;
                    continue;
                }
                let overrides = item["regionalPrices"]
                    .as_array()
                    .cloned()
                    .unwrap_or_else(|| {
                        panic!("case {row:?}: regionalPrices is a list, got {item:?}")
                    });
                assert!(
                    !overrides.is_empty() && overrides.len() <= 50,
                    "case {row:?}: RegionalPriceList is @length(1, 50), got {overrides:?}"
                );
                let mut regions: BTreeSet<String> = BTreeSet::new();
                for price in &overrides {
                    let region_of = price["region"].as_str().unwrap_or_default();
                    assert!(
                        is_aws_region(region_of),
                        "case {row:?}: an override names an AWS Region, got {price:?}"
                    );
                    assert!(
                        regions.insert(region_of.to_string()),
                        "case {row:?}: one override per Region, got {overrides:?}"
                    );
                    let overridden_price = minor(&row, "regionalPrices[].price", &price["price"]);
                    assert!(
                        overridden_price > 0,
                        "case {row:?}: an override prices one unit, got {price:?} against the \
                         default {default}"
                    );
                }
                overridden += 1;
            }
        }
        assert!(
            overridden > 0,
            "case {case:?}: the offer defines per-Region overrides, got {cards:?}"
        );
        assert!(
            plain > 0,
            "case {case:?}: a dimension the offer does not override carries no regionalPrices \
             member at all, got {cards:?}"
        );
        reported.push(cards);
    }
    for (index, (case, _)) in scopes.iter().enumerate().skip(1) {
        assert_eq!(
            reported[index], reported[0],
            "case {case:?}: a rate card belongs to the offer, so it reports the same overrides \
             to every caller, got {:?} against {:?}",
            reported[index], reported[0]
        );
    }
}

/// A dimension is charged at the Region's own override where the card publishes
/// one for the Region the quote was created in, and at the card's default price
/// in every other Region — over every contract length the card publishes, every
/// dimension on it, and the quantities a row buys.
#[tokio::test]
async fn a_charge_is_computed_at_the_override_of_the_region_it_is_quoted_in() {
    let router = emulator();
    // The table is the card the service itself publishes, read back over the
    // API: one row per (contract length, dimension, Region) it prices, so no row
    // can agree with a handler that prices from something else.
    let published = quote_in(
        &router,
        "the published cards",
        DEFAULT_REGION,
        "P12M",
        &[("Users", 1)],
    )
    .await;
    let cards = proposed_rate_cards(
        &router,
        "the published cards",
        &published.str_at("/agreementRequestId"),
        &signed_for(DEFAULT_REGION),
    )
    .await;
    assert!(
        !cards.is_empty(),
        "the prepaid term proposes rate cards, got {:?}",
        published.body
    );

    let mut rows = 0usize;
    let mut overrides_priced = 0usize;
    for card in &cards {
        let selector = card["selector"]["value"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        for item in card["rateCard"].as_array().cloned().unwrap_or_default() {
            let dimension = item["dimensionKey"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            let default = minor(&dimension, "price", &item["price"]);
            let overrides = item["regionalPrices"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            // One case per Region the item overrides, each at that override …
            let mut cases: Vec<(String, u128)> = overrides
                .iter()
                .map(|price| {
                    (
                        price["region"].as_str().unwrap_or_default().to_string(),
                        minor(&dimension, "regionalPrices[].price", &price["price"]),
                    )
                })
                .collect();
            // … and one in a Region it does not, where the default applies.
            let listed: BTreeSet<&str> = overrides
                .iter()
                .filter_map(|price| price["region"].as_str())
                .collect();
            let unlisted = CANDIDATE_REGIONS
                .iter()
                .copied()
                .find(|region| !listed.contains(region))
                .unwrap_or_else(|| {
                    panic!("{dimension} at {selector}: expected a Region without an override")
                });
            cases.push((unlisted.to_string(), default));

            for (index, (region, unit_rate)) in cases.into_iter().enumerate() {
                // The units a row buys vary with its position, so a row cannot
                // pass on a handler that reports the unit rate itself.
                let quantity = 1 + i32::try_from(index).unwrap_or_default() * 3;
                let row = format!("{dimension} at {selector} in {region} for {quantity} units");
                let quote = quote_in(
                    &router,
                    &row,
                    &region,
                    &selector,
                    &[(dimension.as_str(), quantity)],
                )
                .await;
                let expected = unit_rate * u128::from(quantity.unsigned_abs());
                let charges = expected_charges(&quote);
                assert_eq!(
                    charges.len(),
                    1,
                    "case {row:?}: one charged term implies one charge, got {:?}",
                    quote.body
                );
                assert_eq!(
                    minor(&row, "expectedCharges[0].amount", &charges[0]["amount"]),
                    expected,
                    "case {row:?}: expected {} at the rate that Region is priced at, got {:?}",
                    as_amount(expected),
                    charges[0]
                );
                let itemized = itemized_charges(&quote);
                assert_eq!(
                    itemized.len(),
                    1,
                    "case {row:?}: one configured dimension implies one itemized charge, got {:?}",
                    quote.body
                );
                assert_eq!(
                    minor(
                        &row,
                        "itemizedCharges[0].incrementalChargeAmount",
                        &itemized[0]["incrementalChargeAmount"]
                    ),
                    expected,
                    "case {row:?}: the itemized charge is the same amount, got {:?}",
                    itemized[0]
                );
                assert_eq!(
                    quote.body["chargeSummary"]["newAgreementValue"].as_str(),
                    Some(as_amount(expected).as_str()),
                    "case {row:?}: the agreement value totals that charge, got {:?}",
                    quote.body["chargeSummary"]
                );
                if unit_rate != default {
                    overrides_priced += 1;
                }
                rows += 1;
            }
        }
    }
    assert!(
        overrides_priced > 0,
        "the offer publishes overrides that differ from the default price, so some row must \
         have been charged at one: {cards:?}"
    );
    assert!(
        rows > cards.len(),
        "expected a row per published length, dimension and Region, got {rows} over {:?}",
        cards.len()
    );
}
