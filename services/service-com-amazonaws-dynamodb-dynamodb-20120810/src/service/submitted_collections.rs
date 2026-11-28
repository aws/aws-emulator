// SPDX-License-Identifier: CC0-1.0

//! The VALUE a refusal states, read off the body of the request it refuses.
//!
//! An AWS constraint clause states the value that broke the constraint as the request
//! carried it. For a COLLECTION member — a list past its `@length` maximum, a map below its
//! minimum — that value is the collection: `Value '[p00, p01, …, p20]' at
//! '<member>' failed to satisfy constraint: Member must have length less than or equal to
//! 20`. The framework's validating walk hands [`super::EmulatedService::override_refusal`]
//! its own SUMMARY of what it counted instead (`<n> items` for a list, `<n> entries` for a
//! map), which is wire text no caller ever sent and which conveys a count where AWS states
//! the members: an author of an over-long projection list cannot see from it which names to
//! trim.
//!
//! So the collection is read HERE, where the request still is — the same reason the other
//! clauses this service words itself are read off the body
//! ([`super::request_constraints`]) — at the member path that walk reports the violation
//! under. One EMPTY collection needs no request to render it (an empty list is `[]`
//! whatever it held nothing of), and that half is restored at the refusal seam itself; this
//! module answers for every collection that HELD something.
//!
//! Only a collection of SCALARS is rendered: a list of structures has no text rendering, so
//! a clause about one keeps the framework's summary rather than losing its value half
//! altogether.
//!
//! The same reading answers for a SCALAR a refusal names ([`SubmittedBody::string_at`]):
//! the walk hands that seam the value elided at its own echo ceiling, while the sentence
//! DynamoDB states for a member it resolves itself names the value as the request spelled
//! it.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::OnceLock;

use aws_smithy_runtime_api::http::Request as RtRequest;
use serde_json::Value;

/// The path segment the framework contributes when it steps from a collection into one of
/// its elements, which names no member of the body — the parent module's own, so this reader
/// and the one that re-cases a member path cannot disagree about which segments are the
/// caller's ([`super::COLLECTION_STEP`]).
use super::{upper_camel, COLLECTION_STEP};

/// The largest body this reader keeps a copy of.
///
/// What a clause quotes is ONE MEMBER, and the longest member any clause of this service
/// quotes is far below this: the widest selector the model declares is 1283 bytes
/// (`smithy_query(query="shape", shape="com.amazonaws.dynamodb#ResourceArnString")`) and the
/// widest collection a clause states is a twenty-element projection list
/// (`…#NonKeyAttributeNameList`). A request whose body is larger than this is carrying ITEM
/// bytes rather than a wider member, so nothing is copied for it and the framework's own
/// echo stands — which is also what keeps a 400 KB item write and a batch of them off a
/// second full copy they would never read back.
const MAX_READ_BODY_BYTES: usize = 256 * 1024;

/// How many member paths ONE refusal may resolve.
///
/// A refusal states at most [`aws_smithy_schema_server`]'s own violation cap of clauses, and
/// resolving a path walks the parsed document — so the work one refusal can ask for is
/// bounded here rather than by how many members a caller broke. Past this the framework's
/// own count summary and elided echo stand, which is what a clause states for every member
/// this reader declines anyway.
const MAX_RESOLVED_PATHS: usize = 4;

/// The most ONE quoted value may carry, in bytes.
///
/// With [`MAX_RESOLVED_PATHS`] this bounds `clauses x value size` as a PRODUCT, so one
/// refusal message is sized by these two constants and never by the request — the same
/// shape of bound `service_common::validation` states for the same restoration. A value
/// past it is declined rather than truncated, so no clause ever quotes a value no caller
/// sent.
const MAX_QUOTED_VALUE_BYTES: usize = 16 * 1024;

/// The body of one request, kept for the clauses of its refusal that quote a collection or a
/// value it carried.
///
/// The body is parsed AT MOST ONCE per request, on the first clause that asks for a member
/// of it ([`Self::document`]): every well-formed request of every operation would otherwise
/// pay a second full parse for a refusal that never happens, and a refusal reporting many
/// violated members would otherwise pay one parse per clause.
#[derive(Debug)]
pub struct SubmittedBody {
    /// The body as it arrived, kept only up to [`MAX_READ_BODY_BYTES`].
    body: Vec<u8>,
    /// The body read as a document, on the first clause that asks for one — `None` inside
    /// the cell for bytes no document can be read from, so a body that cannot be read is
    /// not re-read per clause either.
    document: OnceLock<Option<Value>>,
    /// How many member paths the clauses of this request's refusal have resolved
    /// ([`MAX_RESOLVED_PATHS`]).
    resolved: AtomicUsize,
}

