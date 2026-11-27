// SPDX-License-Identifier: CC0-1.0

//! WHICH values of one call an SDK MATERIALISES or CONVERTS rather than hand over as the text
//! or the number the wire carries, read from the called service's own model.
//!
//! A JSON protocol carries two kinds of value in a form that is not the value itself:
//!
//! * BYTES. The protocol has no byte type, so a member whose shape is a `blob` travels as the
//!   base64 text of its bytes. An AWS SDK gives the caller the BYTES — `bytes` in Python, a
//!   byte array in JavaScript — because the SDK knows from the service's model which members
//!   are blobs and decodes exactly those.
//! * INSTANTS. A member whose shape is a `timestamp` travels as the seconds since the epoch,
//!   and an AWS SDK gives the caller its language's own instant — a `Date` in JavaScript — for
//!   the same reason: the model says which members are timestamps.
//!
//! So the runtime has to know the same thing: this module reads it out of the model and
//! answers WHERE in a call's document those values sit. Both directions are one question asked
//! of one plan: of an ANSWER, for the values an SDK hands its caller in the caller's own form
//! ([`AnswerPlan::answer_paths`]); of a REQUEST, for the values an SDK puts in the form the
//! protocol carries before it sends them ([`AnswerPlan::input_paths`],
//! [`super::sdk_timestamps`]).
//!
//! The answer is a list of PATHS — each a list of object keys and array indices from the root
//! of the document — rather than a rewritten document, because the seam between the
//! host and a run's engine carries JSON text ([`super::aws_api::Answer`]): the host says which
//! values are bytes and which are instants, and the SDK the guest calls materialises them
//! ([`super::aws_sdk_guest`], [`super::node_sdk_guest`]). Nothing about a value is changed
//! here, and a path is reported only where the document really carries a value.
//!
//! What is KEPT between calls is an [`AnswerPlan`] per kind: the shapes of that service whose
//! values can carry one at all, and which member of each leads to them. The model document
//! itself is parsed once to build that plan and then dropped, so a service whose answers carry
//! bytes in one corner of one operation does not cost this process its whole model for the
//! life of the run. The plan is a GRAPH keyed by shape id rather than a tree, which is what
//! lets a shape that contains itself — a DynamoDB attribute value holds a list of attribute
//! values — be described at all.
//!
//! The walk over an answer is driven by the DOCUMENT: it descends only where the answer
//! carries a value, so a plan whose shapes refer to one another cannot make it recurse without
//! end — the document's own nesting, which `serde_json`'s parse limit bounds, bounds the walk.
//!
//! Two kinds of value are deliberately left out, because neither travels in the form the
//! kind's own leaves do:
//!
//! * a `@streaming` blob, which an SDK hands over as a stream rather than as bytes and which
//!   this runtime reports separately ([`super::aws_api::Answer::streams`]);
//! * a member an answer binds somewhere other than its JSON body in a form that is not the
//!   body's — a header, a prefix of headers, the response code, and a payload that IS the
//!   body's own bytes or text — whose value is the bytes or the text of that binding
//!   ([`super::rest_json`] reads those). A payload that is the body itself is NOT one of them:
//!   that document's values travel in the form a body carries and are part of the plan
//!   ([`carries_the_bodys_form`], which reads WHICH payloads those are from that same seam).
//!
//! The second exception is decided per WALK for the bindings whose form depends on which answer
//! is being read — a header, a prefix of headers, the response code — because it does not hold
//! of an answer this runtime BUILT itself ([`AnswerPlan::built_answer_paths`]): a seam that
//! reads an answer off the output members an SDK CLIENT of this crate reported
//! ([`super::sdk_call`]) states every one of them in the form a JSON body carries, wherever the
//! wire bound it, so an instant of such an answer is an instant whichever binding carried it.

use ::std::collections::{BTreeMap, BTreeSet};
use ::std::sync::OnceLock;

use ::serde_json::{Map, Value};

/// The trait that makes a blob a STREAM rather than bytes an SDK hands over whole.
const STREAMING: &str = "smithy.api#streaming";

/// The name a member travels under in a JSON body, when it is not the member's own name.
const JSON_NAME: &str = "smithy.api#jsonName";

/// The traits that bind a member somewhere other than its JSON body in a form that is not the
/// body's, so what the value reads as is that binding's and not the form a JSON body carries.
///
/// The REST bindings of a REQUEST are here beside an ANSWER's: a timestamp a request binds to
/// a URI label or a query parameter travels in that binding's own form rather than as the
/// seconds a JSON body carries one as, so it is no part of a plan over a request's document
/// either. An answer binds nothing to a label or a query, so one list serves both.
///
/// The binding to the WHOLE body is NOT among them: it is the one binding whose form depends on
/// what the member targets, which [`carries_the_bodys_form`] decides.
const OUTSIDE_THE_BODY: [&str; 6] = [
    "smithy.api#httpHeader",
    "smithy.api#httpPrefixHeaders",
    "smithy.api#httpResponseCode",
    "smithy.api#httpLabel",
    "smithy.api#httpQuery",
    "smithy.api#httpQueryParams",
];

