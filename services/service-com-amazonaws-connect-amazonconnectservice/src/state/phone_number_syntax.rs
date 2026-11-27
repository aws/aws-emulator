// SPDX-License-Identifier: CC0-1.0

//! The form a phone-number member must have, and why this service checks it rather
//! than the request boundary.
//!
//! `com.amazonaws.connect#PhoneNumber` and `com.amazonaws.connect#PhoneNumberPrefix`
//! carry their `@pattern` in the published model with every backslash of the
//! expression escaped a second time, so the regular expression the trait holds asks
//! for a LITERAL backslash where the published expression asks for a `+` or a digit
//! class. No value of the documented form satisfies it — `+15551234567` does not —
//! so the boundary would refuse every phone number a caller can send, for an
//! operation AWS answers. `com.amazonaws.connect#SensitivePhoneNumber`, the same
//! E.164 number in the same model, spells that expression with one level of escaping,
//! which is also the form the Connect API Reference publishes for these two members.
//!
//! So the two members carry `skip_validation` on their input shapes in
//! `src/ops/types/`, which keeps the unsatisfiable expression off the member schema,
//! and the expressions below — the model's own, with the second escaping level
//! removed — are what their operations check a value against. The refusal is this
//! service's own modelled `InvalidParameterException`, the way every other value it
//! cannot read is refused rather than left to the boundary's generic prose (see
//! [`super::invalid_parameter`] and [`super::invalid_enum_value`]).
//!
//! `tests/all/phone_number_syntax.rs` pins each expression against the shape the
//! model snapshot declares it on: a refreshed snapshot that spells one of them as
//! published fails there, and the local check should then give way to the boundary's.

/// The expression `com.amazonaws.connect#PhoneNumber` publishes: an E.164 number, a
/// `+` followed by a non-zero leading digit and up to fourteen more digits. It is the
/// snapshot's own expression for that shape with the second escaping level removed,
/// character for character the one `com.amazonaws.connect#SensitivePhoneNumber`
/// carries.
pub const PHONE_NUMBER_PATTERN: &str = r"^\+[1-9]\d{1,14}$";

/// The expression `com.amazonaws.connect#PhoneNumberPrefix` publishes: one to eleven
/// digits, with or without the leading `+`, so the same prefix selects the same
/// numbers written either way. Read from the snapshot the same way
/// [`PHONE_NUMBER_PATTERN`] is.
pub const PHONE_NUMBER_PREFIX_PATTERN: &str = r"^\+?[0-9]{1,11}$";

/// The member a refusal of a phone number outside [`PHONE_NUMBER_PATTERN`] names,
/// spelled as the model spells it.
pub const PHONE_NUMBER_PARAMETER: &str = "PhoneNumber";

/// The member a refusal of a prefix outside [`PHONE_NUMBER_PREFIX_PATTERN`] names,
/// spelled as the model spells it.
pub const PHONE_NUMBER_PREFIX_PARAMETER: &str = "PhoneNumberPrefix";

/// Whether `value` is a phone number of the form the model publishes
/// ([`PHONE_NUMBER_PATTERN`]).
///
/// The expression is evaluated the way every request of this tree is held to a
/// `@pattern` — by
/// [`::service_common::unapplied_patterns::modelled_pattern_admits`], which compiles a
/// model expression once and searches it the way the boundary searches the ones it
/// applies — so a member checked here and a member checked there answer one rule. Both
/// expressions here are MODEL CONSTANTS that the linear-time engine the boundary
/// validates with compiles, so that engine is what decides a caller's value; nothing
/// is compiled from a request.
pub fn is_phone_number(value: &str) -> bool {
    ::service_common::unapplied_patterns::modelled_pattern_admits(PHONE_NUMBER_PATTERN, value)
}

/// Whether `value` is a phone-number prefix of the form the model publishes
/// ([`PHONE_NUMBER_PREFIX_PATTERN`]), evaluated as [`is_phone_number`] evaluates a
/// number.
pub fn is_phone_number_prefix(value: &str) -> bool {
    ::service_common::unapplied_patterns::modelled_pattern_admits(
        PHONE_NUMBER_PREFIX_PATTERN,
        value,
    )
}
