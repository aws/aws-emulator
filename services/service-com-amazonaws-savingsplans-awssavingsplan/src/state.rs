// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-savingsplans-awssavingsplan service.
//!
//! Three things live here, plus the helpers more than one operation needs:
//!
//! * the seeded, read-only **offering catalog** `DescribeSavingsPlansOfferings`
//!   searches and `CreateSavingsPlan` buys from. It is built in `Default`, so
//!   every offering id is stable for the life of the store;
//! * the **Savings Plans** this emulator has sold, in purchase order so paging is
//!   stable, with an id and an ARN index over them;
//! * the **idempotency** record, so a replayed `clientToken` answers with the plan
//!   the first call created instead of creating a second one.

use std::collections::{BTreeMap, HashMap};
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};

// ---------------------------------------------------------------------------
// Wire vocabulary
// ---------------------------------------------------------------------------

/// `SavingsPlanState` members this slice produces. The full value set is
/// [`crate::ops::types::SAVINGS_PLAN_STATE_VALUES`].
pub const STATE_PAYMENT_PENDING: &str = "payment-pending";
/// See [`STATE_PAYMENT_PENDING`].
pub const STATE_ACTIVE: &str = "active";
/// See [`STATE_PAYMENT_PENDING`].
pub const STATE_QUEUED: &str = "queued";
/// See [`STATE_PAYMENT_PENDING`].
pub const STATE_QUEUED_DELETED: &str = "queued-deleted";
/// See [`STATE_PAYMENT_PENDING`].
pub const STATE_RETURNED: &str = "returned";
/// See [`STATE_PAYMENT_PENDING`]. The state a plan reaches when its term ends.
pub const STATE_RETIRED: &str = "retired";

/// `SavingsPlanPaymentOption` members (model enum `SavingsPlanPaymentOption`).
pub const PAYMENT_ALL_UPFRONT: &str = "All Upfront";
/// See [`PAYMENT_ALL_UPFRONT`].
pub const PAYMENT_PARTIAL_UPFRONT: &str = "Partial Upfront";
/// See [`PAYMENT_ALL_UPFRONT`].
pub const PAYMENT_NO_UPFRONT: &str = "No Upfront";

/// `SavingsPlanOfferingPropertyKey` members (model enum
/// `SavingsPlanOfferingPropertyKey`): the only two property names an offering
/// carries.
pub const PROPERTY_INSTANCE_FAMILY: &str = "instanceFamily";
/// See [`PROPERTY_INSTANCE_FAMILY`].
pub const PROPERTY_REGION: &str = "region";

/// The ARN namespace and sigv4 signing name of this service, as the model's
/// `aws.api#service` trait gives them.
pub const SERVICE_NAMESPACE: &str = "savingsplans";

// ---------------------------------------------------------------------------
// Clock
// ---------------------------------------------------------------------------

/// How long after the purchase call a Savings Plan leaves `payment-pending` for
/// `active`.
///
/// A chosen span, not a documented one: the contract the emulator owes is only
/// that the transition is not instantaneous, so a caller that describes the plan
/// straight after purchasing it still observes `payment-pending`.
pub const ACTIVATION_DELAY_MILLIS: i64 = 500;

/// The return window a purchased Savings Plan has.
///
/// AWS documents it ("Returning a Savings Plan", AWS Savings Plans User Guide:
/// "you can return a Savings Plan that you purchased in the last 7 days (168
/// hours)"), so the value is the service's, not ours, and it is what
/// `SavingsPlan.returnableUntil` reports.
pub const RETURN_WINDOW_MILLIS: i64 = 7 * 24 * 60 * 60 * 1000;

/// How long before `start` plus the term duration a term's reported `end` falls:
/// one second, so `end` is the last instant the term covers.
pub const TERM_END_INCLUSIVE_MILLIS: i64 = 1_000;

/// Wall-clock now, in milliseconds since the Unix epoch.
///
/// Total by construction: a clock before 1970, or one too far past it to fit in
/// `i64` milliseconds, answers with the epoch rather than panicking on the
/// request path. Neither is reachable on a host whose clock is set at all, and
/// every instant derived from this value is computed with saturating
/// arithmetic, so the epoch fallback cannot propagate into an overflow.
pub fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_millis()).ok())
        .unwrap_or_default()
}

/// How many fractional digits a rendered time carries: milliseconds.
pub const TIME_FRACTION_DIGITS: usize = 3;

/// Render epoch milliseconds the way this service renders a time in a `String`
/// member (`start`, `end`, `returnableUntil`): RFC 3339 / ISO 8601 in UTC, at
/// FIXED WIDTH with exactly [`TIME_FRACTION_DIGITS`] fractional digits.
///
/// The width is part of the value. `SavingsPlan.start`/`end`/`returnableUntil`
/// target `String`, not `timestamp`, so the service — not the client's
/// serializer — chooses the spelling, and `aws_smithy_types`' RFC-3339 writer
/// omits the fractional part entirely on a whole second. A variable-width
/// rendering makes a parser configured for millisecond precision fail, and
/// makes two instants of one record sort differently as strings than in time
/// (`...:52Z` sorts AFTER `...:52.100Z`). Fixed width keeps lexicographic and
/// chronological order the same.
pub fn format_millis(millis: i64) -> String {
    let whole_seconds = millis.div_euclid(1000);
    let fraction = millis.rem_euclid(1000);
    let rendered = ::aws_smithy_types::DateTime::from_secs(whole_seconds)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        // A whole-second instant always renders; the fallback keeps this
        // total rather than turning a clock oddity into a 500.
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string());
    match rendered.strip_suffix('Z') {
        Some(seconds) => format!(
            "{seconds}.{fraction:0width$}Z",
            width = TIME_FRACTION_DIGITS
        ),
        None => rendered,
    }
}

/// Read a time a request carries back to epoch milliseconds, so a filter on an
/// instant compares INSTANTS rather than two spellings of one.
///
/// Both ISO-8601 spellings are accepted: the `Z` form this service renders and
/// the explicit-offset form a client may send.
pub fn parse_millis(value: &str) -> Option<i64> {
    use ::aws_smithy_types::date_time::Format;

    [Format::DateTime, Format::DateTimeWithOffset]
        .into_iter()
        .find_map(|format| ::aws_smithy_types::DateTime::from_str(value, format).ok())
        .and_then(|time| time.to_millis().ok())
}

/// Truncate epoch milliseconds to whole seconds: a purchase time is
/// second-granular (`DateTime` is documented as `YYYY-MM-DDTHH:MM:SSZ`).
///
/// Public because this is the instant a purchase is HELD at, so anything that
/// has to agree with the stored record about which instant a request named — a
/// handler's idempotency fingerprint, say — normalises it through this one
/// function rather than through a second copy of the rule.
///
/// Saturating, not plain arithmetic: the value can come from the request, and a
/// total function here is what keeps an extreme one from overflowing rather than
/// being answered.
pub fn truncate_to_second(millis: i64) -> i64 {
    millis.saturating_sub(millis.rem_euclid(1000))
}

/// How far either side of now a caller-supplied `purchaseTime` may fall.
///
/// A chosen bound, not a documented one: the longest term the catalog sells is
/// three years, so a purchase dated a decade out is not a purchase. Refusing it
/// at the edge keeps a request-derived instant away from the term and
/// return-window arithmetic entirely.
pub const MAX_PURCHASE_TIME_SKEW_MILLIS: i64 = 10 * 365 * 24 * 60 * 60 * 1000;

