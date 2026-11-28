// SPDX-License-Identifier: CC0-1.0

//! How this service states that a PARAMETER its own request code requires is not in the
//! request at all.
//!
//! A fact of the request's own shape rather than of the table and item store, so it lives
//! beside [`super`] rather than inside it; `super` re-exports it, so every caller still
//! names it `crate::state::…`.

/// The sentence a request that carries no value for such a parameter earns.
///
/// Not a clause and not inside the `<N> validation error(s) detected: ` envelope: the
/// absence is found by the service's own request code before any constraint is walked, so
/// the whole message is this one sentence, with the member named as the model spells it on
/// the wire and nothing of the operation that was called.
///
/// Which operations answer an absent member this way, and which keep the validating walk's
/// `Member must not be null` clause, follows the OPERATION served and is recorded in
/// `src/service.rs` beside the other refusal wordings that do.
pub fn missing_parameter_message(member: &str) -> String {
    format!("The parameter '{member}' is required but was not present in the request")
}

/// The member path the key schema of a table declaration is reported under, as the
/// validating walk spells it (lowerCamel).
pub const KEY_SCHEMA_MEMBER_PATH: &str = "keySchema";

/// The THIRD way the same absence is stated: the sentence the body READER itself raises
/// for a member it ran out of entries for, which names the member and where in the body
/// the read of the enclosing object stopped.
///
/// The surface whose request reader words it this way states no parameter check and no
/// constraint clause for the absence at all: the read of the body is what fails, so the
/// message is the reader's — the member named as the model spells it on the wire, between
/// backticks, and the ONE-based line and column just past the object the read was building
/// (a read only runs out of members once that object has been consumed whole). Which
/// operations answer an absent member this way is recorded in `src/service.rs` beside the
/// other refusal wordings that follow the operation served.
pub fn missing_field_message(member: &str, line: usize, column: usize) -> String {
    format!("missing field `{member}` at line {line} column {column}")
}

/// The other way the same absence is stated: the validating WALK's own clause for a member
/// the request carries no value for, inside the `<N> validation error(s) detected: `
/// envelope that counts it.
///
/// A member the MODEL marks `@required` earns this clause from that walk, before any handler
/// runs; a member the model leaves optional while the service's own request code requires it
/// is found absent by that code instead, and the ones observed to be stated this way rather
/// than with the sentence above are worded here so the two readings of the same absence
/// cannot drift apart. The clause states `Value null` — the absence itself — and names the
/// member by the path that walk reports, which is the path a client reads out of the message.
pub fn absent_member_message(path: &str) -> String {
    format!("1 validation error detected: {}", absent_member_clause(path))
}

/// That clause on its own, outside the envelope that counts it: what a refusal stating
/// SEVERAL violated members carries for the one the request states no value for, beside
/// the clauses of the other members it is counted with.
///
/// The one wording for the absence, so the two readings of it cannot drift apart.
pub fn absent_member_clause(path: &str) -> String {
    format!(
        "Value null at '{path}' failed to satisfy constraint: Member must not be null"
    )
}

/// The longest an ATTRIBUTE NAME may be, in bytes: the maximum the shape every member that
/// carries one targets declares (`smithy_query(query="shape",
/// shape="com.amazonaws.dynamodb#AttributeName")` gives `@length` 0..65535).
pub const MAX_ATTRIBUTE_NAME_BYTES: usize = 65535;

/// The `ValidationException` message a request carrying an ATTRIBUTE NAME longer than that
/// earns, whichever member carried it.
///
/// DynamoDB's own sentence for the class, not a constraint clause: it states the ceiling as
/// the first size that is too large ([`MAX_ATTRIBUTE_NAME_BYTES`] plus one), with no member
/// path and no constraint.
///
/// `key` is the KEY the request carried the name under, where it carried one under a key a
/// caller would recognise: a substitution entry maps a placeholder to the name, so the
/// placeholder is what tells a caller WHICH entry of its table was refused. Everywhere else
/// the over-long name IS the key or the element the request sent — a bare list of attribute
/// names, a map keyed by one, the key of a nested map — so there is no second name to state
/// and the sentence is the bound alone.
///
/// The names a request can carry one in are the same shape on every member and every
/// operation, so one sentence answers for all of them; `src/service.rs` is where a violation
/// the validating walk reports against such a member reaches it, because that walk refuses
/// the request before any handler could read the name.
pub fn attribute_name_too_large_message(key: Option<&str>) -> String {
    let ceiling = MAX_ATTRIBUTE_NAME_BYTES + 1;
    match key {
        Some(key) => {
            format!("Attribute name is too large, must be less than {ceiling} bytes for key {key}")
        }
        None => format!("Attribute name is too large, must be less than {ceiling} bytes"),
    }
}
