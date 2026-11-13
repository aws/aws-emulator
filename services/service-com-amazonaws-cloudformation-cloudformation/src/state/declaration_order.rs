// SPDX-License-Identifier: CC0-1.0

//! The order a template BODY declares the entries of one of its sections in.
//!
//! A template section is a mapping, and what a client reads back about the template follows
//! the order the body states that mapping in: the logical ids of each
//! `ResourceIdentifierSummary` `GetTemplateSummary` reports are "the logical IDs of the target
//! resources … as defined in the import template", and the summaries themselves follow the
//! first resource declared with each type. So the DECLARED order is part of the answer, and
//! this module is where it is read.
//!
//! It is read from the BODY rather than from the parsed document on purpose: a template is
//! read into a `serde_json::Value`, whose object members are held sorted
//! ([`super::template::document`]), so the declarations' own order is gone by the time any
//! rule sees the document. Nothing here parses the template a second time into a tree — each
//! reader below walks the body and keeps only the KEYS of the one section asked about, which
//! is a list of names bounded by the per-template quota on that section.
//!
//! Both spellings of a body are read, each by the reader that spells it: `serde_json` for the
//! JSON spelling and the YAML event stream for the other, exactly as
//! [`super::template::document`] chooses between them. An order this module cannot read —
//! a body neither reader reads, or a section some other reader built (the resources a
//! transform's expansion contributed, which the body never declared) — is simply not known
//! for those entries, and the caller keeps the order it already had for them.

use ::serde::de::{DeserializeSeed, Deserializer, IgnoredAny, MapAccess, Visitor};
use ::std::collections::BTreeMap;
use ::std::fmt;

/// The `Resources` section, whose entries are a template's resource declarations.
pub const RESOURCES: &str = "Resources";

/// The `Mappings` section, whose entries are the maps `Fn::FindInMap` reads.
pub const MAPPINGS: &str = "Mappings";

/// The keys of `section` in the order `body` declares them, and the EMPTY list where that
/// order cannot be read (see the module docs).
pub fn section_keys(body: &str, section: &str) -> Vec<String> {
    // The JSON spelling first, for the reason the document reader tries it first: YAML 1.2 is
    // a superset of JSON, and `serde_json` reads the JSON spelling exactly. A body it reads is
    // answered from that reading alone — a JSON body stating no such section declares nothing
    // in it, which is the empty order rather than a reason to read the body again.
    if let Some(keys) = json_section_keys(body, section) {
        return keys;
    }
    yaml_section_keys(body, section)
}

/// `declared` re-ordered as `body` declares its `section` entries, each item placed by the
/// `name` it is declared under.
///
/// An item whose name the body does not declare — one some other reader contributed — keeps
/// its position relative to the other such items, after the declared ones: the order this
/// module knows is the body's, and nothing here invents one for a name the body never stated.
pub fn as_declared<'a, T>(
    body: &str,
    section: &str,
    declared: &'a [T],
    name: impl Fn(&T) -> &str,
) -> Vec<&'a T> {
    let keys = section_keys(body, section);
    let position: BTreeMap<&str, usize> = keys
        .iter()
        .enumerate()
        .map(|(at, key)| (key.as_str(), at))
        .collect();
    let mut ordered: Vec<&T> = declared.iter().collect();
    // Stable, so items the body does not declare keep the order they came in.
    ordered.sort_by_key(|item| position.get(name(item)).copied().unwrap_or(usize::MAX));
    ordered
}

/// The keys of `section` as the JSON spelling of `body` states them, or `None` for a body
/// `serde_json` does not read as one JSON object (the YAML reader is what answers for those).
fn json_section_keys(body: &str, section: &str) -> Option<Vec<String>> {
    let mut deserializer = ::serde_json::Deserializer::from_str(body);
    let keys = SectionKeys(section).deserialize(&mut deserializer).ok()?;
    deserializer.end().ok()?;
    Some(keys)
}

/// Reads ONE section's keys out of a JSON document, skipping every other member rather than
/// building a value for it.
struct SectionKeys<'a>(&'a str);