/// Refuse a `purchaseTime` further than [`MAX_PURCHASE_TIME_SKEW_MILLIS`] from
/// now, in either direction.
pub fn check_purchase_time(purchase_millis: i64, now_millis: i64) -> Result<(), String> {
    let skew = purchase_millis.saturating_sub(now_millis).saturating_abs();
    if skew > MAX_PURCHASE_TIME_SKEW_MILLIS {
        return Err(format!(
            "purchaseTime must be within {MAX_PURCHASE_TIME_SKEW_MILLIS} ms of now, got '{}'",
            format_millis(purchase_millis)
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Amounts
// ---------------------------------------------------------------------------

/// How many fractional digits a BILLED `Amount` carries.
///
/// `Amount` targets `string` in the model, so the rendering IS the wire value:
/// a client that compares, displays or hashes it sees exactly these digits,
/// rather than a number it is free to reformat. Every amount this service
/// BILLS — a plan's `commitment`, its `upfrontPaymentAmount` and its
/// `recurringPaymentAmount`, each computed from the commitment the caller
/// bought — is rendered at this one fixed scale, eight fractional digits, the
/// scale AWS bills Savings Plans commitments and payments at, so no billed
/// amount is ever reported in two spellings.
///
/// A rate is NOT one of them: a `SavingsPlanRate`/`SavingsPlanOfferingRate`
/// `rate` is a quotation out of the price list, and a price list quotes each
/// product at its own precision — so a rate is held and spelled as the catalog
/// holds it ([`format_price`]) rather than padded out to this scale. It is also
/// the scale amounts are HELD at internally ([`AMOUNT_SCALE`]), which is why a
/// quoted price is still an exact integer number of these units.
pub const AMOUNT_WIRE_DIGITS: usize = 8;

/// The scale an `Amount` is held at: the wire scale, so every value the service
/// accepts is an exact integer, no rounding creeps into a stored amount, and
/// the stored value and the rendered one are the same number.
pub const AMOUNT_SCALE: i128 = 100_000_000;

/// The most digits an `Amount` a REQUEST carries may have after the decimal
/// point: "You cannot specify more than five digits after the decimal point".
/// The service's own rendering is wider (see [`AMOUNT_WIRE_DIGITS`]).
pub const MAX_AMOUNT_DIGITS: usize = 5;

/// The magnitude a well-formed decimal amount too large to hold at
/// [`AMOUNT_SCALE`] is read at: the largest WHOLE amount the held scale can
/// represent.
///
/// How many digits a number is spelled with says nothing about its FORM: a
/// decimal number wider than this scale can hold exactly is still a decimal
/// number, so it is read as the enormous amount it is and refused by whatever
/// rule governs its member's magnitude — every bound this service publishes is
/// many orders of magnitude below this — rather than reported as malformed. It
/// is deliberately a whole amount, so a member whose rule asks for a whole
/// number is still answered with the range it missed.
pub const AMOUNT_OVERFLOW_UNITS: i128 = (i128::MAX / AMOUNT_SCALE) * AMOUNT_SCALE;

/// The smallest hourly commitment `CreateSavingsPlan` accepts, from the
/// documented rule on `commitment`: "a value between 0.001 and 1 million".
pub const MIN_COMMITMENT_UNITS: i128 = AMOUNT_SCALE / 1000;
/// The largest hourly commitment, from the same sentence.
pub const MAX_COMMITMENT_UNITS: i128 = 1_000_000 * AMOUNT_SCALE;

/// Parse a REQUEST's `Amount` into [`AMOUNT_SCALE`] units, refusing only a value
/// that is not a plain decimal number, or one spelled at more than
/// [`MAX_AMOUNT_DIGITS`] decimal places.
///
/// `Amount` is an unconstrained string in the model, so the shape of the value
/// is the handler's to enforce. Every `Amount` a request carries is a member the
/// documentation gives a RANGE — an hourly commitment between 0.001 and 1
/// million, an up-front payment between 50 and 99 percent of the plan's total
/// value — so the sign is read as part of the number ([`Negative::Read`]) and a
/// value below the member's floor is reported with that bound rather than called
/// malformed. For the same reason a number too large to hold exactly comes back
/// as [`AMOUNT_OVERFLOW_UNITS`]: both ends of the number line are left to the
/// member's own rule to refuse on its magnitude.
pub fn parse_amount(member: &str, value: &str) -> Result<i128, String> {
    parse_decimal_amount(member, value, MAX_AMOUNT_DIGITS, Negative::Read)
}

/// Read an amount the way a FILTER value is read: at the full wire scale, since
/// the value a client filters with is the one this service rendered for it
/// ([`AMOUNT_WIRE_DIGITS`] fractional digits), which is wider than the
/// [`MAX_AMOUNT_DIGITS`] a request may SPECIFY.
///
/// A filter value is COMPARED with an amount this service rendered, never
/// refused by a bound, and no amount it renders carries a sign — so a signed
/// spelling cannot be one of those renderings and is simply not a value to
/// compare ([`Negative::Refused`]).
pub fn parse_amount_at_wire_scale(value: &str) -> Option<i128> {
    parse_decimal_amount("amount", value, AMOUNT_WIRE_DIGITS, Negative::Refused).ok()
}

/// How a leading `-` is read by [`parse_decimal_amount`].
///
/// A value a documented RANGE governs reads a negative amount as the number it
/// is, so the clause a caller gets names the bound the value missed; a value
/// that is only ever COMPARED with an amount this service rendered takes no sign
/// at all, because no rendering of a stored amount carries one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Negative {
    /// Not an amount to compare with (the comparison's caller discards the
    /// reason, there being no bound to report).
    Refused,
    /// Read as the negative number it is.
    Read,
}

/// Parse a plain decimal amount into [`AMOUNT_SCALE`] units, admitting at most
/// `max_digits` fractional digits and reading a leading `-` as `negative` says.
fn parse_decimal_amount(
    member: &str,
    value: &str,
    max_digits: usize,
    negative: Negative,
) -> Result<i128, String> {
    let trimmed = value.trim();
    let invalid = || format!("{member} must be a decimal amount, got '{value}'");
    // The sign is taken off the front before the digit rules are applied, so a
    // negative amount is read by exactly the rules a positive one is.
    let (sign, magnitude) = match trimmed.strip_prefix('-') {
        Some(magnitude) if negative == Negative::Read => (-1, magnitude),
        Some(_) => return Err(invalid()),
        None => (1, trimmed),
    };
    let (whole, fraction) = match magnitude.split_once('.') {
        Some((whole, fraction)) => (whole, fraction),
        None => (magnitude, ""),
    };
    let digits = |text: &str| !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit());
    if !digits(whole) || (magnitude.contains('.') && !digits(fraction)) {
        return Err(invalid());
    }
    if fraction.len() > max_digits {
        return Err(format!(
            "{member} cannot specify more than {max_digits} digits after the decimal point, got '{value}'"
        ));
    }
    // The value's FORM is settled above. Everything below is arithmetic on a
    // value already known to be a decimal number, so a magnitude that does not
    // fit the held scale is read as an amount beyond every published bound
    // ([`AMOUNT_OVERFLOW_UNITS`]) and left to the member's range to refuse —
    // never reclassified as malformed.
    let units = whole
        .parse::<i128>()
        .ok()
        .and_then(|whole| whole.checked_mul(AMOUNT_SCALE))
        .and_then(|units| {
            if fraction.is_empty() {
                return Some(units);
            }
            // Right-pad to the held scale, so ".1" and ".10000000" are one value.
            let padded = format!("{fraction:0<width$}", width = AMOUNT_WIRE_DIGITS);
            units.checked_add(padded.parse::<i128>().ok()?)
        })
        .unwrap_or(AMOUNT_OVERFLOW_UNITS);
    Ok(sign * units)
}

/// Parse and validate `CreateSavingsPlan`'s `commitment` against the rule the
/// model documents for it: a value between 0.001 and 1 million, with at most
/// five digits after the decimal point.
///
/// A value OUTSIDE that interval is a well-formed amount the member's range
/// refuses, so the clause names the member and the bound it missed — the floor
/// for anything below it, including zero and a negative amount, and the ceiling
/// for anything above. Only a value that is not a decimal number at all, or one
/// spelled at more digits than a request may carry, is refused on its form.
pub fn parse_commitment(value: &str) -> Result<i128, String> {
    let units = parse_amount("commitment", value)?;
    if units < MIN_COMMITMENT_UNITS {
        return Err(format!(
            "'commitment' cannot be less than {}",
            format_bound(MIN_COMMITMENT_UNITS)
        ));
    }
    if units > MAX_COMMITMENT_UNITS {
        return Err(format!(
            "'commitment' cannot be greater than {}",
            format_bound(MAX_COMMITMENT_UNITS)
        ));
    }
    Ok(units)
}

/// A BOUND as a message quotes it: the amount in its shortest exact decimal
/// spelling — trailing fractional zeros dropped, and the decimal point with them
/// when the amount is whole.
///
/// A bound is a number the service publishes rather than a value a caller sent,
/// and the documentation writes it this way ("a value between 0.001 and 1
/// million"), so it is quoted this way too. Every amount the service BILLS still
/// goes through [`format_amount`]'s one fixed scale.
pub fn format_bound(units: i128) -> String {
    shortest_exact(format_amount(units), 0)
}

/// `rendered` — a fixed-scale amount as [`format_amount`] writes it — in the
/// shortest EXACT decimal spelling of the same number: its insignificant
/// trailing fractional zeros dropped, keeping at least `min_digits` fractional
/// digits, and the decimal point with them when none are kept.
///
/// The crate's one home for that spelling rule, since its two callers differ
/// only in how far they trim: a BOUND a message quotes drops the point entirely
/// on a whole amount ([`format_bound`]), while a PRICE the catalog quotes keeps a
/// fractional digit ([`format_price`]) so every price carries a point.
fn shortest_exact(rendered: String, min_digits: usize) -> String {
    let Some((whole, fraction)) = rendered.split_once('.') else {
        return rendered;
    };
    // `trim_end_matches` leaves a PREFIX of `fraction`, so keeping `kept` of its
    // own digits is the same number spelled at that many places.
    let kept = fraction
        .trim_end_matches('0')
        .len()
        .max(min_digits.min(fraction.len()));
    if kept == 0 {
        return whole.to_string();
    }
    format!("{whole}.{}", &fraction[..kept])
}

/// Render [`AMOUNT_SCALE`] units as a BILLED `Amount`: FIXED WIDTH, always with
/// a decimal point and exactly [`AMOUNT_WIRE_DIGITS`] fractional digits.
///
/// Trailing zeros are significant here, not noise: see [`AMOUNT_WIRE_DIGITS`].
/// Total over its argument — the magnitude is taken unsigned, so no value of
/// `i128` can turn a response into a panic.
pub fn format_amount(units: i128) -> String {
    let sign = if units < 0 { "-" } else { "" };
    let magnitude = units.unsigned_abs();
    let scale = AMOUNT_SCALE.unsigned_abs();
    format!(
        "{sign}{}.{:0width$}",
        magnitude / scale,
        magnitude % scale,
        width = AMOUNT_WIRE_DIGITS
    )
}

/// Round `units` up to a whole amount (no fractional part).
pub fn ceil_to_whole(units: i128) -> i128 {
    units
        .saturating_add(AMOUNT_SCALE - 1)
        .saturating_div(AMOUNT_SCALE)
        .saturating_mul(AMOUNT_SCALE)
}

/// Round `units` down to a whole amount (no fractional part).
pub fn floor_to_whole(units: i128) -> i128 {
    units.saturating_div(AMOUNT_SCALE).saturating_mul(AMOUNT_SCALE)
}

/// Whether `units` is a whole amount — the form `upfrontPaymentAmount` must
/// take ("a whole number between 50 and 99 percent of the total value").
pub fn is_whole(units: i128) -> bool {
    units % AMOUNT_SCALE == 0
}

/// Divide `units` by `divisor`, rounding to the nearest unit: the recurring
/// payment is the balance spread over the term's hours, and the service holds
/// it at [`AMOUNT_SCALE`].
///
/// Saturating, like every other amount helper here: a value this module reads an
/// over-large request amount as ([`AMOUNT_OVERFLOW_UNITS`]) is divided rather
/// than overflowing before the member's own range refuses it.
pub fn divide_rounded(units: i128, divisor: i128) -> i128 {
    if divisor == 0 {
        return 0;
    }
    units.saturating_add(divisor / 2).saturating_div(divisor)
}

// ---------------------------------------------------------------------------
// Quoted prices
// ---------------------------------------------------------------------------

/// How many fractional digits the catalog QUOTES `units` at: the digits its
/// shortest exact decimal spelling carries.
///
/// A price list quotes each product's price at that product's own precision — an
/// instance-hour at a thousandth of a dollar, a GB-second far finer — and the
/// quoted price is what fixes it. Reading the precision off the price itself is
/// what keeps a rate DERIVED from that price in step with it: both are reported
/// at the precision the price list publishes, rather than at the billing scale
/// [`AMOUNT_WIRE_DIGITS`] that a plan's commitment and payments are held at.
///
/// Read off the one spelling rule ([`shortest_exact`]), so the digits this
/// counts are exactly the digits [`format_price`] writes.
pub fn quoted_digits(units: i128) -> usize {
    format_amount(units)
        .split_once('.')
        .map_or(0, |(_, fraction)| fraction.trim_end_matches('0').len())
}

/// The smallest amount a price quoted at `digits` fractional digits can spell,
/// in [`AMOUNT_SCALE`] units.
pub fn quoted_unit(digits: usize) -> i128 {
    let mut unit = AMOUNT_SCALE;
    for _ in 0..digits.min(AMOUNT_WIRE_DIGITS) {
        unit /= 10;
    }
    unit
}

/// `units` as a price quoted at `digits` fractional digits: rounded to the
/// nearest quoted unit, and never to nothing — a discounted price is still a
/// price, so a value below half a quoted unit is reported as the smallest price
/// that precision can spell.
pub fn quote_price_units(units: i128, digits: usize) -> i128 {
    let unit = quoted_unit(digits);
    let rounded = divide_rounded(units, unit).saturating_mul(unit);
    if rounded == 0 && units > 0 {
        return unit;
    }
    rounded
}

/// Render `units` as the catalog SPELLS a price it holds: the shortest EXACT
/// decimal — no digit beyond the precision the price is held at — with at least
/// one fractional digit, so every price this service reports carries a decimal
/// point.
///
/// `Amount` targets `string`, so the spelling is the wire value: a price list
/// quotes a price at its own precision, and padding it out to the billing scale
/// [`format_amount`] renders a commitment at would report digits the catalog
/// does not hold. The trimming is the crate's one spelling rule
/// ([`shortest_exact`]), which [`format_bound`] shares.
pub fn format_price(units: i128) -> String {
    shortest_exact(format_amount(units), 1)
}

// ---------------------------------------------------------------------------
// Pagination
// ---------------------------------------------------------------------------

/// Encode a page offset into an opaque `PaginationToken`.
///
/// Hex keeps the token inside the model's `^[A-Za-z0-9/=\+]+$` pattern, and
/// `kind` keeps a token minted by one paginator from resuming another's page.
pub fn encode_page_token(kind: &str, offset: usize) -> String {
    hex::encode(format!("{kind}:{offset}"))
}

/// Decode a token [`encode_page_token`] produced, or `None` when it was not
/// produced by this paginator.
///
/// A cursor is opaque, so the only tokens this answers an offset for are the
/// ones [`encode_page_token`] itself writes: the offset is read back, and the
/// token is honoured only when re-minting that offset reproduces it
/// byte-for-byte. That closes every spelling the mint cannot write but
/// `usize::from_str` would still accept (a leading `+`, leading zeros) and every
/// re-encoding of the same payload (upper-case hex), each of which would
/// otherwise resume a walk from a cursor this service never handed out.
pub fn decode_page_token(kind: &str, token: &str) -> Option<usize> {
    let raw = hex::decode(token).ok()?;
    let text = String::from_utf8(raw).ok()?;
    let offset: usize = text.strip_prefix(kind)?.strip_prefix(':')?.parse().ok()?;
    (encode_page_token(kind, offset) == token).then_some(offset)
}

/// The offset a request's `nextToken` resumes a `kind` walk at, or the message
/// with which this service refuses a cursor that paginator never minted.
///
/// One rule in ONE place, on top of the token format
/// [`encode_page_token`]/[`decode_page_token`] own: an absent `nextToken` means
/// the first result, and a token no `kind` page handed out is refused with this
/// sentence. Like [`savings_plan_id_or_refusal`] below, it hands back the
/// MESSAGE and each paginated handler only wraps it in its own operation's
/// `ValidationException` variant, so the default, the error and the wording
/// cannot drift between the operations that page.
pub fn page_offset(kind: &str, next_token: Option<&str>) -> Result<usize, String> {
    // An EMPTY token is the service's own way of saying "no continuation" (see
    // [`search_next_token`]), so a caller that echoes the token a finished walk
    // handed it back asks for the first result rather than for a cursor this
    // service never minted.
    match next_token.filter(|token| !token.is_empty()) {
        Some(token) => decode_page_token(kind, token)
            .ok_or_else(|| "nextToken is not a token this operation issued".to_string()),
        None => Ok(0),
    }
}

/// One page of `items` starting at `offset`, plus the offset of the next page
/// when one remains. `limit` of `None` means "every remaining item".
pub fn paginate<T: Clone>(
    items: &[T],
    offset: usize,
    limit: Option<usize>,
) -> (Vec<T>, Option<usize>) {
    let start = offset.min(items.len());
    let end = match limit {
        Some(limit) => start.saturating_add(limit).min(items.len()),
        None => items.len(),
    };
    let page = items[start..end].to_vec();
    let next = (end < items.len()).then_some(end);
    (page, next)
}

/// One window of a LAZILY walked result: the page of `items` at `offset`, plus
/// whether the walk still has something past that page.
///
/// The single home of the page-window idiom every catalog walk in this crate
/// needs. The walk is advanced exactly one item past the page: that item is what
/// says more remains, and it is dropped without being reported, so one request
/// only ever costs the page it asked for. `items` must therefore be the walk
/// BEFORE any per-row projection, so a row outside the page is never built.
pub fn page_window<T>(
    items: impl Iterator<Item = T>,
    offset: usize,
    page_size: usize,
) -> (Vec<T>, bool) {
    let mut window: Vec<T> = items
        .skip(offset)
        .take(page_size.saturating_add(1))
        .collect();
    let more_remain = window.len() > page_size;
    window.truncate(page_size);
    (window, more_remain)
}

/// One page of a SEARCH over a catalog this service owns: the window
/// [`page_window`] serves, plus the cursor that answer carries.
///
/// The cursor rule, in one place for every such search: an answer carries a
/// cursor only while matches remain PAST the page it served, so the answer that
/// exhausts the matches carries no continuation — which is what `nextToken`
/// being "null when there are no more results to return" says. A page that
/// already held every match therefore ends the walk, whether or not it was the
/// first answer, so the standard paginator stops on it instead of issuing an
/// extra call per search against a cursor that cannot advance.
pub fn search_page<T>(
    items: impl Iterator<Item = T>,
    offset: usize,
    page_size: usize,
) -> (Vec<T>, Option<usize>) {
    let (page, more_remain) = page_window(items, offset, page_size);
    let reached = offset.saturating_add(page.len());
    let next = more_remain.then_some(reached);
    (page, next)
}

/// The `nextToken` a catalog search's answer reports for the cursor
/// [`search_page`] handed back: the token that resumes the walk at `next`, or
/// the EMPTY token when the page served exhausted the matches.
///
/// One spelling of "no continuation" for every search of this service's
/// catalogs, which is how the recorded answers spell it: the member is present
/// and empty rather than absent, and [`page_offset`] reads that empty token back
/// as the first result, so echoing it is harmless.
pub fn search_next_token(kind: &str, next: Option<usize>) -> String {
    next.map(|offset| encode_page_token(kind, offset))
        .unwrap_or_default()
}

/// The page the offering and offering-rate searches serve when the caller names
/// no size.
///
/// `DescribeSavingsPlansOfferings$maxResults` and
/// `DescribeSavingsPlansOfferingRates$maxResults` are `@required` with
/// `@default: 0` and a `PageSize` range of 0..1000, so 0 is "no size asked for"
/// and the service picks one: a search of a catalog the service owns comes back
/// as one page of it, resumable through `nextToken`, never as the whole catalog.
/// `DescribeSavingsPlanRates$maxResults` is optional rather than defaulted, and
/// an absent one says the same thing about the same catalog, so the plan-rate
/// search serves this page too.
///
/// A chosen bound, not a documented one: AWS publishes no default page size for
/// this search. What the recorded answers show is only that a search WAS
/// answered with one page and a cursor the next call resumed, which is the part
/// this constant exists to keep true.
///
/// The seeded catalog is deliberately larger than this, so the page is
/// observable: `a_search_that_names_no_page_size_gets_the_services_own_page`
/// asserts a search that names no size answers exactly one page of this many
/// offerings, and fails loudly if the catalog ever shrinks below it.
pub const DEFAULT_OFFERINGS_PAGE_SIZE: usize = 20;

/// Whether a list-shaped request filter admits `value`: an absent or empty list
/// constrains nothing, a populated one admits only the values it names.
///
/// A value a resource always carries, so the admission rule is
/// [`property_filter_admits`] with the property present.
pub fn filter_admits(filter: Option<&Vec<String>>, value: &str) -> bool {
    property_filter_admits(filter, Some(value))
}

/// Whether a filter on ONE named property admits a resource whose value for
/// that property is `field` — `None` when the resource does not carry the
/// property at all.
///
/// The crate's one home for the property-filter rule, which every search that
/// narrows by a named property follows: a filter with no values constrains
/// nothing, a resource that is not dimensioned by the property a filter names
/// cannot satisfy it, and otherwise the filter admits the values it names.
pub fn property_filter_admits(values: Option<&Vec<String>>, field: Option<&str>) -> bool {
    match (values, field) {
        (Some(values), _) if values.is_empty() => true,
        (None, _) => true,
        (Some(_), None) => false,
        (Some(values), Some(field)) => values.iter().any(|value| value == field),
    }
}

/// The identity of an idempotent request: everything about it that the stored
/// resource depends on.
///
/// A replayed `clientToken` is only idempotent for the SAME request; a token
/// reused for a different one is refused rather than answered with the first
/// call's resource, which would report success for a write that never happened.
pub fn fingerprint(parts: &[(&str, &str)]) -> String {
    // Every component is length-prefixed, so no two different requests can
    // encode to the same string however their values are spelled: a tag value
    // carrying the separator, or a key carrying the `=`, cannot be read as a
    // different part.
    parts
        .iter()
        .map(|(name, value)| format!("{}:{name}:{}:{value}", name.len(), value.len()))
        .collect::<Vec<_>>()
        .join("\u{1f}")
}

// ---------------------------------------------------------------------------
// Tags
// ---------------------------------------------------------------------------

/// The most tags one resource may carry.
///
/// AWS's documented tag limit ("Tag naming limits and requirements", AWS
/// General Reference: a maximum of 50 tags per resource). The model declares no
/// constraint on `TagMap`, so the handler enforces it.
pub const MAX_TAGS_PER_PLAN: usize = 50;

/// The longest tag key AWS accepts (128 Unicode characters, same source).
pub const MAX_TAG_KEY_CHARS: usize = 128;

/// The longest tag value AWS accepts (256 Unicode characters, same source).
pub const MAX_TAG_VALUE_CHARS: usize = 256;

/// Why a tag set was refused, which is what decides WHICH modelled error the
/// refusal is reported as.
///
/// The two are different failures and the model names both: a tag set that
/// does not fit the per-resource tag allowance has exhausted a QUOTA, so an
/// operation that declares `ServiceQuotaExceededException` reports it as that
/// (HTTP 402); a key or value outside its documented length is a malformed
/// input parameter, so it stays `ValidationException` (HTTP 400) everywhere.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TagRejection {
    /// The resulting tag set would be larger than [`MAX_TAGS_PER_PLAN`].
    QuotaExceeded(String),
    /// A tag key or value breaks the length the service accepts.
    Invalid(String),
}

impl TagRejection {
    /// Whether this refusal is a quota failure rather than a constraint one.
    pub fn is_quota_exceeded(&self) -> bool {
        matches!(self, Self::QuotaExceeded(_))
    }

    /// The refusal's message, for whichever modelled error carries it.
    pub fn into_message(self) -> String {
        match self {
            Self::QuotaExceeded(message) | Self::Invalid(message) => message,
        }
    }
}

/// Refuse a tag set that is larger than the service accepts, or that carries an
/// over-long key or value. Checked before anything is stored, so a refused
/// write changes nothing.
///
/// The two refusals are distinguished by the [`TagRejection`] handed back, so a
/// caller branching on the quota error sees it for the tag allowance and the
/// constraint error for a malformed tag.
pub fn check_tags(tags: &BTreeMap<String, String>) -> Result<(), TagRejection> {
    // The SHAPE of each tag is checked first: a malformed input parameter is
    // refused as one however many tags came with it, so a request that is both
    // malformed and over the allowance is reported as the constraint failure it
    // is rather than blamed on the quota.
    for (key, value) in tags {
        if key.chars().count() > MAX_TAG_KEY_CHARS {
            return Err(TagRejection::Invalid(format!(
                "a tag key cannot be longer than {MAX_TAG_KEY_CHARS} characters, got {}",
                key.chars().count()
            )));
        }
        if value.chars().count() > MAX_TAG_VALUE_CHARS {
            return Err(TagRejection::Invalid(format!(
                "the value of tag '{key}' cannot be longer than {MAX_TAG_VALUE_CHARS} characters, got {}",
                value.chars().count()
            )));
        }
    }
    if tags.len() > MAX_TAGS_PER_PLAN {
        return Err(TagRejection::QuotaExceeded(format!(
            "a Savings Plan cannot carry more than {MAX_TAGS_PER_PLAN} tags, got {}",
            tags.len()
        )));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// ARNs
// ---------------------------------------------------------------------------

/// Build the ARN of a Savings Plan.
///
/// Savings Plans are account-wide rather than regional, so the ARN's region
/// segment is empty — the form the model's `SavingsPlanArn` pattern admits
/// through its `([a-z]{2}-[a-z]+-\d{1}|)` alternative.
pub fn savings_plan_arn(savings_plan_id: &str) -> String {
    ::engine::emulator_arn(
        SERVICE_NAMESPACE,
        "",
        &format!("savingsplan/{savings_plan_id}"),
    )
}

/// The partition field every ARN of ours carries, as the model's
/// `SavingsPlanArn` pattern spells it: the literal `arn:aws:` prefix, with no
/// alternative for a suffixed partition.
const ARN_PARTITION: &str = "aws";

/// Whether an ARN's partition field is the one a Savings Plan ARN carries.
///
/// The `SavingsPlanArn` pattern anchors the partition as the literal `aws`, so
/// this mirrors that group rather than guessing a wider family: an ARN in some
/// other partition is not a well-formed value of the shape at all, and the
/// boundary validator refuses it before any handler runs.
fn is_aws_partition(field: &str) -> bool {
    field == ARN_PARTITION
}

/// The Savings Plan id inside a `SavingsPlanArn`, or `None` when the ARN names
/// some other service or some other resource type.
///
/// The model's `@pattern` already refused anything that is not ARN-shaped
/// before the handler ran, so what is left to reject is a well-formed ARN that
/// names another service namespace. Whether a plan with a Savings Plan ARN
/// exists is a question for the store (a 404), not for the shape of the ARN
/// (a 400).
pub fn savings_plan_id_from_arn(arn: &str) -> Option<&str> {
    let mut fields = arn.splitn(6, ':');
    let (
        Some("arn"),
        Some(partition),
        Some(SERVICE_NAMESPACE),
        Some(_region),
        Some(_account),
        Some(resource),
    ) = (
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
    )
    else {
        return None;
    };
    if !is_aws_partition(partition) {
        return None;
    }
    resource.strip_prefix("savingsplan/")
}

/// The Savings Plan id the `resourceArn` of a tagging operation names, or the
/// message with which this service refuses an ARN that is not one of ours.
///
/// The model's `@pattern` already refused anything that is not ARN-shaped
/// before the handler ran; what is left to refuse is a well-formed ARN of some
/// other service or resource type. That refusal is one rule with one message,
/// so — like [`resource_not_found_message`] beside it — it lives in ONE place,
/// shared by every operation that takes a `resourceArn`; each handler only
/// wraps the message in its own `ValidationException` variant.
pub fn savings_plan_id_or_refusal(resource_arn: &str) -> Result<&str, String> {
    savings_plan_id_from_arn(resource_arn).ok_or_else(|| {
        format!("resourceArn '{resource_arn}' is not the ARN of a {SERVICE_NAMESPACE} savingsplan resource")
    })
}

/// How this service states that the resource an operation was pointed at does
/// not exist.
///
/// AWS builds this message FROM THE REQUEST, naming the `resourceArn` the
/// caller sent — `Resource with resourceArn: <arn> doesn't exist` — so the
/// template is part of the contract and lives in ONE place, shared by every
/// operation that takes a `resourceArn`.
pub fn resource_not_found_message(resource_arn: &str) -> String {
    format!("Resource with resourceArn: {resource_arn} doesn't exist")
}

/// How this service states that the Savings Plan an operation was pointed at
/// does not exist.
///
/// AWS builds this message FROM THE REQUEST, naming the `savingsPlanId` the
/// caller sent — `SavingsPlan can't be found. Savings Plan Id: <id>.`, the id
/// taken verbatim and closed with a period — so the template is part of the
/// contract and lives in ONE place, shared by every operation that is pointed
/// at a plan by its `savingsPlanId`; each handler only wraps the message in its
/// own `ResourceNotFoundException` variant. Nothing about the id's spelling
/// changes the sentence: not being in the store is what makes the answer a 404.
pub fn savings_plan_not_found_message(savings_plan_id: &str) -> String {
    format!("SavingsPlan can't be found. Savings Plan Id: {savings_plan_id}.")
}

/// How this service states that the OFFERING a purchase named does not exist.
///
/// AWS words this refusal as a fixed sentence plus the remediation hint that
/// follows it, and builds nothing into it from the request: the submitted
/// offering id is not echoed, and the two sentences are separated by two
/// spaces, which is part of the text. One constant, so the operation that buys
/// an offering cannot word it differently from any other that resolves one.
pub const OFFERING_NOT_FOUND_MESSAGE: &str =
    "Offering ID not found.  Insert a valid offering ID and try again.";

/// How this service refuses an operation that only a QUEUED purchase accepts.
///
/// A fixed sentence as well: it states the state the plan had to be in, and
/// never the state it is in, so every state the call is refused from — one
/// already cancelled, one being paid for, one active, retired or returned — is
/// refused with the one sentence.
pub const NOT_QUEUED_MESSAGE: &str = "Savings Plan is not in Queued State.";

// ---------------------------------------------------------------------------
// The offering catalog
// ---------------------------------------------------------------------------

/// A Savings Plan offering: one purchasable combination of plan type, payment
/// option, term and (for the plan types that are scoped to one) region and
/// instance family. Projected onto the wire as `SavingsPlanOffering`.
#[derive(Debug, Clone)]
pub struct Offering {
    pub offering_id: String,
    pub product_types: Vec<String>,
    pub plan_type: String,
    pub description: String,
    pub payment_option: String,
    pub duration_seconds: i64,
    pub currency: String,
    pub service_code: String,
    pub usage_type: String,
    pub operation: String,
    /// `(name, value)` pairs, where `name` is a
    /// `SavingsPlanOfferingPropertyKey`.
    pub properties: Vec<(String, String)>,
}

impl Offering {
    /// The value of the named property, or `None` when this offering does not
    /// carry it (a Compute Savings Plan is scoped to no region).
    pub fn property(&self, name: &str) -> Option<&str> {
        self.properties
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }

    /// Whether a `SavingsPlanOfferingFilterElement` admits this offering.
    /// `name` is a `SavingsPlanOfferingFilterAttribute` — one of the properties
    /// an offering is scoped by — and the rule is the crate's one
    /// property-filter rule ([`property_filter_admits`]): an offering that does
    /// not carry the property a filter names cannot satisfy it.
    pub fn matches_filter(&self, name: &str, values: Option<&Vec<String>>) -> bool {
        property_filter_admits(values, self.property(name))
    }

    /// The term, in hours — what an hourly commitment is multiplied by to get
    /// the total value of the plan. Both terms the catalog sells are whole
    /// numbers of hours.
    pub fn term_hours(&self) -> i128 {
        i128::from(self.duration_seconds / 3600)
    }
}

/// The terms AWS sells Savings Plans in: a one-year and a three-year term, each
/// as `(durationSeconds, description label, usage-type token)`.
///
/// `durationSeconds` is the term the wire reports
/// (`SavingsPlanOffering$durationSeconds`); the description label is how the
/// term is spelled in `description` and the usage-type token how it is spelled
/// in the billing `usageType` (see [`offering_description`] and
/// [`offering_usage_type`]).
const TERMS: [(i64, &str, &str); 2] = [
    (31_536_000, "1 year", "1yr"),
    (94_608_000, "3 year", "3yr"),
];

/// Every `SavingsPlanPaymentOption` the model declares, in the order the
/// catalog enumerates them.
///
/// The catalog is walked in a stable total order over the attributes that
/// distinguish one offering from another — ascending term, then payment option
/// in LEXICOGRAPHIC order, then (where the plan type is scoped to them) region
/// and instance family — so page N of a search is the same rows on every call
/// and two searches agree on which offering comes first. Lexicographic order is
/// not the model's declaration order (`All Upfront`, `Partial Upfront`,
/// `No Upfront`), which is why this list is not in the enum's order.
const PAYMENT_OPTIONS: [&str; 3] = [
    PAYMENT_ALL_UPFRONT,
    PAYMENT_NO_UPFRONT,
    PAYMENT_PARTIAL_UPFRONT,
];

/// Whether `code` is a value of `SavingsPlanServiceCode` — the shape BOTH
/// `SavingsPlanOffering$serviceCode` and the offerings search's own
/// `serviceCodes` filter target: letters only, at least one and at most 255 of
/// them (pattern `^[a-zA-Z]+$`, length 0..255).
///
/// The two directions are one shape, so a code the catalog reports is a code the
/// same operation's filter accepts; a code outside it could never be fed back.
fn is_service_code(code: &str) -> bool {
    !code.is_empty() && code.len() <= 255 && code.bytes().all(|b| b.is_ascii_alphabetic())
}

/// The billing line item a **compute** plan type's offerings bill under, which
/// is what `SavingsPlanOffering$serviceCode` reports for them.
///
/// BOTH compute plan types report it, and that is a deliberate divergence worth
/// stating: the live service answers an EC2 Instance offering with the EC2
/// service's own per-service pricing code, whose spelling carries a digit — a
/// value its OWN model excludes, since `SavingsPlanServiceCode` (the shape this
/// member and the search's `serviceCodes` filter share) admits letters only
/// ([`is_service_code`]). An emulator that validates requests from the model
/// cannot report both halves of that: a digit-carrying code is refused as a
/// filter value before any offering is compared, so reporting it would publish a
/// code its own search rejects. This crate follows the model and reports the
/// letters-only line item the plan bills under — the compute Savings Plans line
/// item AWS publishes Compute and EC2 Instance Savings Plans in one price list
/// for. A plan type that spans several services has no single member service's
/// code to report anyway ([`SERVICE_CODE_DATABASE`]).
///
/// Where a digit-carrying per-service code IS a modelled value, the RATE catalog
/// reports it unchanged: `SavingsPlanRateServiceCode` is an enum that lists those
/// spellings ([`crate::ops::types::SAVINGS_PLAN_RATE_SERVICE_CODE_VALUES`]).
const SERVICE_CODE_COMPUTE: &str = "ComputeSavingsPlans";

/// The line item a SageMaker plan type's offerings bill under: the one service
/// such a plan is scoped to. See [`SERVICE_CODE_COMPUTE`].
const SERVICE_CODE_SAGEMAKER: &str = "AmazonSageMaker";

/// The line item a Database plan type's offerings bill under. A plan type that
/// spans several services bills under its OWN Savings Plans line item rather
/// than under one member service's. See [`SERVICE_CODE_COMPUTE`].
const SERVICE_CODE_DATABASE: &str = "DatabaseSavingsPlans";

/// How an offering's `description` names it: the term, the payment option and
/// the plan type, in that order (`SavingsPlanDescription`, pattern
/// `^[a-zA-Z0-9_\- ]+$`, so no punctuation beyond `-` and `_`).
fn offering_description(term_label: &str, payment_option: &str, plan_label: &str) -> String {
    format!("{term_label} {payment_option} {plan_label} Savings Plan")
}

/// The billing `usageType` of an offering's line item: the usage-type family of
/// the plan type (prefixed by the region it is scoped to, where it is scoped to
/// one) followed by the term and payment option that line item is billed
/// under — so every offering reports the usage type a billing report would show
/// for it, and no two offerings of one plan type share a usage type.
///
/// `family` carries the region prefix and any further dimension the plan type is
/// scoped to (an EC2 instance family). `SavingsPlanUsageType` admits `:`, `.`,
/// `/` and `-` (pattern `^[a-zA-Z0-9_ \/.:-]+$`), so the `:`-separated form is
/// inside the model's constraint.
fn offering_usage_type(family: &str, term_token: &str, payment_option: &str) -> String {
    let payment_token: String = payment_option
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    format!("{family}:{term_token}{payment_token}")
}

/// The regions the catalog is seeded for, each with the `CurrencyCode` the
/// region bills in.
///
/// Savings Plans are sold in every commercial region, so the catalog carries a
/// region-scoped offering for each one rather than for a handful: a client that
/// searches for offerings in the region it runs in finds them, and the catalog
/// is the size a real one is — bigger than any one page a caller may ask for
/// (`PageSize` tops out at 1000), so a search of it is genuinely paginated.
///
/// The currency is the one the model's `CurrencyCode` enum offers for that
/// region's billing: `CNY` in the China partition, `EUR` in Europe, `USD`
/// elsewhere.
const REGIONS: [(&str, &str); 22] = [
    ("us-east-1", "USD"),
    ("us-east-2", "USD"),
    ("us-west-1", "USD"),
    ("us-west-2", "USD"),
    ("ca-central-1", "USD"),
    ("sa-east-1", "USD"),
    ("eu-west-1", "EUR"),
    ("eu-west-2", "EUR"),
    ("eu-west-3", "EUR"),
    ("eu-central-1", "EUR"),
    ("eu-north-1", "EUR"),
    ("eu-south-1", "EUR"),
    ("ap-south-1", "USD"),
    ("ap-east-1", "USD"),
    ("ap-northeast-1", "USD"),
    ("ap-northeast-2", "USD"),
    ("ap-southeast-1", "USD"),
    ("ap-southeast-2", "USD"),
    ("me-south-1", "USD"),
    ("af-south-1", "USD"),
    ("cn-north-1", "CNY"),
    ("cn-northwest-1", "CNY"),
];

/// The EC2 instance families the catalog carries EC2 Instance offerings for
/// (`SavingsPlanOffering$properties` -> `instanceFamily`): the general purpose,
/// compute, memory and storage optimised families of several generations, so a
/// search by instance family finds the family a client is actually running.
///
/// Each family carries the on-demand price of ONE vCPU-hour in it, in
/// [`AMOUNT_SCALE`] units — AWS's published on-demand Linux price for a family,
/// divided by the vCPUs it is for (`m5.large` at $0.096 an hour is two vCPUs at
/// $0.048) — which is what [`RateLineItem::rate`] discounts into a Savings
/// Plan rate. One list, so a family the offering catalog sells is a family the
/// rate catalog prices.
pub const EC2_INSTANCE_FAMILIES: [(&str, i128); 18] = [
    ("m5", 4_800_000),
    ("m6a", 4_320_000),
    ("m6i", 4_800_000),
    ("m7g", 4_080_000),
    ("c5", 4_250_000),
    ("c6a", 3_825_000),
    ("c6i", 4_250_000),
    ("c7g", 3_625_000),
    ("r5", 6_300_000),
    ("r6a", 5_670_000),
    ("r6i", 6_300_000),
    ("r7g", 5_355_000),
    ("t3", 4_160_000),
    ("t3a", 3_760_000),
    ("t4g", 3_360_000),
    ("i3", 7_800_000),
    ("i4i", 8_600_000),
    ("x2idn", 10_420_000),
];

/// The product types a **Compute** Savings Plan covers
/// (`SavingsPlanOffering$productTypes`, a `SavingsPlanProductType` list).
///
/// AWS documents the coverage of each plan type, and that is what fixes these
/// lists: a Compute Savings Plan "applies to usage across Amazon EC2, AWS
/// Lambda, and AWS Fargate" in any region. The order a coverage list is reported
/// in carries no meaning on the wire; this one mirrors the recorded AWS catalog
/// row so a client diffing the two sees the same row.
const COMPUTE_PRODUCT_TYPES: [&str; 3] = ["Fargate", "EC2", "Lambda"];

/// The product types an **EC2 Instance** Savings Plan covers: EC2 usage of one
/// instance family in one region, so EC2 alone. See [`COMPUTE_PRODUCT_TYPES`].
const EC2_INSTANCE_PRODUCT_TYPES: [&str; 1] = ["EC2"];

/// The product types a **SageMaker** Savings Plan covers: SageMaker usage alone.
/// See [`COMPUTE_PRODUCT_TYPES`].
const SAGEMAKER_PRODUCT_TYPES: [&str; 1] = ["SageMaker"];

/// The product types a **Database** Savings Plan covers: every
/// `SavingsPlanProductType` the model enumerates that no other plan type sells.
///
/// The model declares exactly four `SavingsPlanType` values (Compute,
/// EC2Instance, SageMaker, Database) and fourteen `SavingsPlanProductType`
/// values. Compute, EC2 Instance and SageMaker plans cover the compute and ML
/// products AWS documents for them ([`COMPUTE_PRODUCT_TYPES`] and friends), so
/// the remaining products — the database and database-adjacent ones — are sold
/// by the one plan type left, and a search narrowed to any of them must find the
/// offerings that cover it.
///
/// Derived from the generated value set of the model's own enum
/// ([`crate::ops::types::SAVINGS_PLAN_PRODUCT_TYPE_VALUES`]) rather than written
/// out by hand, so a product type the model gains is covered by the catalog
/// without anyone remembering to add it here.
fn database_product_types() -> Vec<&'static str> {
    crate::ops::types::SAVINGS_PLAN_PRODUCT_TYPE_VALUES
        .iter()
        .copied()
        .filter(|product| {
            !COMPUTE_PRODUCT_TYPES.contains(product)
                && !EC2_INSTANCE_PRODUCT_TYPES.contains(product)
                && !SAGEMAKER_PRODUCT_TYPES.contains(product)
        })
        .collect()
}

/// Build the catalog: one offering per plan type x term x payment option, times
/// the region (and, for EC2 Instance plans, the instance family) each plan type
/// is scoped to.
///
/// The catalog is enumerated BY PLAN TYPE — the order the model's
/// `SavingsPlanType` enum declares (Compute, EC2Instance, SageMaker, Database) —
/// then by ascending term, then by payment option, then by the region and
/// instance family the plan type is scoped to. That is the order the recorded
/// catalog answers in: a search narrowed to Compute plans answers with that plan
/// type's six term/payment-option rows as the first rows of the catalog, in
/// payment-option order. Plan types that are not scoped to a region (Compute)
/// carry one row per term and payment option; the scoped ones carry one per
/// region, and EC2 Instance plans one per instance family within the region.
///
/// The product types of each plan type are the ones AWS covers with it
/// ([`COMPUTE_PRODUCT_TYPES`], [`EC2_INSTANCE_PRODUCT_TYPES`],
/// [`SAGEMAKER_PRODUCT_TYPES`] and [`database_product_types`]), and between them
/// those four lists cover EVERY `SavingsPlanProductType` the model enumerates:
/// every product type the service sells Savings Plans for is carried by at least
/// one purchasable offering, so a search narrowed to any of them answers with
/// the offerings that cover it and a plan bought from one reports it.
fn seed_offerings(id_seed: u128) -> Vec<Offering> {
    let mut offerings = Vec::new();
    for (duration_seconds, term_label, term_token) in TERMS {
        for payment_option in PAYMENT_OPTIONS {
            offerings.push(offering(
                "Compute",
                &COMPUTE_PRODUCT_TYPES,
                &offering_description(term_label, payment_option, "Compute"),
                payment_option,
                duration_seconds,
                "USD",
                SERVICE_CODE_COMPUTE,
                &offering_usage_type("ComputeSP", term_token, payment_option),
                "",
                Vec::new(),
            ));
        }
    }
    for (duration_seconds, term_label, term_token) in TERMS {
        for payment_option in PAYMENT_OPTIONS {
            for (region, currency) in REGIONS {
                for (family, _) in EC2_INSTANCE_FAMILIES {
                    offerings.push(offering(
                        "EC2Instance",
                        &EC2_INSTANCE_PRODUCT_TYPES,
                        &offering_description(term_label, payment_option, "EC2 Instance"),
                        payment_option,
                        duration_seconds,
                        currency,
                        SERVICE_CODE_COMPUTE,
                        &offering_usage_type(
                            &format!("{region}-EC2SP:{family}"),
                            term_token,
                            payment_option,
                        ),
                        "RunInstances",
                        vec![
                            (PROPERTY_INSTANCE_FAMILY.to_string(), family.to_string()),
                            (PROPERTY_REGION.to_string(), region.to_string()),
                        ],
                    ));
                }
            }
        }
    }
    for (duration_seconds, term_label, term_token) in TERMS {
        for payment_option in PAYMENT_OPTIONS {
            for (region, currency) in REGIONS {
                offerings.push(offering(
                    "SageMaker",
                    &SAGEMAKER_PRODUCT_TYPES,
                    &offering_description(term_label, payment_option, "SageMaker"),
                    payment_option,
                    duration_seconds,
                    currency,
                    SERVICE_CODE_SAGEMAKER,
                    &offering_usage_type(
                        &format!("{region}-SageMakerSP"),
                        term_token,
                        payment_option,
                    ),
                    "RunInstance",
                    vec![(PROPERTY_REGION.to_string(), region.to_string())],
                ));
            }
        }
    }
    for (duration_seconds, term_label, term_token) in TERMS {
        for payment_option in PAYMENT_OPTIONS {
            for (region, currency) in REGIONS {
                offerings.push(offering(
                    "Database",
                    &database_product_types(),
                    &offering_description(term_label, payment_option, "Database"),
                    payment_option,
                    duration_seconds,
                    currency,
                    // A Database plan covers every database product the model
                    // enumerates, so it reports the plan's own line item here
                    // and no per-service `operation`.
                    SERVICE_CODE_DATABASE,
                    &offering_usage_type(
                        &format!("{region}-DatabaseSP"),
                        term_token,
                        payment_option,
                    ),
                    "",
                    vec![(PROPERTY_REGION.to_string(), region.to_string())],
                ));
            }
        }
    }
    for (index, entry) in offerings.iter_mut().enumerate() {
        entry.offering_id = mint_id(id_seed, index as u64);
    }
    offerings
}

/// Mint an id in the v4 UUID form: `counter` makes it unique within the store
/// and `seed` makes it unguessable from one store to the next, and the two are
/// run through SHA-256 so no request value can be read back out of an id.
///
/// The form is what matters on the wire: the model's `UUID` pattern (and the
/// `SavingsPlanArn` pattern's `[1-5]`/`[89ab]` nibbles) require hyphenated
/// lowercase hex with the RFC 4122 version and variant bits set, which is what
/// this writes.
fn mint_id(seed: u128, counter: u64) -> String {
    let digest = Sha256::digest(format!("{seed}/{counter}").as_bytes());
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex = hex::encode(bytes);
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// A per-store seed for [`mint_id`].
fn id_seed() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_nanos())
        .unwrap_or_default()
}

/// One catalog entry. Its `offering_id` is assigned by [`seed_offerings`] once
/// the catalog is built, so an offering keeps one id for the life of the store,
/// the way a real offering id outlives any one request.
#[allow(clippy::too_many_arguments)]
fn offering(
    plan_type: &str,
    product_types: &[&str],
    description: &str,
    payment_option: &str,
    duration_seconds: i64,
    currency: &str,
    service_code: &str,
    usage_type: &str,
    operation: &str,
    properties: Vec<(String, String)>,
) -> Offering {
    debug_assert!(
        is_service_code(service_code),
        "the catalog may only report a service code the offerings search's own serviceCodes filter accepts, got {service_code:?}"
    );
    Offering {
        offering_id: String::new(),
        product_types: product_types.iter().map(|p| (*p).to_string()).collect(),
        plan_type: plan_type.to_string(),
        description: description.to_string(),
        payment_option: payment_option.to_string(),
        duration_seconds,
        currency: currency.to_string(),
        service_code: service_code.to_string(),
        usage_type: usage_type.to_string(),
        operation: operation.to_string(),
        properties,
    }
}

// ---------------------------------------------------------------------------
// The offering-rate catalog
// ---------------------------------------------------------------------------

/// `SavingsPlanRatePropertyKey` members a RATE is dimensioned by, beyond the two
/// an offering itself carries ([`PROPERTY_INSTANCE_FAMILY`] and
/// [`PROPERTY_REGION`]). The full value set is
/// [`crate::ops::types::SAVINGS_PLAN_RATE_PROPERTY_KEY_VALUES`].
pub const RATE_PROPERTY_INSTANCE_TYPE: &str = "instanceType";
/// See [`RATE_PROPERTY_INSTANCE_TYPE`].
pub const RATE_PROPERTY_PRODUCT_DESCRIPTION: &str = "productDescription";
/// See [`RATE_PROPERTY_INSTANCE_TYPE`].
pub const RATE_PROPERTY_TENANCY: &str = "tenancy";

/// The EC2 product description of an unlicensed Linux line item: the first row
/// of [`EC2_PLATFORMS`], and the description every EC2 usage carries no
/// operating-system or database licence for.
pub const PRODUCT_DESCRIPTION_LINUX: &str = "Linux/UNIX";
/// See [`PRODUCT_DESCRIPTION_LINUX`].
pub const TENANCY_SHARED: &str = "shared";

/// What a paid operating-system licence adds to the price of one vCPU-hour, in
/// [`AMOUNT_SCALE`] units: AWS's published licence uplifts, which is why an
/// `m5.large` (two vCPUs) at $0.096 an hour on unlicensed Linux is $0.188 an
/// hour on Windows.
///
/// A platform's licence is the SUM of its operating-system uplift and its
/// database-edition uplift ([`LICENCE_SQL_WEB`] and the two below it), so the
/// licence column of [`EC2_PLATFORMS`] is composed from these rather than
/// written out per platform: a platform is an operating system plus a database
/// edition, and that is how EC2 prices one.
const LICENCE_WINDOWS: i128 = 4_600_000;
/// See [`LICENCE_WINDOWS`].
const LICENCE_RHEL: i128 = 1_500_000;
/// See [`LICENCE_WINDOWS`]. Red Hat's high-availability add-on doubles the
/// Red Hat Enterprise Linux uplift.
const LICENCE_RHEL_HA: i128 = 2 * LICENCE_RHEL;
/// See [`LICENCE_WINDOWS`].
const LICENCE_SUSE: i128 = 1_000_000;
/// See [`LICENCE_WINDOWS`].
const LICENCE_UBUNTU_PRO: i128 = 500_000;
/// What a SQL Server Web licence adds to the price of one vCPU-hour, in
/// [`AMOUNT_SCALE`] units: AWS's published per-vCPU database licence prices, so
/// an `m5.large` on unlicensed Linux at $0.096 an hour is $0.193 an hour with
/// SQL Server Web and $0.928 an hour on Windows with SQL Server Standard. See
/// [`LICENCE_WINDOWS`].
const LICENCE_SQL_WEB: i128 = 4_850_000;
/// See [`LICENCE_SQL_WEB`].
const LICENCE_SQL_STANDARD: i128 = 37_000_000;
/// See [`LICENCE_SQL_WEB`].
const LICENCE_SQL_ENTERPRISE: i128 = 140_000_000;

/// Every EC2 platform a Savings Plan rate card prices, as `(productDescription,
/// operation, licence units per vCPU-hour)`.
///
/// EC2 bills one line item per PLATFORM — the operating system and database
/// licence the instance runs — and reports which one through the usage-operation
/// code of that line item, so the rate card of a commitment that covers EC2
/// carries one rate per platform per instance type rather than a single
/// unlicensed row. The `productDescription`/`operation` pairs are AWS's own
/// published platform-details-to-usage-operation table (EC2 User Guide,
/// "Platform details and usage operation"; AWS Billing's usage-operation
/// values), so `RunInstances` is unlicensed Linux, `RunInstances:0006` is
/// Windows with SQL Server Standard, `RunInstances:0200` is Linux with SQL
/// Server Web, and so on.
///
/// The licence column is what the platform adds to the price of one vCPU-hour of
/// the instance family ([`EC2_INSTANCE_FAMILIES`]), composed from the published
/// per-vCPU uplifts above. This list is the platform CATALOG, in its reference
/// order — the unlicensed row first, then the platforms by operating system —
/// and a rate card lists an instance type's platforms in the catalog's own
/// pricing order instead ([`ec2_platforms_in_card_order`]).
pub const EC2_PLATFORMS: [(&str, &str, i128); 19] = [
    (PRODUCT_DESCRIPTION_LINUX, "RunInstances", 0),
    ("Red Hat BYOL Linux", "RunInstances:00g0", 0),
    ("Red Hat Enterprise Linux", "RunInstances:0010", LICENCE_RHEL),
    (
        "Red Hat Enterprise Linux with HA",
        "RunInstances:1010",
        LICENCE_RHEL_HA,
    ),
    (
        "Red Hat Enterprise Linux with HA and SQL Server Standard",
        "RunInstances:1014",
        LICENCE_RHEL_HA + LICENCE_SQL_STANDARD,
    ),
    (
        "Red Hat Enterprise Linux with HA and SQL Server Enterprise",
        "RunInstances:1110",
        LICENCE_RHEL_HA + LICENCE_SQL_ENTERPRISE,
    ),
    (
        "Red Hat Enterprise Linux with SQL Server Web",
        "RunInstances:0210",
        LICENCE_RHEL + LICENCE_SQL_WEB,
    ),
    (
        "Red Hat Enterprise Linux with SQL Server Standard",
        "RunInstances:0014",
        LICENCE_RHEL + LICENCE_SQL_STANDARD,
    ),
    (
        "Red Hat Enterprise Linux with SQL Server Enterprise",
        "RunInstances:0110",
        LICENCE_RHEL + LICENCE_SQL_ENTERPRISE,
    ),
    ("SUSE Linux", "RunInstances:000g", LICENCE_SUSE),
    ("Ubuntu Pro", "RunInstances:0g00", LICENCE_UBUNTU_PRO),
    ("Windows", "RunInstances:0002", LICENCE_WINDOWS),
    ("Windows BYOL", "RunInstances:0800", 0),
    (
        "Windows with SQL Server Web",
        "RunInstances:0202",
        LICENCE_WINDOWS + LICENCE_SQL_WEB,
    ),
    (
        "Windows with SQL Server Standard",
        "RunInstances:0006",
        LICENCE_WINDOWS + LICENCE_SQL_STANDARD,
    ),
    (
        "Windows with SQL Server Enterprise",
        "RunInstances:0102",
        LICENCE_WINDOWS + LICENCE_SQL_ENTERPRISE,
    ),
    (
        "Linux with SQL Server Web",
        "RunInstances:0200",
        LICENCE_SQL_WEB,
    ),
    (
        "Linux with SQL Server Standard",
        "RunInstances:0004",
        LICENCE_SQL_STANDARD,
    ),
    (
        "Linux with SQL Server Enterprise",
        "RunInstances:0100",
        LICENCE_SQL_ENTERPRISE,
    ),
];

/// Seconds in a 365-day year: the unit a Savings Plans term is sold in, so the
/// terms the catalog sells ([`TERMS`]) are one and three of these.
const SECONDS_PER_YEAR: i64 = 31_536_000;

/// Basis points in a whole, so a discount is applied in integer arithmetic and
/// no price is ever computed in floating point.
const BASIS_POINTS: i128 = 10_000;

/// The largest discount any Savings Plan earns: AWS publishes Savings Plans
/// savings of "up to 72 percent" over on-demand, so no row of this catalog is
/// priced below 28 percent of its on-demand price however the ladder below
/// compounds.
const MAX_DISCOUNT_BASIS_POINTS: i128 = 7_200;

/// The discount, in basis points, a ONE-year No Upfront plan of each
/// `SavingsPlanType` earns.
///
/// The Compute row is ANCHORED on AWS's published Savings Plans price list, read
/// off the relation between an instance's on-demand price and its Savings Plans
/// rate: an `m5.large` in `us-east-1` at $0.096 an hour on-demand is $0.072 an
/// hour under a one-year No Upfront Compute plan (25 percent off) and $0.066
/// under the All Upfront one, which is what
/// [`payment_option_discount_percent`] deepens it to. The other three are the
/// catalog's own magnitudes, as they were before: what AWS publishes for them is
/// the ORDERING and the ceiling ([`MAX_DISCOUNT_BASIS_POINTS`]), and the ordering
/// is the documented one — an EC2 Instance Savings Plan, which commits to one
/// instance family in one region, earns the deepest discount; a Compute Savings
/// Plan, which commits to nothing but an hourly spend, earns less; the plan types
/// scoped to one service sit between a Compute plan and nothing.
fn plan_type_discount_basis_points(plan_type: &str) -> i128 {
    match plan_type {
        "EC2Instance" => 2_800,
        "Compute" => 2_500,
        "SageMaker" => 2_200,
        // Every other `SavingsPlanType` the model declares (Database today)
        // commits to one family of services, which earns the shallowest of the
        // published discounts.
        _ => 1_800,
    }
}

/// How much of the base discount a payment option earns, in percent: paying
/// more of the commitment up front earns a deeper discount, which is why AWS
/// prices All Upfront below Partial Upfront below No Upfront.
///
/// Read off the published price list the same way the bases are
/// ([`plan_type_discount_basis_points`]): a one-year All Upfront Compute plan
/// earns a quarter more discount than the No Upfront one of the same term, so an
/// `m5.large` at $0.072 an hour under No Upfront is $0.066 under All Upfront.
fn payment_option_discount_percent(payment_option: &str) -> i128 {
    match payment_option {
        PAYMENT_ALL_UPFRONT => 125,
        PAYMENT_PARTIAL_UPFRONT => 120,
        // No Upfront pays nothing in advance and earns the base discount.
        _ => 100,
    }
}

/// The discount a commitment of `plan_type`, sold for `duration_seconds` under
/// `payment_option`, earns in basis points: the plan type's base discount,
/// deepened by 30 percent of itself for each year of term past the first and by
/// the payment option's share, and capped at [`MAX_DISCOUNT_BASIS_POINTS`].
///
/// Strictly increasing in the term and in how much is paid up front, so two
/// commitments of one plan type never price the same usage at the same rate
/// unless they carry the same term and payment option.
///
/// Asked of the same three attributes whether the commitment is still on sale as
/// an offering or has already been bought as a plan, so a plan is priced by the
/// ladder the offering it was bought from is priced by.
pub fn discount_basis_points(plan_type: &str, duration_seconds: i64, payment_option: &str) -> i128 {
    let years = i128::from(duration_seconds / SECONDS_PER_YEAR).max(1);
    let term_percent = 100 + 30 * (years - 1);
    let base = plan_type_discount_basis_points(plan_type);
    let laddered =
        base * term_percent / 100 * payment_option_discount_percent(payment_option) / 100;
    laddered.min(MAX_DISCOUNT_BASIS_POINTS)
}

/// The discount `offering` earns, in basis points: see [`discount_basis_points`].
pub fn offering_discount_basis_points(offering: &Offering) -> i128 {
    discount_basis_points(
        &offering.plan_type,
        offering.duration_seconds,
        &offering.payment_option,
    )
}

/// What a commitment earning `discount_basis_points` charges for usage whose
/// on-demand price is `on_demand_units`, before the catalog quotes it: the
/// on-demand price less that discount.
///
/// Never zero — a discounted price is still a price — so a unit billed at a
/// fraction of the service's scale is reported at the smallest amount the scale
/// can spell rather than as free. What a rate card REPORTS is this price quoted
/// at the catalog's own precision ([`RateLineItem::rate`]).
pub fn discounted_units(discount_basis_points: i128, on_demand_units: i128) -> i128 {
    let retained = BASIS_POINTS - discount_basis_points;
    divide_rounded(on_demand_units.saturating_mul(retained), BASIS_POINTS).max(1)
}

/// The catalog's rate for `item` under `offering`, as the catalog spells it. See
/// [`RateLineItem::rate`].
pub fn offering_rate(offering: &Offering, item: &RateLineItem) -> String {
    item.rate(offering_discount_basis_points(offering))
}

/// The instance sizes the catalog prices in every EC2 family, with the vCPUs of
/// that size.
///
/// EC2 doubles the vCPUs — and the on-demand price — at each step up the size
/// ladder, so the hourly price of a size is its vCPU count times the family's
/// per-vCPU price (see [`EC2_INSTANCE_FAMILIES`]).
pub const EC2_SIZES: [(&str, i128); 3] = [("large", 2), ("xlarge", 4), ("2xlarge", 8)];

/// The SageMaker ML instance types the catalog prices, with the on-demand hourly
/// price of one instance in [`AMOUNT_SCALE`] units (AWS's published SageMaker
/// real-time inference prices: $0.23, $0.204 and $0.7364 an hour).
const SAGEMAKER_INSTANCE_TYPES: [(&str, i128); 3] = [
    ("ml.m5.xlarge", 23_000_000),
    ("ml.c5.xlarge", 20_400_000),
    ("ml.g4dn.xlarge", 73_640_000),
];

/// A product a Savings Plan covers, and the billing line item the catalog prices
/// it by.
///
/// `service_codes` are the `SavingsPlanRateServiceCode`s that bill the product,
/// `unit` the `SavingsPlanRateUnit` it is billed in, `usage_token` the
/// usage-type of its line item, `operation` the billing operation, and
/// `on_demand_units` the on-demand price of ONE `unit` in [`AMOUNT_SCALE`]
/// units.
struct PricedProduct {
    product_type: &'static str,
    service_codes: &'static [&'static str],
    unit: &'static str,
    usage_token: &'static str,
    operation: &'static str,
    on_demand_units: i128,
}