/// The trait that binds a member to the WHOLE body, which is the one binding whose form depends
/// on what the member targets ([`carries_the_bodys_form`]).
const PAYLOAD: &str = "smithy.api#httpPayload";

/// One service's Smithy model, and where the plan read out of it is kept.
///
/// The source is the SAME file that service's own emulator crate is generated from, embedded by
/// [`super::aws_api`], so what is read here cannot drift from the surface that service serves.
/// The plan is built once per process, on the first call to the service.
#[derive(Debug)]
pub struct ServiceModel {
    /// The model document.
    pub source: &'static str,
    /// The plan read out of it.
    pub plan: OnceLock<AnswerPlan>,
}

impl ServiceModel {
    /// The plan, reading it on first use.
    pub fn plan(&self) -> &AnswerPlan {
        self.plan
            .get_or_init(|| AnswerPlan::of(self.source, Materialised::Bytes))
    }

    /// Where the answer of `operation` carries BYTES, in this service.
    pub fn answer_paths(&self, operation: &str, document: &Value) -> Vec<Value> {
        self.plan().answer_paths(operation, document)
    }
}

/// What an SDK MATERIALISES a value of one shape into, rather than hand it over as the text or
/// the number the wire carried it as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Materialised {
    /// The bytes of a `blob`, which a JSON protocol carries as base64 text.
    Bytes,
    /// The instant of a `timestamp`, which a JSON protocol carries as the seconds since the
    /// epoch.
    Instant,
}

impl Materialised {
    /// The type a model declares the shapes of this kind as (`blob`, `timestamp`).
    fn declared(self) -> &'static str {
        match self {
            Self::Bytes => "blob",
            Self::Instant => "timestamp",
        }
    }

    /// The prelude's own shape of that type, which a member may target although no service's
    /// model declares it.
    fn prelude(self) -> &'static str {
        match self {
            Self::Bytes => PRELUDE_BLOB,
            Self::Instant => PRELUDE_TIMESTAMP,
        }
    }

    /// Whether a value an answer carries is one of this kind: the base64 TEXT of a blob's
    /// bytes, and the NUMBER of seconds — or, for a member whose own trait states another
    /// form, the text — a timestamp travels as.
    fn carried_by(self, document: &Value) -> bool {
        match self {
            Self::Bytes => document.is_string(),
            Self::Instant => document.is_number() || document.is_string(),
        }
    }
}

/// Which values of one service's answers an SDK materialises for ONE kind ([`Materialised`]):
/// the output shape of each operation whose answer can carry one, and the shapes that lead to
/// them.
#[derive(Debug, Default)]
pub struct AnswerPlan {
    /// The kind this plan is about, which is what a value reached at one of its leaves is.
    /// `None` for the empty plan of a model that could not be read, which reports nothing.
    kind: Option<Materialised>,
    /// The output shape of each operation whose answer can carry one, by the operation's own
    /// short name — which is the name a client calls it by.
    outputs: BTreeMap<String, String>,
    /// The INPUT shape of each operation whose request can carry one, by the same name.
    ///
    /// What a client sends for such a member is whatever its caller handed it, and the client
    /// is what puts it in the form the protocol carries — so the seam that builds a request
    /// asks WHERE those values sit exactly as the one that reads an answer does
    /// ([`Self::input_paths`]).
    inputs: BTreeMap<String, String>,
    /// Every shape whose values can carry one, by shape id.
    shapes: BTreeMap<String, Carrier>,
}

/// How one shape's values carry the plan's own kind.
#[derive(Debug)]
enum Carrier {
    /// The value IS one: the blob, or the timestamp, itself.
    Leaf,
    /// A structure or a union: these of its members lead to one.
    Members(Vec<Member>),
    /// A list or a set: every entry is this shape.
    Entries(String),
    /// A map: every value is this shape.
    Values(String),
}

/// One member of a structure or a union that leads to a value of the plan's kind.
#[derive(Debug)]
struct Member {
    /// The member's own name, which is what a REST-bound answer's document is keyed by.
    name: String,
    /// The name `@jsonName` gives it on the wire, when it carries that trait.
    wire: Option<String>,
    /// The shape it targets.
    target: String,
    /// Whether the protocol binds it somewhere other than the JSON body
    /// ([`OUTSIDE_THE_BODY`]), which is what decides the walks that report it.
    outside_the_body: bool,
}