impl SubmittedBody {
    /// `req`'s body, or `None` for a request whose body this layer cannot see and for one
    /// larger than [`MAX_READ_BODY_BYTES`].
    pub fn of(req: &RtRequest) -> Option<Self> {
        let body = req.body().bytes()?;
        (body.len() <= MAX_READ_BODY_BYTES).then(|| Self {
            body: body.to_vec(),
            document: OnceLock::new(),
            resolved: AtomicUsize::new(0),
        })
    }

    /// The collection the request carried at `path`, rendered as AWS quotes one.
    ///
    /// `None` for a body no document can be read from, for a path naming nothing the body
    /// carries, for a collection whose elements have no text rendering, and once the bounds
    /// above are reached.
    pub fn collection_at(&self, path: &str) -> Option<String> {
        let document = self.document()?;
        self.within_the_bounds(rendered_collection(resolve(document, path)?)?)
    }

    /// The STRING the request carried at `path`, whole.
    ///
    /// For the refusals this service words from the value itself: the validating walk hands
    /// [`super::EmulatedService::override_refusal`] the value elided at its own echo
    /// ceiling, while the sentence DynamoDB states for such a member names the value as the
    /// request spelled it. `None` in the cases [`Self::collection_at`] answers `None` in,
    /// and for a value that is not a string.
    pub fn string_at(&self, path: &str) -> Option<String> {
        let document = self.document()?;
        match resolve(document, path)? {
            Value::String(text) => self.within_the_bounds(text.clone()),
            _ => None,
        }
    }

    /// The body read as a document, parsed once and kept — `None` for bytes no document can
    /// be read from, and once this request's refusal has resolved as many paths as it may.
    fn document(&self) -> Option<&Value> {
        if self.resolved.fetch_add(1, Ordering::Relaxed) >= MAX_RESOLVED_PATHS {
            return None;
        }
        self.document
            .get_or_init(|| ::serde_json::from_slice::<Value>(&self.body).ok())
            .as_ref()
    }

    /// `quoted` when one clause may state it, and `None` past [`MAX_QUOTED_VALUE_BYTES`] —
    /// where the framework's own echo stands instead of a value cut short.
    fn within_the_bounds(&self, quoted: String) -> Option<String> {
        (quoted.len() <= MAX_QUOTED_VALUE_BYTES).then_some(quoted)
    }
}

/// The value `path` names in `document`, where the path is the one the validating walk
/// reports a violation under.
///
/// The walk spells a member in lowerCamel while this protocol puts the model's own
/// UpperCamel spelling on the wire, so a member is looked up under both; a list element
/// names its 1-BASED position and a map entry its key, each followed by the collection
/// marker (`aws_smithy_schema_server::constraints::path`).
fn resolve<'d>(document: &'d Value, path: &str) -> Option<&'d Value> {
    let segments: Vec<&str> = path.split('.').collect();
    let mut at = document;
    for (index, segment) in segments.iter().enumerate() {
        if *segment == COLLECTION_STEP {
            continue;
        }
        let steps_into_a_collection =
            segments.get(index + 1).copied() == Some(COLLECTION_STEP);
        at = if steps_into_a_collection {
            match at {
                // A list element, by the 1-based position the walk names it with.
                Value::Array(elements) => {
                    elements.get(segment.parse::<usize>().ok()?.checked_sub(1)?)?
                }
                // A map entry, by the key the caller chose.
                _ => at.get(*segment)?,
            }
        } else {
            at.get(*segment)
                .or_else(|| at.get(upper_camel(segment)))?
        };
    }
    Some(at)
}