/// The `SavingsPlanProductType`s the catalog prices by a single line item each,
/// one row per service code that bills them.
///
/// Every product type is mapped onto the `SavingsPlanRateServiceCode` of the
/// service that bills it (Keyspaces bills under `AmazonMCS`, OpenSearch under
/// `AmazonES`, and Fargate under both `AmazonECS` and `AmazonEKS`, the two
/// services that run Fargate tasks) and onto the `SavingsPlanRateUnit` that
/// service bills it in — both model enums, so a product type the model gains is
/// a product type this table is missing and a `cargo test` row below says so.
///
/// The two product types priced along an instance-type dimension (EC2 and
/// SageMaker) are not here: they are priced per instance type from
/// [`EC2_INSTANCE_FAMILIES`] and [`SAGEMAKER_INSTANCE_TYPES`].
const PRICED_PRODUCTS: [PricedProduct; 12] = [
    PricedProduct {
        product_type: "Fargate",
        service_codes: &["AmazonECS", "AmazonEKS"],
        unit: "Hrs",
        usage_token: "Fargate-vCPU-Hours:perCPU",
        operation: "FargateTask",
        // $0.04048 a vCPU-hour.
        on_demand_units: 4_048_000,
    },
    PricedProduct {
        product_type: "Lambda",
        service_codes: &["AWSLambda"],
        unit: "Lambda-GB-Second",
        usage_token: "Lambda-GB-Second",
        operation: "",
        // $0.00001667 a GB-second, at this service's scale.
        on_demand_units: 1_667,
    },
    PricedProduct {
        product_type: "RDS",
        service_codes: &["AmazonRDS"],
        unit: "Hrs",
        usage_token: "InstanceUsage:db.m5.large",
        operation: "CreateDBInstance",
        // $0.178 an instance-hour.
        on_demand_units: 17_800_000,
    },
    PricedProduct {
        product_type: "DSQL",
        service_codes: &["AuroraDSQL"],
        unit: "DPU",
        usage_token: "DPU",
        operation: "",
        // $0.000008 a distributed processing unit.
        on_demand_units: 800,
    },
    PricedProduct {
        product_type: "DynamoDB",
        service_codes: &["AmazonDynamoDB"],
        unit: "ReadCapacityUnit-Hrs",
        usage_token: "ReadCapacityUnit-Hrs",
        operation: "",
        // $0.00013 a read-capacity-unit-hour.
        on_demand_units: 13_000,
    },
    PricedProduct {
        product_type: "ElastiCache",
        service_codes: &["AmazonElastiCache"],
        unit: "Hrs",
        usage_token: "NodeUsage:cache.m5.large",
        operation: "CreateCacheCluster",
        // $0.156 a node-hour.
        on_demand_units: 15_600_000,
    },
    PricedProduct {
        product_type: "DocDB",
        service_codes: &["AmazonDocDB"],
        unit: "Hrs",
        usage_token: "InstanceUsage:db.r5.large",
        operation: "CreateDBInstance",
        // $0.277 an instance-hour.
        on_demand_units: 27_700_000,
    },
    PricedProduct {
        product_type: "Neptune",
        service_codes: &["AmazonNeptune"],
        unit: "Hrs",
        usage_token: "InstanceUsage:db.r5.large",
        operation: "CreateDBInstance",
        // $0.348 an instance-hour.
        on_demand_units: 34_800_000,
    },
    PricedProduct {
        product_type: "Timestream",
        service_codes: &["AmazonTimestream"],
        unit: "Hrs",
        usage_token: "ComputeUsage:TCU",
        operation: "",
        // $0.052 a compute-unit-hour.
        on_demand_units: 5_200_000,
    },
    PricedProduct {
        product_type: "Keyspaces",
        service_codes: &["AmazonMCS"],
        unit: "ReadRequestUnits",
        usage_token: "ReadRequestUnits",
        operation: "",
        // $0.283 a million read request units, at this service's scale.
        on_demand_units: 28,
    },
    PricedProduct {
        product_type: "DMS",
        service_codes: &["AWSDatabaseMigrationSvc"],
        unit: "Hrs",
        usage_token: "InstanceUsage:dms.c5.large",
        operation: "CreateReplicationInstance",
        // $0.154 an instance-hour.
        on_demand_units: 15_400_000,
    },
    PricedProduct {
        product_type: "OpenSearch",
        service_codes: &["AmazonES"],
        unit: "OCU-hours",
        usage_token: "OCU-Hours",
        operation: "",
        // $0.24 an OpenSearch compute-unit-hour.
        on_demand_units: 24_000_000,
    },
];