impl<'de> DeserializeSeed<'de> for SectionKeys<'_> {
    type Value = Vec<String>;

    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<Self::Value, D::Error> {
        deserializer.deserialize_map(self)
    }
}

impl<'de> Visitor<'de> for SectionKeys<'_> {
    type Value = Vec<String>;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a template document")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut document: A) -> Result<Self::Value, A::Error> {
        let mut keys = Vec::new();
        while let Some(member) = document.next_key::<String>()? {
            if member == self.0 {
                keys = document.next_value::<KeyOrder>()?.0;
            } else {
                document.next_value::<IgnoredAny>()?;
            }
        }
        Ok(keys)
    }
}

/// The keys of one JSON object, in the order the document states them.
struct KeyOrder(Vec<String>);

impl<'de> ::serde::de::Deserialize<'de> for KeyOrder {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Keys;

        impl<'de> Visitor<'de> for Keys {
            type Value = KeyOrder;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a template section")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut section: A) -> Result<KeyOrder, A::Error> {
                let mut keys = Vec::new();
                while let Some(key) = section.next_key::<String>()? {
                    section.next_value::<IgnoredAny>()?;
                    keys.push(key);
                }
                Ok(KeyOrder(keys))
            }
        }

        deserializer.deserialize_map(Keys)
    }
}