/// `value` as a clause quotes the collection a caller submitted: a list as its elements in
/// order and a map as its entries, through the renderings every service of this tree quotes
/// a submitted collection with (`service_common::validation`).
///
/// `None` for anything that is not a collection of scalars.
fn rendered_collection(value: &Value) -> Option<String> {
    match value {
        Value::Array(elements) => {
            let rendered: Vec<String> = elements
                .iter()
                .map(scalar)
                .collect::<Option<Vec<String>>>()?;
            let borrowed: Vec<&str> = rendered.iter().map(String::as_str).collect();
            Some(::service_common::validation::rendered_list(&borrowed))
        }
        Value::Object(entries) => {
            let rendered: Vec<(&str, String)> = entries
                .iter()
                .map(|(key, held)| scalar(held).map(|value| (key.as_str(), value)))
                .collect::<Option<Vec<(&str, String)>>>()?;
            let borrowed: Vec<(&str, &str)> = rendered
                .iter()
                .map(|(key, value)| (*key, value.as_str()))
                .collect();
            Some(::service_common::validation::rendered_map(&borrowed))
        }
        _ => None,
    }
}

/// One element of such a collection as it renders inside the quoted value: a string as its
/// own text, and a number or a boolean as the body spelled it. `None` for an element that is
/// itself a collection or a structure, which has no such rendering.
fn scalar(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        Value::Bool(flag) => Some(flag.to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{rendered_collection, resolve};
    use serde_json::json;

    #[test]
    fn a_walk_path_resolves_to_the_collection_the_body_carried() {
        // Each row is the path the validating walk reports and the collection a clause
        // quotes for it: a top-level list, a list nested under a list element's structure,
        // a map of strings, and a list of structures (which has no text rendering).
        let document = json!({
            "AttributesToGet": ["one", "two"],
            "GlobalSecondaryIndexes": [
                {"Projection": {"NonKeyAttributes": ["p00", "p01"]}},
                {"Projection": {"NonKeyAttributes": ["q"]}},
            ],
            "ExpressionAttributeNames": {"#a": "alpha"},
            "TransactItems": [{"Put": {}}],
        });
        let rows: Vec<(&str, Option<&str>)> = vec![
            ("attributesToGet", Some("[one, two]")),
            (
                "globalSecondaryIndexes.2.member.projection.nonKeyAttributes",
                Some("[q]"),
            ),
            (
                "globalSecondaryIndexes.1.member.projection.nonKeyAttributes",
                Some("[p00, p01]"),
            ),
            ("expressionAttributeNames", Some("{#a=alpha}")),
            ("transactItems", None),
            ("globalSecondaryIndexes.9.member", None),
            ("noSuchMember", None),
        ];
        for (path, expected) in rows {
            let found = resolve(&document, path).and_then(rendered_collection);
            assert_eq!(
                found.as_deref(),
                expected,
                "case {path:?}: expected {expected:?}, got {found:?}"
            );
        }
    }

    #[test]
    fn the_reader_is_bounded_by_its_own_constants_and_not_by_the_request() {
        use super::{SubmittedBody, MAX_QUOTED_VALUE_BYTES, MAX_RESOLVED_PATHS};
        use std::sync::atomic::AtomicUsize;
        use std::sync::OnceLock;

        // A body built in place, so the bounds are read without a request: one member whose
        // value is inside the per-value ceiling and one past it.
        let inside = "v".repeat(MAX_QUOTED_VALUE_BYTES);
        let past = "v".repeat(MAX_QUOTED_VALUE_BYTES + 1);
        let held = SubmittedBody {
            body: json!({"Inside": inside, "Past": past}).to_string().into_bytes(),
            document: OnceLock::new(),
            resolved: AtomicUsize::new(0),
        };
        assert_eq!(
            held.string_at("inside").map(|quoted| quoted.len()),
            Some(MAX_QUOTED_VALUE_BYTES),
            "case a value at the ceiling: expected it quoted whole"
        );
        assert_eq!(
            held.string_at("past"),
            None,
            "case a value past the ceiling: expected the framework's own echo to stand"
        );

        // And the paths ONE refusal may resolve are capped, whatever a caller broke: the
        // reads above have used two of them, so the remaining ones run out.
        let answered = (0..MAX_RESOLVED_PATHS + 4)
            .filter(|_| held.string_at("inside").is_some())
            .count();
        assert_eq!(
            answered,
            MAX_RESOLVED_PATHS - 2,
            "case the path budget: expected {} further paths resolved, got {answered}",
            MAX_RESOLVED_PATHS - 2
        );
    }
}