/// One usage line item the catalog prices, WITHOUT the offering that prices it:
/// the billed product, the service that bills it, its usage type, operation and
/// unit, its on-demand price, and the dimensions it is priced along.
///
/// A line item is priced by every offering that sells it
/// ([`offering_prices`]), and each pairing of the two is one
/// `SavingsPlanOfferingRate` on the wire.
#[derive(Debug, Clone)]
pub struct RateLineItem {
    pub product_type: String,
    pub service_code: String,
    pub usage_type: String,
    pub operation: String,
    /// The `SavingsPlanRateUnit` the line item is billed in.
    pub unit: String,
    /// The undiscounted price of one `unit`, in [`AMOUNT_SCALE`] units.
    pub on_demand_units: i128,
    /// `(name, value)` pairs, where `name` is a `SavingsPlanRatePropertyKey`.
    ///
    /// `productId` is a `SavingsPlanRateFilterAttribute` the model's
    /// property-key enum does NOT carry, so no line item is dimensioned by it
    /// and nothing in this catalog claims a price-list SKU it does not publish.
    pub properties: Vec<(String, String)>,
}

impl RateLineItem {
    /// The value of the named property, or `None` when this line item is not
    /// dimensioned by it (a Lambda line item is scoped to no instance type).
    pub fn property(&self, name: &str) -> Option<&str> {
        self.properties
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }

    /// The region this line item is billed in. Every line item is billed in one.
    pub fn region(&self) -> Option<&str> {
        self.property(PROPERTY_REGION)
    }

    /// The instance family this line item is priced for, when it is priced per
    /// instance.
    pub fn instance_family(&self) -> Option<&str> {
        self.property(PROPERTY_INSTANCE_FAMILY)
    }

    /// Whether a `SavingsPlanRateFilter` admits this line item. `name` is a
    /// `SavingsPlanRateFilterName`; a filter with no values constrains nothing.
    ///
    /// A filter names either one of the line item's own billing dimensions —
    /// the product, the service that bills it, its usage type or its billing
    /// operation — or one of the properties the item is priced along, and a line
    /// item that is not dimensioned by the property a filter names cannot
    /// satisfy it — the crate's one property-filter rule
    /// ([`property_filter_admits`]), over the fields a rate filter can name.
    pub fn matches_filter(&self, name: &str, values: Option<&Vec<String>>) -> bool {
        let field = match name {
            "productType" => Some(self.product_type.as_str()),
            "serviceCode" => Some(self.service_code.as_str()),
            "usageType" => Some(self.usage_type.as_str()),
            "operation" => Some(self.operation.as_str()),
            property => self.property(property),
        };
        property_filter_admits(values, field)
    }