/// The keys of `section` as the YAML spelling of `body` states them, and the empty list for a
/// body this reader will not walk.
///
/// Built on the parser's EVENT stream, which is what carries the order; the loaded tree is
/// sorted. The body is held to the same pre-flight bounds the document reader holds a YAML
/// body to ([`super::template::guard_yaml_body`]): an `EventReceiver` cannot abort a load, so
/// what bounds the work is refusing the body before the parser runs.
fn yaml_section_keys(body: &str, section: &str) -> Vec<String> {
    /// One open collection: a sequence, or a mapping and the key it has read but not yet
    /// paired with a value.
    enum Frame {
        Sequence,
        Mapping { pending_key: Option<String> },
    }

    struct Order<'a> {
        section: &'a str,
        open: Vec<Frame>,
        /// Which open frame IS the section's own mapping, once one has been entered.
        section_frame: Option<usize>,
        keys: Vec<String>,
    }

    impl Order<'_> {
        /// The key the node now being read is the value of, consuming it: a node inside a
        /// sequence is the value of no key.
        fn key_being_filled(&mut self) -> Option<String> {
            match self.open.last_mut() {
                Some(Frame::Mapping { pending_key }) => pending_key.take(),
                _ => None,
            }
        }

        /// A collection opens as the value of `key`: the section's own mapping is the one the
        /// ROOT mapping states under the section's name.
        fn open_frame(&mut self, frame: Frame, key: Option<String>) {
            if self.open.len() == 1 && key.as_deref() == Some(self.section) {
                self.section_frame = Some(self.open.len());
            }
            self.open.push(frame);
        }
    }

    impl ::yaml_rust2::parser::EventReceiver for Order<'_> {
        fn on_event(&mut self, event: ::yaml_rust2::parser::Event) {
            use ::yaml_rust2::parser::Event;
            match event {
                Event::Scalar(value, ..) => {
                    let at = self.open.len().saturating_sub(1);
                    // A scalar where no key is pending IS the next key of this mapping;
                    // anything else is a value, which consumes the key it fills.
                    let expecting_key =
                        matches!(self.open.last(), Some(Frame::Mapping { pending_key: None }));
                    if !expecting_key {
                        self.key_being_filled();
                        return;
                    }
                    if self.section_frame == Some(at)
                        && self.keys.len() < super::template::MAX_TEMPLATE_NODES
                    {
                        self.keys.push(value.clone());
                    }
                    if let Some(Frame::Mapping { pending_key }) = self.open.last_mut() {
                        *pending_key = Some(value);
                    }
                }
                Event::SequenceStart(..) => {
                    let key = self.key_being_filled();
                    self.open_frame(Frame::Sequence, key);
                }
                Event::MappingStart(..) => {
                    let key = self.key_being_filled();
                    self.open_frame(Frame::Mapping { pending_key: None }, key);
                }
                Event::SequenceEnd | Event::MappingEnd => {
                    if self.section_frame == Some(self.open.len().saturating_sub(1)) {
                        self.section_frame = None;
                    }
                    self.open.pop();
                }
                _ => {}
            }
        }
    }

    if super::template::guard_yaml_body(body, super::TEMPLATE_MAX_BYTES).is_none() {
        return Vec::new();
    }
    let mut order = Order {
        section,
        open: Vec::new(),
        section_frame: None,
        keys: Vec::new(),
    };
    if ::yaml_rust2::parser::Parser::new_from_str(body)
        .load(&mut order, true)
        .is_err()
    {
        return Vec::new();
    }
    order.keys
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The declared order of one section, over the spellings a template body comes in: JSON
    /// and YAML, the section stated before and after the other sections, nested mappings and
    /// sequences inside the declarations, and a body that declares no such section.
    #[test]
    fn the_declared_order_is_the_order_the_body_states_the_section_in() {
        let rows: &[(&str, &str, &[&str])] = &[
            (
                "JSON, resources stated out of alphabetical order",
                r#"{"Resources":{"Zebra":{"Type":"AWS::S3::Bucket"},
                     "Alpha":{"Type":"AWS::SNS::Topic"},
                     "Mid":{"Type":"AWS::SQS::Queue"}}}"#,
                &["Zebra", "Alpha", "Mid"],
            ),
            (
                "JSON, other sections before and after the Resources section",
                r#"{"Parameters":{"P":{"Type":"String"}},
                     "Resources":{"Second":{"Type":"AWS::S3::Bucket"},
                       "First":{"Type":"AWS::SNS::Topic"}},
                     "Outputs":{"O":{"Value":"v"}}}"#,
                &["Second", "First"],
            ),
            (
                "JSON, declarations carrying nested maps and lists",
                r#"{"Resources":{"Vpc":{"Type":"AWS::EC2::VPC",
                       "Properties":{"Tags":[{"Key":"a","Value":"b"}],
                                     "Nested":{"Deep":{"Deeper":1}}}},
                     "Bucket":{"Type":"AWS::S3::Bucket"}}}"#,
                &["Vpc", "Bucket"],
            ),
            (
                "YAML block style",
                "Resources:\n  Zebra:\n    Type: AWS::S3::Bucket\n  \
                 Alpha:\n    Type: AWS::SNS::Topic\n",
                &["Zebra", "Alpha"],
            ),
            (
                "YAML with short-form intrinsics and a nested sequence",
                "Parameters:\n  Name:\n    Type: String\n\
                 Resources:\n  Topic:\n    Type: AWS::SNS::Topic\n    \
                 Properties:\n      TopicName: !Ref Name\n      \
                 Tags:\n        - Key: a\n          Value: b\n  \
                 Bucket:\n    Type: AWS::S3::Bucket\n",
                &["Topic", "Bucket"],
            ),
            (
                "a body declaring no Resources section at all",
                r#"{"Parameters":{"P":{"Type":"String"}}}"#,
                &[],
            ),
            (
                "a body that is neither JSON nor YAML",
                "{\"Resources\": ",
                &[],
            ),
        ];
        for (case, body, expected) in rows {
            let keys = section_keys(body, RESOURCES);
            assert_eq!(
                keys, *expected,
                "case {case:?}: expected {expected:?}, got {keys:?}"
            );
        }
    }

    /// Items are re-ordered by the order the body declares their names in, and a name the
    /// body does not declare keeps its place after the ones it does.
    #[test]
    fn items_are_ordered_by_the_names_the_body_declares() {
        let body = r#"{"Resources":{"Second":{"Type":"AWS::S3::Bucket"},
             "First":{"Type":"AWS::SNS::Topic"}}}"#;
        let declared = ["Expanded", "First", "Second"];
        let ordered: Vec<&&str> = as_declared(body, RESOURCES, &declared, |name| name);
        assert_eq!(
            ordered,
            vec![&"Second", &"First", &"Expanded"],
            "case \"one name the body never declared\": got {ordered:?}"
        );
    }
}