/// How one walk over a document reads it.
#[derive(Debug, Clone, Copy)]
struct Reading {
    /// Whether a value in ANY form is one of the plan's kind, which is what tells a REQUEST's
    /// walk from an ANSWER's: an answer carries each value in the one form its protocol
    /// states, so a value in another form is not one of this kind at all, while a request
    /// states one in whatever form its caller handed over.
    any_form: bool,
    /// Whether a member the protocol binds outside the JSON body is reported
    /// ([`AnswerPlan::built_answer_paths`]).
    bound_outside_the_body: bool,
}

impl AnswerPlan {
    /// The plan `source`, a Smithy model document, states for `kind`.
    #[must_use]
    pub fn of(source: &str, kind: Materialised) -> Self {
        let Some(shapes) = ::serde_json::from_str::<Value>(source)
            .ok()
            .and_then(|document| document.get("shapes").cloned())
            .and_then(|shapes| match shapes {
                Value::Object(shapes) => Some(shapes),
                _ => None,
            })
        else {
            return Self::default();
        };
        Self::of_shapes(&shapes, kind)
    }

    /// The same, for a model whose shapes are ALREADY parsed — which is how a seam that reads
    /// several things out of one reading of one model builds it
    /// ([`super::node_sdk_guest::exports`]).
    #[must_use]
    pub fn of_shapes(shapes: &Map<String, Value>, kind: Materialised) -> Self {
        let carrying = carrying_shapes(shapes, kind);
        let mut plan = Self {
            kind: Some(kind),
            ..Self::default()
        };
        for id in &carrying {
            if let Some(carrier) = carrier_of(shapes, id, &carrying, kind) {
                plan.shapes.insert(id.clone(), carrier);
            }
        }
        for (id, shape) in shapes {
            if shape.get("type").and_then(Value::as_str) != Some("operation") {
                continue;
            }
            let Some(name) = id.split('#').next_back().filter(|name| !name.is_empty()) else {
                continue;
            };
            if let Some(input) = shape
                .get("input")
                .and_then(|input| input.get("target"))
                .and_then(Value::as_str)
                .filter(|input| plan.shapes.contains_key(*input))
            {
                plan.inputs.insert(name.to_string(), input.to_string());
            }
            let Some(output) = shape
                .get("output")
                .and_then(|output| output.get("target"))
                .and_then(Value::as_str)
            else {
                continue;
            };
            if !plan.shapes.contains_key(output) {
                continue;
            }
            plan.outputs.insert(name.to_string(), output.to_string());
        }
        plan
    }

    /// Where the answer of `operation` carries a value of this plan's kind: one path per value
    /// the plan types as one and the document really carries.
    ///
    /// `document` is the output document as the guest will read it — the answered body for a
    /// service whose protocol carries the output document as the body, and the document
    /// [`super::rest_json::read`] built for a REST-bound one, which is keyed by the output's
    /// own member names. A key is looked up under the member's own name and under the name
    /// `@jsonName` gives it, so one walk serves both.
    #[must_use]
    pub fn answer_paths(&self, operation: &str, document: &Value) -> Vec<Value> {
        self.answered(operation, document, false)
    }

    /// The same, for an answer this runtime BUILT out of the output members an SDK CLIENT of
    /// this crate reported ([`super::sdk_call`]).
    ///
    /// Such a document is keyed by the output's own member names and states every member in the
    /// form a JSON body carries one — a timestamp as the seconds since the epoch
    /// ([`super::sdk_call::insert_timestamp`]) — WHEREVER the wire bound that member, because
    /// the client has already read the answer's headers and XML into its output. So a member
    /// bound outside the body is part of this walk, while it is no part of the one above, where
    /// the value in the document is that binding's own form.
    #[must_use]
    pub fn built_answer_paths(&self, operation: &str, document: &Value) -> Vec<Value> {
        self.answered(operation, document, true)
    }

    /// Where the answer of `operation` carries a value of this plan's kind, for a walk that
    /// does or does not report the members bound outside the body.
    fn answered(&self, operation: &str, document: &Value, bound: bool) -> Vec<Value> {
        let Some(output) = self.outputs.get(operation) else {
            return Vec::new();
        };
        let mut paths = Vec::new();
        self.walk(
            output,
            document,
            &mut Vec::new(),
            &mut paths,
            Reading {
                any_form: false,
                bound_outside_the_body: bound,
            },
        );
        paths
    }

    /// Where the REQUEST of `operation` carries a value of this plan's kind: one path per value
    /// the plan types as one and the input document really states, WHATEVER form it states it
    /// in.
    ///
    /// Whatever form is the point: a client takes for such a member whatever its caller handed
    /// it — a native instant, a number, the text of one — and the client is what sends it in
    /// the form the called protocol carries, so the seam that builds the request has to be told
    /// where every one of them sits before it can put them in that form
    /// ([`super::sdk_timestamps`]).
    #[must_use]
    pub fn input_paths(&self, operation: &str, document: &Value) -> Vec<Value> {
        let Some(input) = self.inputs.get(operation) else {
            return Vec::new();
        };
        let mut paths = Vec::new();
        self.walk(
            input,
            document,
            &mut Vec::new(),
            &mut paths,
            Reading {
                any_form: true,
                // A value a REQUEST binds outside its body travels in that binding's own
                // form, which the client writes rather than this seam.
                bound_outside_the_body: false,
            },
        );
        paths
    }