    /// How many fractional digits the catalog quotes this line item's prices at:
    /// the precision of its own published on-demand price
    /// ([`quoted_digits`]), so a rate is reported at the precision the product
    /// it prices is published at.
    pub fn quoted_digits(&self) -> usize {
        quoted_digits(self.on_demand_units)
    }

    /// The catalog's rate for this line item under a commitment earning
    /// `discount_basis_points`, in [`AMOUNT_SCALE`] units: its own on-demand
    /// price less that discount, HELD at the precision the catalog quotes this
    /// product at — so the value a client compares or hashes is the catalog's
    /// price for this combination and not a figure carried to the billing scale.
    pub fn rate_units(&self, discount_basis_points: i128) -> i128 {
        quote_price_units(
            discounted_units(discount_basis_points, self.on_demand_units),
            self.quoted_digits(),
        )
    }

    /// That rate as the catalog spells it: `Amount` targets `string`, so the
    /// spelling IS the wire value, and the catalog spells a price exactly as it
    /// holds it ([`format_price`]) rather than padded to the billing scale
    /// ([`AMOUNT_WIRE_DIGITS`]) a commitment and its payments are held at.
    pub fn rate(&self, discount_basis_points: i128) -> String {
        format_price(self.rate_units(discount_basis_points))
    }
}

/// [`EC2_PLATFORMS`] in the order a rate card lists an instance type's platform
/// rows: the dearest licence first, so every licensed platform precedes the
/// unlicensed Linux row and the rows of one instance type read down from the
/// most expensive platform to the cheapest.
///
/// A rate card is a page of a price list, and a price list lists the platforms of
/// one instance type in its own order rather than in the order the
/// platform-details table happens to document them.
pub fn ec2_platforms_in_card_order() -> Vec<(&'static str, &'static str, i128)> {
    let mut platforms = EC2_PLATFORMS.to_vec();
    // Stable, so platforms carrying the same licence keep the catalog's
    // reference order among themselves.
    platforms.sort_by_key(|platform| std::cmp::Reverse(platform.2));
    platforms
}

/// Every usage line item the catalog prices, in a stable order.
///
/// One exists for each product type the model declares, in each region the
/// catalog sells in, and for each dimension that product is priced along: an
/// instance type and platform for EC2, an instance type for SageMaker, a single
/// line item for every other product. The outer order is the model's own
/// `SavingsPlanProductType` value set
/// ([`crate::ops::types::SAVINGS_PLAN_PRODUCT_TYPE_VALUES`]), then region, then
/// the dimensions, so page N of a search is the same rows on every call.
///
/// A line item's `usageType` names the usage alone and carries no region prefix:
/// the region it is billed in is one of the dimensions the line item is priced
/// along ([`PROPERTY_REGION`]), so it is reported there rather than built into
/// the usage type, and a usage record's unprefixed usage type joins against the
/// rate that prices it.
///
/// Derived from the model's value set rather than written out by hand, so a
/// product type the model gains is a product type the catalog is asked to price.
///
/// Built ONCE: it is a pure function of the tables above, so every request reads
/// the same slice instead of rebuilding it.
pub fn rate_line_items() -> &'static [RateLineItem] {
    static LINE_ITEMS: OnceLock<Vec<RateLineItem>> = OnceLock::new();
    LINE_ITEMS.get_or_init(build_rate_line_items)
}

/// Build the line items [`rate_line_items`] hands out.
fn build_rate_line_items() -> Vec<RateLineItem> {
    let mut items = Vec::new();
    // The platform order a card lists an instance type's rows in, resolved once
    // for the whole catalog rather than per instance type.
    let platforms = ec2_platforms_in_card_order();
    for product_type in crate::ops::types::SAVINGS_PLAN_PRODUCT_TYPE_VALUES {
        for (region, _) in REGIONS {
            match *product_type {
                "EC2" => {
                    for (family, vcpu_units) in EC2_INSTANCE_FAMILIES {
                        for (size, vcpus) in EC2_SIZES {
                            let instance_type = format!("{family}.{size}");
                            for (product_description, operation, licence_units) in &platforms {
                                items.push(line_item(
                                    product_type,
                                    "AmazonEC2",
                                    format!("BoxUsage:{instance_type}"),
                                    operation,
                                    "Hrs",
                                    vcpu_units
                                        .saturating_add(*licence_units)
                                        .saturating_mul(vcpus),
                                    vec![
                                        (RATE_PROPERTY_TENANCY, TENANCY_SHARED),
                                        (RATE_PROPERTY_INSTANCE_TYPE, &instance_type),
                                        (PROPERTY_REGION, region),
                                        (RATE_PROPERTY_PRODUCT_DESCRIPTION, product_description),
                                        (PROPERTY_INSTANCE_FAMILY, family),
                                    ],
                                ));
                            }
                        }
                    }
                }
                "SageMaker" => {
                    for (instance_type, hourly_units) in SAGEMAKER_INSTANCE_TYPES {
                        // An ML instance type's family is the type without its
                        // size (`ml.m5.xlarge` is an `ml.m5`).
                        let family = instance_type
                            .rsplit_once('.')
                            .map_or(instance_type, |(family, _)| family);
                        items.push(line_item(
                            product_type,
                            "AmazonSageMaker",
                            format!("Hosting:{instance_type}"),
                            "RunInstance",
                            "Hrs",
                            hourly_units,
                            vec![
                                (RATE_PROPERTY_INSTANCE_TYPE, instance_type),
                                (PROPERTY_REGION, region),
                                (PROPERTY_INSTANCE_FAMILY, family),
                            ],
                        ));
                    }
                }
                other => {
                    let Some(priced) = PRICED_PRODUCTS
                        .iter()
                        .find(|priced| priced.product_type == other)
                    else {
                        continue;
                    };
                    for service_code in priced.service_codes {
                        items.push(line_item(
                            product_type,
                            service_code,
                            priced.usage_token.to_string(),
                            priced.operation,
                            priced.unit,
                            priced.on_demand_units,
                            vec![(PROPERTY_REGION, region)],
                        ));
                    }
                }
            }
        }
    }
    items
}

/// Whether a commitment that covers `product_types` and is scoped to `region`
/// and `instance_family` prices `item`: it covers the item's product, and its
/// scope admits the item's region and instance family.
///
/// A commitment scoped to neither (a Compute Savings Plan) prices the product
/// wherever it is billed; one scoped to a region prices it in that region; one
/// scoped to an instance family as well prices only that family. So a client
/// comparing what each offering charges for the usage it runs sees every
/// offering that would cover that usage, and the rates of a plan already bought
/// are the line items its own scope covers.
pub fn scope_prices(
    product_types: &[String],
    region: Option<&str>,
    instance_family: Option<&str>,
    item: &RateLineItem,
) -> bool {
    product_types
        .iter()
        .any(|covered| covered.as_str() == item.product_type)
        && region.is_none_or(|region| Some(region) == item.region())
        && instance_family.is_none_or(|family| Some(family) == item.instance_family())
}

/// Whether `offering` prices `item`, by the scope it is sold for. See
/// [`scope_prices`].
pub fn offering_prices(offering: &Offering, item: &RateLineItem) -> bool {
    scope_prices(
        &offering.product_types,
        offering.property(PROPERTY_REGION),
        offering.property(PROPERTY_INSTANCE_FAMILY),
        item,
    )
}

/// One line item of the rate catalog.
///
/// `properties` are given in the CATALOG's own order and are reported in it: the
/// dimensions that identify the usage being priced first — its tenancy, its
/// instance type, the region it is billed in, then the platform it runs — and
/// the dimension of a commitment's SCOPE ([`PROPERTY_INSTANCE_FAMILY`], which a
/// plan rate does not repeat because the instance type implies it) last. Two line
/// items of one product therefore report their dimensions in the same order.
fn line_item(
    product_type: &str,
    service_code: &str,
    usage_type: String,
    operation: &str,
    unit: &str,
    on_demand_units: i128,
    properties: Vec<(&str, &str)>,
) -> RateLineItem {
    RateLineItem {
        product_type: product_type.to_string(),
        service_code: service_code.to_string(),
        usage_type,
        operation: operation.to_string(),
        unit: unit.to_string(),
        on_demand_units,
        properties: properties
            .into_iter()
            .map(|(name, value)| (name.to_string(), value.to_string()))
            .collect(),
    }
}

// ---------------------------------------------------------------------------
// Purchased plans
// ---------------------------------------------------------------------------

/// A purchased Savings Plan, as this service stores it. Projected onto the wire
/// as `SavingsPlan` by [`SavingsPlanRecord::project`].
#[derive(Debug, Clone)]
pub struct SavingsPlanRecord {
    pub savings_plan_id: String,
    pub savings_plan_arn: String,
    pub offering_id: String,
    pub description: String,
    /// The plan's term start, epoch millis: the purchase time.
    pub start_millis: i64,
    /// The plan's term end, epoch millis: the LAST instant of the term, which is
    /// one second before `start` plus the term duration.
    pub end_millis: i64,
    pub state: String,
    /// When the clock owes this plan its move out of `queued`.
    pub pending_due_millis: i64,
    /// When the clock owes this plan its move out of `payment-pending`.
    pub active_due_millis: i64,
    pub region: Option<String>,
    pub ec2_instance_family: Option<String>,
    pub savings_plan_type: String,
    pub payment_option: String,
    pub product_types: Vec<String>,
    pub currency: String,
    /// The hourly commitment, in [`AMOUNT_SCALE`] units. Held as a NUMBER, not
    /// as the string it is rendered to, so one amount has exactly one spelling
    /// on the wire ([`format_amount`]) and a filter on it compares numbers.
    pub commitment_units: i128,
    /// The up-front payment, in [`AMOUNT_SCALE`] units.
    pub upfront_payment_units: i128,
    /// The recurring hourly payment, in [`AMOUNT_SCALE`] units.
    pub recurring_payment_units: i128,
    pub term_duration_in_seconds: i64,
    pub tags: BTreeMap<String, String>,
    /// When this plan's return was accepted, epoch millis, or `None` for a plan
    /// that was never returned.
    ///
    /// It is what the per-calendar-month return allowance
    /// ([`MAX_RETURNS_PER_CALENDAR_MONTH`]) is counted from, so the allowance is
    /// DERIVED from the plans the store already holds rather than tallied in a
    /// second place that could drift from them.
    pub returned_at_millis: Option<i64>,
}

impl SavingsPlanRecord {
    /// Whether a return can be requested for this plan RIGHT NOW: it must have
    /// started and been paid for, and still be inside its return window.
    ///
    /// A plan that has not started yet (`queued`) is not returned but cancelled,
    /// through `DeleteQueuedSavingsPlan`, so it is not returnable here however
    /// open its window is.
    pub fn is_returnable(&self, now_millis: i64) -> bool {
        matches!(
            self.state.as_str(),
            STATE_PAYMENT_PENDING | STATE_ACTIVE
        ) && now_millis <= self.return_window_closes_millis()
    }

    /// Whether this plan still HAS a return window: its purchase has not already
    /// been returned, and the window its start opened has not run out.
    ///
    /// Wider than [`Self::is_returnable`]: the window is a property of the
    /// PURCHASE, which is why a plan that has not started yet — and one whose
    /// queued purchase was cancelled — still reports the instant its window
    /// closes even though a return cannot be requested for it. Only a plan whose
    /// return was already accepted has consumed the window; for every other
    /// state the clock closes it.
    pub fn has_return_window(&self, now_millis: i64) -> bool {
        self.state != STATE_RETURNED && now_millis <= self.return_window_closes_millis()
    }

    /// The instant this plan's return window closes. Saturating: `start_millis`
    /// carries a caller-supplied purchase time.
    pub fn return_window_closes_millis(&self) -> i64 {
        self.start_millis.saturating_add(RETURN_WINDOW_MILLIS)
    }

    /// `returnableUntil`: "the time until when a return for the Savings Plan can
    /// be requested. If the Savings Plan is not returnable, the field reflects
    /// the Savings Plans start time."
    ///
    /// Reported from [`Self::has_return_window`], not from
    /// [`Self::is_returnable`]: the member is the instant the window CLOSES,
    /// which a plan whose term has not begun still has.
    pub fn returnable_until_millis(&self, now_millis: i64) -> i64 {
        if self.has_return_window(now_millis) {
            self.return_window_closes_millis()
        } else {
            self.start_millis
        }
    }

    /// Whether this plan's commitment prices `item` — one of the line items its
    /// rates cover.
    ///
    /// Asked of the scope the plan was BOUGHT with, so a plan's rate card covers
    /// the line items the offering it was bought from prices. See
    /// [`scope_prices`].
    pub fn prices(&self, item: &RateLineItem) -> bool {
        scope_prices(
            &self.product_types,
            self.region.as_deref(),
            self.ec2_instance_family.as_deref(),
            item,
        )
    }

    /// The discount this plan earns, in basis points: the ladder the offering it
    /// was bought from is priced by, asked of the plan's own type, term and
    /// payment option.
    pub fn discount_basis_points(&self) -> i128 {
        discount_basis_points(
            &self.savings_plan_type,
            self.term_duration_in_seconds,
            &self.payment_option,
        )
    }

    /// The catalog's rate for `item` under this plan, as the catalog spells it:
    /// the line item's own price less the discount this plan's type, term and
    /// payment option earn — the ladder the offering it was bought from is
    /// priced by, so a plan's rate for a line item is that offering's rate for
    /// it. See [`RateLineItem::rate`].
    pub fn rate(&self, item: &RateLineItem) -> String {
        item.rate(self.discount_basis_points())
    }

    /// Record that this plan's return was accepted at `now_millis`: it leaves the
    /// states a return is accepted from, and the return counts against the
    /// calendar month `now_millis` falls in.
    pub fn mark_returned(&mut self, now_millis: i64) {
        self.state = STATE_RETURNED.to_string();
        self.returned_at_millis = Some(now_millis);
    }

    /// Merge `tags` into this plan's tags, refusing the WHOLE write when the
    /// result would break [`check_tags`] — so a refused `TagResource` leaves the
    /// stored tag set exactly as it was, and no caller can grow it past the
    /// service's tag limit.
    ///
    /// The [`TagRejection`] says which refusal it was, so the handler reports a
    /// tag set that does not fit the allowance as a quota failure and a
    /// malformed tag as a constraint one.
    pub fn merge_tags(
        &mut self,
        tags: impl IntoIterator<Item = (String, String)>,
    ) -> Result<(), TagRejection> {
        let mut merged = self.tags.clone();
        merged.extend(tags);
        check_tags(&merged)?;
        self.tags = merged;
        Ok(())
    }

    /// Remove the named tags from this plan, ignoring a key it does not carry.
    ///
    /// `UntagResource` states the keys the resource must END UP without, so a key
    /// that was never there is already in the state the caller asked for: an
    /// absent key is not an error, which is what makes a retried untag safe.
    /// Nothing here can break [`check_tags`] — removal only shrinks the set.
    pub fn remove_tags<'a>(&mut self, tag_keys: impl IntoIterator<Item = &'a str>) {
        for key in tag_keys {
            self.tags.remove(key);
        }
    }

    /// Whether a `SavingsPlanFilter` admits this plan. `name` is a
    /// `SavingsPlansFilterName`; a filter with no values constrains nothing.
    pub fn matches_filter(&self, name: &str, values: Option<&Vec<String>>) -> bool {
        // An amount or term filter compares NUMBERS and an instant filter
        // compares INSTANTS, so a filter spelled "0.100" matches a commitment
        // held as "0.1" and one spelled with an offset matches the same moment.
        let (field, kind): (Option<String>, FilterKind) = match name {
            "region" => (self.region.clone(), FilterKind::Text),
            "ec2-instance-family" | "instance-family" => {
                (self.ec2_instance_family.clone(), FilterKind::Text)
            }
            "commitment" => (
                Some(format_amount(self.commitment_units)),
                FilterKind::Number(self.commitment_units),
            ),
            "upfront" => (
                Some(format_amount(self.upfront_payment_units)),
                FilterKind::Number(self.upfront_payment_units),
            ),
            "term" => (
                Some(self.term_duration_in_seconds.to_string()),
                FilterKind::Number(
                    i128::from(self.term_duration_in_seconds).saturating_mul(AMOUNT_SCALE),
                ),
            ),
            "savings-plan-type" => (Some(self.savings_plan_type.clone()), FilterKind::Text),
            "payment-option" => (Some(self.payment_option.clone()), FilterKind::Text),
            "start" => (
                Some(format_millis(self.start_millis)),
                FilterKind::Instant(self.start_millis),
            ),
            "end" => (
                Some(format_millis(self.end_millis)),
                FilterKind::Instant(self.end_millis),
            ),
            _ => (None, FilterKind::Text),
        };
        match (values, field) {
            (Some(values), _) if values.is_empty() => true,
            (None, _) => true,
            (Some(_), None) => false,
            (Some(values), Some(field)) => values.iter().any(|value| {
                if *value == field {
                    return true;
                }
                match kind {
                    FilterKind::Text => false,
                    FilterKind::Number(units) => {
                        parse_amount_at_wire_scale(value) == Some(units)
                    }
                    FilterKind::Instant(millis) => parse_millis(value) == Some(millis),
                }
            }),
        }
    }
}