    /// Collect the paths of every value of this plan's kind that `document` carries under the
    /// shape `id`.
    fn walk(
        &self,
        id: &str,
        document: &Value,
        path: &mut Vec<Value>,
        paths: &mut Vec<Value>,
        reading: Reading,
    ) {
        match self.shapes.get(id) {
            Some(Carrier::Leaf) => {
                // A blob the document carries as text is the base64 of its bytes, and a
                // timestamp it carries as a number is the seconds since the epoch; anything
                // else is not a value this seam materialises.
                let carried = if reading.any_form {
                    !document.is_null()
                } else {
                    self.kind.is_some_and(|kind| kind.carried_by(document))
                };
                if carried {
                    paths.push(Value::Array(path.clone()));
                }
            }
            Some(Carrier::Members(members)) => {
                let Some(object) = document.as_object() else {
                    return;
                };
                for member in members {
                    // A member the protocol binds OUTSIDE the JSON body is reported only
                    // where the walk reads a document whose values are all in the form a body
                    // carries.
                    if member.outside_the_body && !reading.bound_outside_the_body {
                        continue;
                    }
                    let Some((key, carried)) = member.carried(object) else {
                        continue;
                    };
                    path.push(Value::String(key));
                    self.walk(&member.target, carried, path, paths, reading);
                    path.pop();
                }
            }
            Some(Carrier::Entries(entry)) => {
                let Some(entries) = document.as_array() else {
                    return;
                };
                for (index, carried) in entries.iter().enumerate() {
                    path.push(Value::from(index));
                    self.walk(entry, carried, path, paths, reading);
                    path.pop();
                }
            }
            Some(Carrier::Values(value)) => {
                let Some(entries) = document.as_object() else {
                    return;
                };
                for (key, carried) in entries {
                    path.push(Value::String(key.clone()));
                    self.walk(value, carried, path, paths, reading);
                    path.pop();
                }
            }
            None => {}
        }
    }
}

impl Member {
    /// The value `object` carries for this member, and the key it carries it under: the
    /// member's own name, or the name `@jsonName` gives it on the wire.
    fn carried<'a>(&self, object: &'a Map<String, Value>) -> Option<(String, &'a Value)> {
        if let Some(carried) = object.get(&self.name) {
            return Some((self.name.clone(), carried));
        }
        let wire = self.wire.as_deref()?;
        let carried = object.get(wire)?;
        Some((wire.to_string(), carried))
    }
}

/// Every shape of `shapes` whose values can carry a value of `kind`.
///
/// Computed by passes over the shapes until nothing more is added, rather than by descending
/// each shape: a shape that contains itself — a DynamoDB attribute value holds a list of
/// attribute values — has no bottom to descend to, while the passes simply stop when the set
/// stops growing. The model's shape count bounds how many passes there can be.
fn carrying_shapes(shapes: &Map<String, Value>, kind: Materialised) -> BTreeSet<String> {
    let mut carrying: BTreeSet<String> = BTreeSet::new();
    // The leaves themselves, including the prelude's own shape of that type, which a member
    // may target although no service declares it. A `@streaming` blob is left out for the
    // reason the module docs give; no other shape carries that trait.
    for id in shapes
        .keys()
        .map(String::as_str)
        .chain(::std::iter::once(kind.prelude()))
    {
        if kind_of(shapes, id) == kind.declared() && !is_streaming(shapes, id) {
            carrying.insert(id.to_string());
        }
    }
    loop {
        let mut grew = false;
        for id in shapes.keys() {
            if carrying.contains(id) {
                continue;
            }
            if leads_to_a_leaf(shapes, id, &carrying) {
                carrying.insert(id.clone());
                grew = true;
            }
        }
        if !grew {
            return carrying;
        }
    }
}

/// Whether the shape `id` leads to a value in `carrying`.
fn leads_to_a_leaf(shapes: &Map<String, Value>, id: &str, carrying: &BTreeSet<String>) -> bool {
    let reached = |field: &str| {
        nested(shapes, id, field).is_some_and(|member| carrying.contains(target_of(member)))
    };
    match kind_of(shapes, id) {
        "structure" | "union" => members_of(shapes, id).is_some_and(|members| {
            members
                .values()
                .any(|member| carrying.contains(target_of(member)))
        }),
        "list" | "set" => reached("member"),
        "map" => reached("value"),
        _ => false,
    }
}

/// How the shape `id` carries a value of `kind`, for a shape already known to carry one.
fn carrier_of(
    shapes: &Map<String, Value>,
    id: &str,
    carrying: &BTreeSet<String>,
    kind: Materialised,
) -> Option<Carrier> {
    let declared = kind_of(shapes, id);
    if declared == kind.declared() {
        return Some(Carrier::Leaf);
    }
    match declared {
        "structure" | "union" => {
            let members = members_of(shapes, id)?
                .iter()
                .filter(|(_name, member)| carrying.contains(target_of(member)))
                .filter(|(_name, member)| carries_the_bodys_form(shapes, member))
                .map(|(name, member)| Member {
                    name: name.clone(),
                    wire: member_trait(member, JSON_NAME)
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    target: target_of(member).to_string(),
                    // A member an answer binds outside its JSON body does not carry the form
                    // a JSON body carries, so only a walk over a document whose values are
                    // all in that form reports it ([`Reading`]).
                    outside_the_body: OUTSIDE_THE_BODY
                        .iter()
                        .any(|bound| member_trait(member, bound).is_some()),
                })
                .collect::<Vec<Member>>();
            (!members.is_empty()).then_some(Carrier::Members(members))
        }
        "list" | "set" => Some(Carrier::Entries(
            target_of(nested(shapes, id, "member")?).to_string(),
        )),
        "map" => Some(Carrier::Values(
            target_of(nested(shapes, id, "value")?).to_string(),
        )),
        _ => None,
    }
}

/// Whether the value of `member` in an ANSWER carries the form a JSON body carries, which is
/// what makes it part of the plan at all.
///
/// A member bound to the WHOLE body is the one binding whose form depends on what it targets,
/// and WHICH kinds of payload are the body is read from the seam that reads such an answer back
/// ([`super::rest_json::payload_is_the_body`]) rather than restated here: a payload the model
/// types as a blob or as text IS the body's own bytes or text, while a payload it types as a
/// document IS the body — every value inside it travels in exactly the form a body carries one
/// in, so an SDK materialises those values as it does any other.
///
/// A member bound to a HEADER, to a prefix of headers or to the response code is part of the
/// plan too, carrying [`Member::outside_the_body`]: what form ITS value has depends on which
/// answer is being read, so which walks report it is decided per walk ([`Reading`]) rather than
/// here — an answer read off the protocol's own body carries that binding's own text, while one
/// this runtime BUILT from an SDK client's output states every member in the body's form.
fn carries_the_bodys_form(shapes: &Map<String, Value>, member: &Value) -> bool {
    if member_trait(member, PAYLOAD).is_none() {
        return true;
    }
    super::rest_json::payload_is_the_body(kind_of(shapes, target_of(member)))
}

/// The prelude's own blob shape, which a member may target although no service declares it.
const PRELUDE_BLOB: &str = "smithy.api#Blob";

/// The prelude's own timestamp shape, which members target far more often than a service's own.
const PRELUDE_TIMESTAMP: &str = "smithy.api#Timestamp";

/// The type the shape `id` declares (`structure`, `list`, `blob`, `timestamp`, …), or the
/// prelude's own type for a shape of the prelude, which a service's model does not declare.
fn kind_of(shapes: &Map<String, Value>, id: &str) -> &'static str {
    if let Some(kind) = shapes
        .get(id)
        .and_then(|shape| shape.get("type"))
        .and_then(Value::as_str)
    {
        return match kind {
            "blob" => "blob",
            "timestamp" => "timestamp",
            "structure" => "structure",
            "union" => "union",
            "list" => "list",
            "set" => "set",
            "map" => "map",
            _ => "",
        };
    }
    match id {
        PRELUDE_BLOB => "blob",
        PRELUDE_TIMESTAMP => "timestamp",
        _ => "",
    }
}

/// Whether the shape `shape` describes carries the `@streaming` trait.
///
/// Read from the shape's own traits, so the one answer serves both seams that ask it: the plan
/// below, which leaves a streaming blob out because an SDK hands one over unread, and
/// [`super::rest_json`], which reads from it the form a payload blob reaches the guest in.
pub(super) fn is_streaming_shape(shape: Option<&Value>) -> bool {
    shape
        .and_then(|shape| shape.get("traits"))
        .and_then(Value::as_object)
        .is_some_and(|traits| traits.contains_key(STREAMING))
}

/// Whether the shape `id` carries the `@streaming` trait.
fn is_streaming(shapes: &Map<String, Value>, id: &str) -> bool {
    is_streaming_shape(shapes.get(id))
}

/// The members of the shape `id`, or `None` for a shape declaring none.
fn members_of<'a>(shapes: &'a Map<String, Value>, id: &str) -> Option<&'a Map<String, Value>> {
    shapes
        .get(id)
        .and_then(|shape| shape.get("members"))
        .and_then(Value::as_object)
}

/// The nested shape reference `field` of the shape `id` (`member` of a list, `value` of a
/// map), or `None` for a shape that declares none.
fn nested<'a>(shapes: &'a Map<String, Value>, id: &str, field: &str) -> Option<&'a Value> {
    shapes.get(id).and_then(|shape| shape.get(field))
}