/// How a `SavingsPlanFilter`'s value is compared with the member it names.
#[derive(Debug, Clone, Copy)]
enum FilterKind {
    /// Compared as text, the way the model's enum-valued members are.
    Text,
    /// Compared as a number (the member's value in [`AMOUNT_SCALE`] units), so
    /// two spellings of one amount are one value.
    Number(i128),
    /// Compared as an instant (epoch millis), for the same reason.
    Instant(i64),
}

/// The most Savings Plans this account may hold.
///
/// A quota, answered with the service's own modelled
/// `ServiceQuotaExceededException`: the store keeps every plan ever purchased
/// (a Savings Plan is never deleted — it is returned, cancelled or retired),
/// so without a ceiling one caller could grow it for the life of the process.
pub const MAX_PLANS_PER_ACCOUNT: usize = 1000;

/// The most Savings Plans this account may RETURN in one calendar month.
///
/// A return is bounded by more than the 168-hour window: AWS documents
/// per-calendar-month limits on returns per payer account ("Returning a Savings
/// Plan", AWS Savings Plans User Guide — besides the 7-day window, there are
/// caps on how many Savings Plans and how much cumulative commitment may be
/// returned in a calendar month), and `ReturnSavingsPlan` models
/// `ServiceQuotaExceededException` (HTTP 402) for exhausting them.
///
/// The FIGURE is this emulator's, the way [`MAX_PLANS_PER_ACCOUNT`] is: what the
/// service owes a caller is that the allowance exists, that it is counted per
/// payer account per calendar month, and that the return past it is refused with
/// the modelled 402 — not a particular number.
pub const MAX_RETURNS_PER_CALENDAR_MONTH: usize = 10;

/// The largest cumulative hourly commitment, in [`AMOUNT_SCALE`] units, this
/// account may return in one calendar month. See
/// [`MAX_RETURNS_PER_CALENDAR_MONTH`] for where the allowance comes from; this
/// is the same allowance counted in commitment rather than in plans, so a
/// handful of very large returns is bounded as well as a crowd of small ones.
pub const MAX_RETURNED_COMMITMENT_UNITS_PER_CALENDAR_MONTH: i128 = 1000 * AMOUNT_SCALE;

/// The calendar month `millis` falls in, as `YYYY-MM`.
///
/// Taken from [`format_millis`], so a month is the UTC calendar month — the same
/// zone every instant this service renders is in — and the boundary between two
/// months is one instant, not a range.
pub fn calendar_month(millis: i64) -> String {
    format_millis(millis).chars().take("YYYY-MM".len()).collect()
}

/// Per-service state for com-amazonaws-savingsplans-awssavingsplan. A handler
/// reaches it through [`service_state`].
#[derive(Debug)]
pub struct ComAmazonawsSavingsplansAwssavingsplanState {
    /// The seeded, read-only catalog.
    offerings: Vec<Offering>,
    /// Every plan sold, in purchase order.
    plans: Vec<SavingsPlanRecord>,
    /// `savingsPlanId` -> index into `plans`.
    plan_index: HashMap<String, usize>,
    /// `savingsPlanArn` -> index into `plans`.
    arn_index: HashMap<String, usize>,
    /// `<operation>/<clientToken>` -> the request that token's first call
    /// carried (its [`fingerprint`]) and the `savingsPlanId` it resolved to.
    idempotency: HashMap<String, (String, String)>,
    /// This store's id seed, and how many ids it has minted: see [`mint_id`].
    id_seed: u128,
    minted_ids: u64,
}

impl Default for ComAmazonawsSavingsplansAwssavingsplanState {
    fn default() -> Self {
        let seed = id_seed();
        let offerings = seed_offerings(seed);
        Self {
            minted_ids: offerings.len() as u64,
            offerings,
            plans: Vec::new(),
            plan_index: HashMap::new(),
            arn_index: HashMap::new(),
            idempotency: HashMap::new(),
            id_seed: seed,
        }
    }
}

impl ComAmazonawsSavingsplansAwssavingsplanState {
    /// Mint a fresh id for a resource this store is creating. See [`mint_id`].
    pub fn next_id(&mut self) -> String {
        self.minted_ids = self.minted_ids.saturating_add(1);
        mint_id(self.id_seed, self.minted_ids)
    }

    /// The whole catalog, in catalog order.
    pub fn offerings(&self) -> &[Offering] {
        &self.offerings
    }

    /// The catalog entry with this offering id.
    pub fn offering(&self, offering_id: &str) -> Option<&Offering> {
        self.offerings
            .iter()
            .find(|offering| offering.offering_id == offering_id)
    }

    /// Every plan sold, in purchase order.
    pub fn plans(&self) -> &[SavingsPlanRecord] {
        &self.plans
    }

    /// The plan with this id.
    pub fn plan(&self, savings_plan_id: &str) -> Option<&SavingsPlanRecord> {
        self.plan_index
            .get(savings_plan_id)
            .and_then(|index| self.plans.get(*index))
    }

    /// The plan with this id, mutably.
    pub fn plan_mut(&mut self, savings_plan_id: &str) -> Option<&mut SavingsPlanRecord> {
        let index = *self.plan_index.get(savings_plan_id)?;
        self.plans.get_mut(index)
    }

    /// The plan with this ARN.
    pub fn plan_by_arn(&self, arn: &str) -> Option<&SavingsPlanRecord> {
        self.arn_index
            .get(arn)
            .and_then(|index| self.plans.get(*index))
    }

    /// The plan with this ARN, mutably.
    pub fn plan_by_arn_mut(&mut self, arn: &str) -> Option<&mut SavingsPlanRecord> {
        let index = *self.arn_index.get(arn)?;
        self.plans.get_mut(index)
    }

    /// Whether another purchase would exceed [`MAX_PLANS_PER_ACCOUNT`].
    pub fn at_plan_quota(&self) -> bool {
        self.plans.len() >= MAX_PLANS_PER_ACCOUNT
    }

    /// What this account has already returned in the calendar month `now_millis`
    /// falls in: how many plans, and their cumulative hourly commitment in
    /// [`AMOUNT_SCALE`] units.
    ///
    /// Counted from the stored plans' [`SavingsPlanRecord::returned_at_millis`],
    /// so a return recorded in an earlier month does not consume this month's
    /// allowance and nothing has to be reset when the month turns over.
    pub fn returns_in_calendar_month(&self, now_millis: i64) -> (usize, i128) {
        let month = calendar_month(now_millis);
        let mut returned = 0_usize;
        let mut commitment = 0_i128;
        for plan in &self.plans {
            if plan
                .returned_at_millis
                .is_some_and(|at| calendar_month(at) == month)
            {
                returned = returned.saturating_add(1);
                commitment = commitment.saturating_add(plan.commitment_units);
            }
        }
        (returned, commitment)
    }

    /// Whether returning a plan of `commitment_units` at `now_millis` would take
    /// this account past its return allowance for that calendar month, and if so
    /// which cap it breaks and how much of the allowance is left.
    ///
    /// `Some(message)` is a quota refusal: the caller answers it with the
    /// operation's modelled `ServiceQuotaExceededException` and changes nothing.
    pub fn return_allowance_exceeded(
        &self,
        commitment_units: i128,
        now_millis: i64,
    ) -> Option<String> {
        let month = calendar_month(now_millis);
        let (returned, committed) = self.returns_in_calendar_month(now_millis);
        if returned >= MAX_RETURNS_PER_CALENDAR_MONTH {
            return Some(format!(
                "this account has already returned its allowance of {MAX_RETURNS_PER_CALENDAR_MONTH} \
                 Savings Plans in {month}"
            ));
        }
        let total = committed.saturating_add(commitment_units);
        if total > MAX_RETURNED_COMMITMENT_UNITS_PER_CALENDAR_MONTH {
            return Some(format!(
                "returning a Savings Plan committed at {} an hour would take this account past its \
                 allowance of {} an hour of returned commitment in {month}: {} an hour has already \
                 been returned",
                format_amount(commitment_units),
                format_amount(MAX_RETURNED_COMMITMENT_UNITS_PER_CALENDAR_MONTH),
                format_amount(committed)
            ));
        }
        None
    }

    /// Record a newly purchased plan, keeping both indexes in step.
    pub fn insert_plan(&mut self, record: SavingsPlanRecord) {
        let index = self.plans.len();
        self.plan_index
            .insert(record.savings_plan_id.clone(), index);
        self.arn_index.insert(record.savings_plan_arn.clone(), index);
        self.plans.push(record);
    }

    /// What an earlier call with this `clientToken` carried and resolved to:
    /// `(fingerprint, savingsPlanId)`.
    pub fn idempotent(&self, operation: &str, client_token: &str) -> Option<(&str, &str)> {
        self.idempotency
            .get(&format!("{operation}/{client_token}"))
            .map(|(fingerprint, id)| (fingerprint.as_str(), id.as_str()))
    }

    /// Remember the request this `clientToken` carried and the plan it resolved
    /// to, so a replay of the SAME request answers with that plan and a replay
    /// of a different one can be refused.
    pub fn remember_idempotent(
        &mut self,
        operation: &str,
        client_token: &str,
        fingerprint: &str,
        savings_plan_id: &str,
    ) {
        self.idempotency.insert(
            format!("{operation}/{client_token}"),
            (fingerprint.to_string(), savings_plan_id.to_string()),
        );
    }

    /// Apply every state transition the clock already owes at `now_millis`.
    ///
    /// A queued purchase becomes payable at its purchase time, a payable plan
    /// becomes active once its payment has settled, and an active plan retires
    /// when its term ends. Each was owed at that instant whether or not anybody
    /// called, so a reporting operation materialises them before it answers
    /// rather than inventing them.
    pub fn materialize_due(&mut self, now_millis: i64) {
        for plan in &mut self.plans {
            if plan.state == STATE_QUEUED && now_millis >= plan.pending_due_millis {
                plan.state = STATE_PAYMENT_PENDING.to_string();
            }
            if plan.state == STATE_PAYMENT_PENDING && now_millis >= plan.active_due_millis {
                plan.state = STATE_ACTIVE.to_string();
            }
            if matches!(plan.state.as_str(), STATE_PAYMENT_PENDING | STATE_ACTIVE)
                && now_millis > plan.end_millis
            {
                plan.state = STATE_RETIRED.to_string();
            }
        }
    }
}

/// Build the record for a purchase of `offering`.
///
/// `start_millis` is the purchase time: a purchase dated in the future is
/// *queued* (it has not been paid for yet), one dated now or earlier is
/// `payment-pending` until its payment settles.
#[allow(clippy::too_many_arguments)]
pub fn purchase(
    offering: &Offering,
    savings_plan_id: String,
    commitment_units: i128,
    upfront_payment_units: i128,
    recurring_payment_units: i128,
    purchase_millis: i64,
    now_millis: i64,
    tags: BTreeMap<String, String>,
) -> SavingsPlanRecord {
    let start_millis = truncate_to_second(purchase_millis);
    let queued = start_millis > now_millis;
    SavingsPlanRecord {
        savings_plan_arn: savings_plan_arn(&savings_plan_id),
        savings_plan_id,
        offering_id: offering.offering_id.clone(),
        description: offering.description.clone(),
        start_millis,
        // Saturating throughout: `start_millis` carries a caller-supplied
        // purchase time, so none of this arithmetic may be partial.
        //
        // A term is reported by the LAST instant it covers, not by the first
        // instant after it: a one-year term that starts at `T` ends at
        // `T + 1 year - 1s`, so two consecutive terms of one commitment do not
        // both claim the instant between them and term arithmetic over `start`
        // and `end` spans exactly `termDurationInSeconds`.
        end_millis: start_millis
            .saturating_add(offering.duration_seconds.saturating_mul(1000))
            .saturating_sub(TERM_END_INCLUSIVE_MILLIS),
        state: if queued {
            STATE_QUEUED.to_string()
        } else {
            STATE_PAYMENT_PENDING.to_string()
        },
        pending_due_millis: start_millis,
        active_due_millis: if queued {
            start_millis.saturating_add(ACTIVATION_DELAY_MILLIS)
        } else {
            now_millis.saturating_add(ACTIVATION_DELAY_MILLIS)
        },
        region: offering.property(PROPERTY_REGION).map(str::to_string),
        ec2_instance_family: offering
            .property(PROPERTY_INSTANCE_FAMILY)
            .map(str::to_string),
        savings_plan_type: offering.plan_type.clone(),
        payment_option: offering.payment_option.clone(),
        product_types: offering.product_types.clone(),
        currency: offering.currency.clone(),
        commitment_units,
        upfront_payment_units,
        recurring_payment_units,
        term_duration_in_seconds: offering.duration_seconds,
        tags,
        returned_at_millis: None,
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsSavingsplansAwssavingsplanState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsSavingsplansAwssavingsplanState>)
}