/// The shape a member reference targets.
fn target_of(member: &Value) -> &str {
    member
        .get("target")
        .and_then(Value::as_str)
        .unwrap_or_default()
}

/// The value of the trait `name` on `member`, if it carries it.
fn member_trait<'a>(member: &'a Value, name: &str) -> Option<&'a Value> {
    member
        .get("traits")
        .and_then(Value::as_object)
        .and_then(|traits| traits.get(name))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ::serde_json::json;

    /// The paths a plan reported, rendered and SORTED.
    ///
    /// Which order they come in is not part of the rule — each one names one value, and the
    /// SDK materialises them one by one — and it follows the order the document's own keys
    /// come in, so the rows below compare the SET of paths rather than a sequence.
    fn rendered(paths: &[Value]) -> Vec<String> {
        let mut rendered: Vec<String> = paths
            .iter()
            .map(|path| {
                path.as_array()
                    .map(|steps| {
                        steps
                            .iter()
                            .map(|step| match step {
                                Value::String(key) => key.clone(),
                                other => other.to_string(),
                            })
                            .collect::<Vec<String>>()
                            .join(".")
                    })
                    .unwrap_or_default()
            })
            .collect();
        rendered.sort();
        rendered
    }

    /// The model the rows below are read against: one operation whose output carries a blob
    /// member, a blob inside a list, a blob inside a map, a blob under a `@jsonName`, a
    /// streaming blob, a blob bound to the payload, a STRUCTURE bound to the payload, a text
    /// member, and the timestamp members the instant rows are read off.
    const MODEL: &str = r#"{
      "smithy": "2.0",
      "shapes": {
        "example#Read": {
          "type": "operation",
          "input": {"target": "example#ReadInput"},
          "output": {"target": "example#ReadOutput"}
        },
        "example#Write": {
          "type": "operation",
          "input": {"target": "example#ReadInput"},
          "output": {"target": "example#ReadInput"}
        },
        "example#ReadInput": {"type": "structure", "members": {}},
        "example#ReadOutput": {
          "type": "structure",
          "members": {
            "Value": {"target": "example#Bytes"},
            "Text": {"target": "smithy.api#String"},
            "Named": {
              "target": "example#Bytes",
              "traits": {"smithy.api#jsonName": "named"}
            },
            "Entries": {"target": "example#Entries"},
            "Held": {"target": "example#Held"},
            "Nested": {"target": "example#Nested"},
            "Stream": {"target": "example#Streamed"},
            "Payload": {
              "target": "example#Bytes",
              "traits": {"smithy.api#httpPayload": {}}
            },
            "Document": {
              "target": "example#Nested",
              "traits": {"smithy.api#httpPayload": {}}
            },
            "At": {"target": "smithy.api#Timestamp"},
            "Count": {"target": "smithy.api#Long"},
            "Stamps": {"target": "example#Stamps"},
            "Header": {
              "target": "example#Instant",
              "traits": {"smithy.api#httpHeader": "X-At"}
            }
          }
        },
        "example#Nested": {
          "type": "structure",
          "members": {
            "Inner": {"target": "example#Bytes"},
            "InnerAt": {"target": "example#Instant"}
          }
        },
        "example#Entries": {"type": "list", "member": {"target": "example#Bytes"}},
        "example#Stamps": {"type": "list", "member": {"target": "example#Instant"}},
        "example#Held": {
          "type": "map",
          "key": {"target": "smithy.api#String"},
          "value": {"target": "example#Bytes"}
        },
        "example#Bytes": {"type": "blob"},
        "example#Instant": {"type": "timestamp"},
        "example#Streamed": {"type": "blob", "traits": {"smithy.api#streaming": {}}}
      }
    }"#;

    /// The plan of one kind, read off the model above.
    fn plan(kind: Materialised) -> AnswerPlan {
        AnswerPlan::of(MODEL, kind)
    }

    /// Every blob value an answer carries is reported by its path, at whatever depth and
    /// under whichever name the wire carried it — and a value that is not one is not.
    #[test]
    fn every_blob_value_of_an_answer_is_reported_by_its_path() {
        let plan = plan(Materialised::Bytes);
        // Each row: the answer document, and the paths its blob values sit at.
        let cases: &[(&str, Value, Value)] = &[
            (
                "a blob member of the output structure",
                json!({"Value": "AAE=", "Text": "not bytes"}),
                json!([["Value"]]),
            ),
            (
                "a blob member under the name @jsonName gives it",
                json!({"named": "AAE="}),
                json!([["named"]]),
            ),
            (
                "a blob member under its own name, which @jsonName renames on the wire",
                json!({"Named": "AAE="}),
                json!([["Named"]]),
            ),
            (
                "a blob inside a structure the output carries",
                json!({"Nested": {"Inner": "AAE="}}),
                json!([["Nested", "Inner"]]),
            ),
            (
                "a blob at each index of a list",
                json!({"Entries": ["AAE=", "Ag=="]}),
                json!([["Entries", 0], ["Entries", 1]]),
            ),
            (
                "a blob under each key of a map",
                json!({"Held": {"first": "AAE="}}),
                json!([["Held", "first"]]),
            ),
            (
                "a streaming blob, which is handed over as a stream",
                json!({"Stream": "AAE="}),
                json!([]),
            ),
            (
                "a blob bound to the answer's payload, which is its own bytes",
                json!({"Payload": "not base64 at all"}),
                json!([]),
            ),
            (
                "a blob inside a STRUCTURE bound to the answer's payload, which is the body",
                json!({"Document": {"Inner": "AAE="}}),
                json!([["Document", "Inner"]]),
            ),
            (
                "several blob values at once",
                json!({"Value": "AAE=", "Nested": {"Inner": "Ag=="}}),
                json!([["Nested", "Inner"], ["Value"]]),
            ),
            (
                "an answer carrying none of them",
                json!({"Text": "AAE="}),
                json!([]),
            ),
        ];
        for (case, document, expected) in cases {
            let paths = plan.answer_paths("Read", document);
            let expected = rendered(expected.as_array().expect("the expectation is a list"));
            assert_eq!(
                rendered(&paths),
                expected,
                "case {case:?}: expected the blob paths {expected:?}, got {paths:?}"
            );
        }
    }

    /// An operation the model does not bind, and one whose output carries no blob at all,
    /// report no path rather than guessing at the answer's shape.
    #[test]
    fn an_operation_whose_answer_carries_no_bytes_reports_no_path() {
        let plan = plan(Materialised::Bytes);
        let document = json!({"Value": "AAE=", "Entries": ["AAE="]});
        for operation in ["Write", "ReadOutput", "Describe", ""] {
            let paths = plan.answer_paths(operation, &document);
            assert!(
                paths.is_empty(),
                "case {operation:?}: expected no blob path, got {paths:?}"
            );
        }
    }

    /// The plan read out of a model this crate really embeds, for an operation the issue that
    /// prompted this module did not cite: Secrets Manager's `GetSecretValue` carries the
    /// secret's bytes in `SecretBinary` (`SecretBinaryType`, a blob) and its text in
    /// `SecretString`, so one is reported and the other is not.
    #[test]
    fn a_blob_member_of_an_embedded_services_model_is_reported() {
        let plan = AnswerPlan::of(super::super::aws_api::SECRETSMANAGER_MODEL, Materialised::Bytes);
        let paths = plan.answer_paths(
            "GetSecretValue",
            &json!({"SecretBinary": "AAE=", "SecretString": "text", "Name": "a-secret"}),
        );
        assert_eq!(
            rendered(&paths),
            vec!["SecretBinary".to_string()],
            "expected the blob member of GetSecretValue's answer, got {paths:?}"
        );
    }

    /// The same, for the case that makes the DEPTH of the walk the point: a DynamoDB attribute
    /// value carries bytes in `B` and in `BS`, and an attribute value holds lists (`L`) and
    /// maps (`M`) of attribute values, so the bytes of an item sit at whatever depth the item
    /// put them.
    #[test]
    fn a_blob_nested_in_an_embedded_services_model_is_reported_at_its_depth() {
        let plan = AnswerPlan::of(super::super::aws_api::DYNAMODB_MODEL, Materialised::Bytes);
        let paths = plan.answer_paths(
            "GetItem",
            &json!({
                "Item": {
                    "pk": {"S": "text"},
                    "data": {"B": "AAE="},
                    "set": {"BS": ["AAE=", "Ag=="]},
                    "list": {"L": [{"B": "AAE="}]},
                    "map": {"M": {"inner": {"B": "AAE="}}},
                },
            }),
        );
        assert_eq!(
            rendered(&paths),
            vec![
                "Item.data.B".to_string(),
                "Item.list.L.0.B".to_string(),
                "Item.map.M.inner.B".to_string(),
                "Item.set.BS.0".to_string(),
                "Item.set.BS.1".to_string(),
            ],
            "expected every blob of the item at its own depth, got {paths:?}"
        );
    }

    /// Every value an answer carries for a member the model types as a TIMESTAMP is reported by
    /// its path, at whatever depth — and a number the model types as anything else is not.
    #[test]
    fn every_timestamp_value_of_an_answer_is_reported_by_its_path() {
        let plan = plan(Materialised::Instant);
        // Each row: the answer document, and the paths its timestamp values sit at.
        let cases: &[(&str, Value, Value)] = &[
            (
                "a timestamp member of the output structure",
                json!({"At": 1_761_000_000, "Text": "not an instant"}),
                json!([["At"]]),
            ),
            (
                "a timestamp carrying a fraction of a second",
                json!({"At": 1_761_000_000.5_f64}),
                json!([["At"]]),
            ),
            (
                "a timestamp inside a structure the output carries",
                json!({"Nested": {"InnerAt": 0}}),
                json!([["Nested", "InnerAt"]]),
            ),
            (
                "a timestamp at each index of a list",
                json!({"Stamps": [1, 2]}),
                json!([["Stamps", 0], ["Stamps", 1]]),
            ),
            (
                "a timestamp beside a blob and a long, each at its own path",
                json!({"At": 1, "Value": "AAE=", "Count": 1_761_000_000_000_i64}),
                json!([["At"]]),
            ),
            (
                "a member the model types as a long, which is not an instant",
                json!({"Count": 1_761_000_000_000_i64}),
                json!([]),
            ),
            (
                "a timestamp the answer binds to a header, whose form is that binding's",
                json!({"Header": 1}),
                json!([]),
            ),
            (
                "a timestamp inside a STRUCTURE bound to the answer's payload, which is the \
                 body",
                json!({"Document": {"InnerAt": 1_761_000_000}}),
                json!([["Document", "InnerAt"]]),
            ),
            (
                "an answer carrying none of them",
                json!({"Text": "1761000000", "Value": "AAE="}),
                json!([]),
            ),
        ];
        for (case, document, expected) in cases {
            let paths = plan.answer_paths("Read", document);
            let expected = rendered(expected.as_array().expect("the expectation is a list"));
            assert_eq!(
                rendered(&paths),
                expected,
                "case {case:?}: expected the timestamp paths {expected:?}, got {paths:?}"
            );
        }
    }

    /// The instant plan read out of a model this crate really embeds, for operations the issue
    /// that prompted it did not cite: DynamoDB's `ListBackups` carries one timestamp per backup
    /// summary, at the depth the listing puts it, and `GetItem` carries none at all — an item's
    /// attribute values have no timestamp type.
    #[test]
    fn a_timestamp_member_of_an_embedded_services_model_is_reported_at_its_depth() {
        let plan = AnswerPlan::of(super::super::aws_api::DYNAMODB_MODEL, Materialised::Instant);
        let paths = plan.answer_paths(
            "ListBackups",
            &json!({
                "BackupSummaries": [
                    {
                        "TableName": "items",
                        "BackupCreationDateTime": 1_761_000_000,
                        "BackupExpiryDateTime": 1_761_000_001,
                    },
                    {"TableName": "other", "BackupCreationDateTime": 1_761_000_002},
                ],
            }),
        );
        assert_eq!(
            rendered(&paths),
            vec![
                "BackupSummaries.0.BackupCreationDateTime".to_string(),
                "BackupSummaries.0.BackupExpiryDateTime".to_string(),
                "BackupSummaries.1.BackupCreationDateTime".to_string(),
            ],
            "expected every timestamp of the listing at its own depth, got {paths:?}"
        );
        let item = plan.answer_paths("GetItem", &json!({"Item": {"at": {"N": "1761000000"}}}));
        assert!(
            item.is_empty(),
            "expected no instant in an item's attribute values, got {item:?}"
        );
    }

    /// The instant plan read out of the CloudFront model this crate embeds, whose answers bind
    /// their one output member to the response PAYLOAD: that payload is a STRUCTURE, so it is
    /// the body, and the instants inside it are reported at their own depth — one for the
    /// invalidation a create or a read answers, and one per summary of a listing's page.
    #[test]
    fn a_timestamp_inside_an_answers_structure_payload_is_reported_at_its_depth() {
        let plan = AnswerPlan::of(super::super::aws_api::CLOUDFRONT_MODEL, Materialised::Instant);
        for operation in ["CreateInvalidation", "GetInvalidation"] {
            let paths = plan.answer_paths(
                operation,
                &json!({
                    "Invalidation": {
                        "Id": "IDEXAMPLE",
                        "Status": "InProgress",
                        "CreateTime": 1_761_000_000,
                        "InvalidationBatch": {"CallerReference": "a-reference"},
                    },
                }),
            );
            assert_eq!(
                rendered(&paths),
                vec!["Invalidation.CreateTime".to_string()],
                "case {operation:?}: expected the invalidation's own creation instant, got \
                 {paths:?}"
            );
        }
        let listed = plan.answer_paths(
            "ListInvalidations",
            &json!({
                "InvalidationList": {
                    "Items": [
                        {"Id": "A", "Status": "Completed", "CreateTime": 1_761_000_000},
                        {"Id": "B", "Status": "Completed", "CreateTime": 1_761_000_001},
                    ],
                },
            }),
        );
        assert_eq!(
            rendered(&listed),
            vec![
                "InvalidationList.Items.0.CreateTime".to_string(),
                "InvalidationList.Items.1.CreateTime".to_string(),
            ],
            "expected each summary's own creation instant, got {listed:?}"
        );
    }
}
