// SPDX-License-Identifier: CC0-1.0

//! The XML answer of a REST-XML service, read as a tree of elements.
//!
//! One service this crate calls answers XML rather than JSON: Amazon S3, whose
//! protocol is `restXml` — the dataset a `Map` state's `ItemReader` names lives there
//! ([`super::item_reader`]), and the listing that reader reads is an XML document, as
//! is the error document a refused read carries. Nothing else in this crate reads XML,
//! so this is the ONE place that does.
//!
//! The parse is the maintained pull parser's ([`::quick_xml`]) rather than anything
//! hand-written: this module adds only the shape the callers want — an element's name,
//! its text and its children — and the ceilings that confine a document a service
//! answered. Every value in the tree is the ANSWER's own; nothing is defaulted.

/// One element of an XML document: its local name, the text it carries directly, and
/// the elements nested inside it, in document order.
#[derive(Debug, Default, Clone)]
pub struct Element {
    /// The element's LOCAL name — the part after any namespace prefix — because an
    /// answer may carry a prefix and the member it stands for is the same either way.
    pub name: String,
    /// The character data directly inside the element, with the XML escaping undone and
    /// the document's own indentation around it dropped ([`close`]) — the whitespace
    /// INSIDE it is the answer's own and is kept, including the whitespace beside an
    /// entity reference, which the parser reports as an event of its own.
    pub text: String,
    /// The elements nested inside it, in document order.
    pub children: Vec<Element>,
}

impl Element {
    /// The first child element named `name`.
    #[must_use]
    pub fn child(&self, name: &str) -> Option<&Element> {
        self.children.iter().find(|child| child.name == name)
    }

    /// Every child element named `name`, in document order — which is what a list the
    /// protocol flattens (`ListObjectsV2`'s `Contents`) is carried as.
    pub fn each<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Element> {
        self.children.iter().filter(move |child| child.name == name)
    }

    /// The text of the first child named `name`; `None` when the element carries no
    /// such child, which is how an answer reports a member it did not carry.
    #[must_use]
    pub fn text_of(&self, name: &str) -> Option<&str> {
        self.child(name).map(|child| child.text.as_str())
    }
}

/// How deeply nested an element this reads: past it the document is refused rather
/// than built, so a document a service answered cannot be read into the thread's
/// stack. Far deeper than any answer this crate reads, whose deepest nesting is a
/// listing's `Contents` entries.
const MAX_DEPTH: usize = 32;

/// How many elements one document may hold, which bounds what one answer costs to
/// read. A listing page carries at most a thousand entries of some five elements
/// each, so this is several pages' worth.
const MAX_ELEMENTS: usize = 100_000;

/// `xml` as its root element, or why it could not be read.
///
/// The `Err` says what about the document was not readable; it never claims a value
/// the document did not carry.
pub fn read(xml: &[u8]) -> Result<Element, String> {
    use ::quick_xml::events::Event;

    // Every text event is read as the document wrote it: trimming each event on its own
    // would drop the whitespace that stands beside an ENTITY REFERENCE, which the parser
    // reports as an event of its own, so a sentence carrying an escaped character would
    // lose the spaces around it. The indentation a pretty-printed document puts around an
    // element's text is dropped once the element is complete ([`close`]) instead.
    let mut reader = ::quick_xml::Reader::from_reader(xml);
    reader.config_mut().trim_text(false);
    let mut open: Vec<Element> = Vec::new();
    let mut root: Option<Element> = None;
    let mut elements = 0usize;
    loop {
        match reader.read_event() {
            Ok(Event::Eof) => break,
            Ok(Event::Start(start)) => {
                elements += 1;
                if elements > MAX_ELEMENTS {
                    return Err(format!(
                        "the answer holds more than the {MAX_ELEMENTS} XML elements this \
                         emulator reads"
                    ));
                }
                if open.len() >= MAX_DEPTH {
                    return Err(format!(
                        "the answer nests XML elements more than the {MAX_DEPTH} levels deep \
                         this emulator reads"
                    ));
                }
                open.push(Element {
                    name: local_name(start.name().as_ref()),
                    ..Element::default()
                });
            }
            Ok(Event::Empty(empty)) => {
                elements += 1;
                if elements > MAX_ELEMENTS {
                    return Err(format!(
                        "the answer holds more than the {MAX_ELEMENTS} XML elements this \
                         emulator reads"
                    ));
                }
                let element = Element {
                    name: local_name(empty.name().as_ref()),
                    ..Element::default()
                };
                close(element, &mut open, &mut root);
            }
            Ok(Event::Text(text)) => {
                let content = text
                    .xml10_content()
                    .map_err(|error| error.to_string())?;
                if let Some(element) = open.last_mut() {
                    element.text.push_str(&content);
                }
            }
            // An entity reference is its own event, so the character it stands for is
            // resolved by the parser's own escaping rules rather than by a table here.
            Ok(Event::GeneralRef(reference)) => {
                let name = reference.decode().map_err(|error| error.to_string())?;
                let resolved = ::quick_xml::escape::unescape(&format!("&{name};"))
                    .map_err(|error| error.to_string())?
                    .into_owned();
                if let Some(element) = open.last_mut() {
                    element.text.push_str(&resolved);
                }
            }
            Ok(Event::CData(data)) => {
                let text = String::from_utf8_lossy(data.as_ref()).to_string();
                if let Some(element) = open.last_mut() {
                    element.text.push_str(&text);
                }
            }
            Ok(Event::End(_)) => {
                let element = open
                    .pop()
                    .ok_or_else(|| "the answer closes an XML element it never opened".to_string())?;
                close(element, &mut open, &mut root);
            }
            Ok(_) => {}
            Err(error) => return Err(error.to_string()),
        }
    }
    root.ok_or_else(|| "the answer carries no XML element at all".to_string())
}

/// Place a finished element: inside the element that encloses it, else as the
/// document's root.
///
/// The text it accumulated is trimmed HERE, once, rather than event by event: that drops
/// the indentation a pretty-printed document writes around an element's character data
/// (and leaves an element whose text is only that indentation carrying none) while
/// keeping every space inside the data itself.
fn close(mut element: Element, open: &mut [Element], root: &mut Option<Element>) {
    if element.text.trim().len() != element.text.len() {
        element.text = element.text.trim().to_string();
    }
    match open.last_mut() {
        Some(parent) => parent.children.push(element),
        None => *root = Some(element),
    }
}

/// The LOCAL part of an element's name: what follows the last `:`, since a prefix
/// names a namespace rather than the member.
fn local_name(name: &[u8]) -> String {
    let name = String::from_utf8_lossy(name).to_string();
    match name.rsplit_once(':') {
        Some((_prefix, local)) => local.to_string(),
        None => name,
    }
}

/// The document `xml` carries, as the flat object its root element's text-only
/// children make: what an XML refusal is read as, so one reading of an error answer
/// serves both protocols.
///
/// S3's model sets `noErrorWrapping`, so a refusal's `Code` and `Message` are the
/// root's own children; an `awsQuery` refusal wraps the same pair in an `Error`
/// element, which is read through to the same object. `None` for a body that is not an
/// XML document at all.
#[must_use]
pub fn as_document(xml: &[u8]) -> Option<::serde_json::Value> {
    let root = read(xml).ok()?;
    let carried = root.child(ERROR_ELEMENT).unwrap_or(&root);
    let mut members = ::serde_json::Map::new();
    for child in &carried.children {
        if child.children.is_empty() {
            members.insert(
                child.name.clone(),
                ::serde_json::Value::String(child.text.clone()),
            );
        }
    }
    Some(::serde_json::Value::Object(members))
}

/// The element an `awsQuery` refusal wraps its `Code` and `Message` in.
const ERROR_ELEMENT: &str = "Error";

/// `element` as the JSON document its tree carries: an element with no children is the
/// TEXT it holds, one with children is the object of those children, and a name the
/// element repeats is the array of what each of them holds.
///
/// Every value is a STRING, because that is what an XML document carries: which of
/// them the invoked operation models as a number or a boolean is a fact of that
/// operation's own model, which this crate cannot read for another service, so nothing
/// here coerces one.
#[must_use]
pub fn as_value(element: &Element) -> ::serde_json::Value {
    if element.children.is_empty() {
        return ::serde_json::Value::String(element.text.clone());
    }
    let mut members = ::serde_json::Map::new();
    for child in &element.children {
        let value = as_value(child);
        match members.get_mut(&child.name) {
            // The second element of a repeated name turns what was read into the array
            // the protocol means by repeating it.
            Some(::serde_json::Value::Array(values)) => values.push(value),
            Some(held) => {
                let first = held.clone();
                members.insert(
                    child.name.clone(),
                    ::serde_json::Value::Array(vec![first, value]),
                );
            }
            None => {
                members.insert(child.name.clone(), value);
            }
        }
    }
    ::serde_json::Value::Object(members)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_element_tree_carries_each_child_in_document_order() {
        let xml = br#"<ListBucketResult xmlns="http://s3.amazonaws.com/doc/2006-03-01/">
            <Name>bucket</Name>
            <IsTruncated>false</IsTruncated>
            <Contents><Key>a</Key><Size>1</Size></Contents>
            <Contents><Key>b</Key><Size>2</Size></Contents>
        </ListBucketResult>"#;
        let root = read(xml).expect("the listing reads");
        assert_eq!(root.name, "ListBucketResult", "the root is the result element");
        assert_eq!(
            root.text_of("Name"),
            Some("bucket"),
            "the bucket is the answer's own Name, got {:?}",
            root.text_of("Name")
        );
        let keys: Vec<&str> = root
            .each("Contents")
            .filter_map(|entry| entry.text_of("Key"))
            .collect();
        assert_eq!(
            keys,
            vec!["a", "b"],
            "every Contents entry is read in document order, got {keys:?}"
        );
    }

    #[test]
    fn an_empty_element_carries_no_text() {
        let root = read(b"<ListBucketResult><Prefix/></ListBucketResult>")
            .expect("the listing reads");
        assert_eq!(
            root.text_of("Prefix"),
            Some(""),
            "a self-closing element carries the empty text, got {:?}",
            root.text_of("Prefix")
        );
    }

    #[test]
    fn a_refusal_document_is_read_as_its_code_and_message() {
        let document = as_document(
            b"<Error><Code>NoSuchKey</Code><Message>The specified key does not exist.</Message>\
              </Error>",
        )
        .expect("the refusal reads");
        assert_eq!(
            document.get("Code").and_then(::serde_json::Value::as_str),
            Some("NoSuchKey"),
            "the code is the document's own, got {document:?}"
        );
        assert_eq!(
            document
                .get("Message")
                .and_then(::serde_json::Value::as_str),
            Some("The specified key does not exist."),
            "the message is the document's own, got {document:?}"
        );
    }

    #[test]
    fn an_elements_text_keeps_the_whitespace_beside_an_escaped_character() {
        // (case, the document, the text the element carries)
        let rows: &[(&str, &[u8], &str)] = &[
            (
                "an escaped apostrophe mid-sentence",
                b"<Message>Could not cast message attribute &apos;count&apos; value to \
                  number.</Message>",
                "Could not cast message attribute 'count' value to number.",
            ),
            (
                "the same character by its code point",
                b"<Message>attribute &#39;a b&#39; is invalid</Message>",
                "attribute 'a b' is invalid",
            ),
            (
                "an escaped ampersand and angle bracket",
                b"<Message>a &amp; b &lt; c</Message>",
                "a & b < c",
            ),
            (
                "the indentation around character data is not part of it",
                b"<Message>\n            a plain sentence\n        </Message>",
                "a plain sentence",
            ),
        ];
        for (case, xml, expected) in rows {
            let element = read(xml).expect("the document reads");
            assert_eq!(
                element.text, *expected,
                "case {case:?}: expected {expected:?}, got {:?}",
                element.text
            );
        }
    }

    #[test]
    fn a_body_that_is_not_xml_carries_no_document() {
        for case in ["", "not xml at all", "{\"Code\":\"X\"}"] {
            let read = as_document(case.as_bytes());
            let empty = read
                .as_ref()
                .and_then(::serde_json::Value::as_object)
                .is_some_and(::serde_json::Map::is_empty);
            assert!(
                read.is_none() || empty,
                "case {case:?}: a body that is not an XML document carries no members, got \
                 {read:?}"
            );
        }
    }

    #[test]
    fn a_wrapped_refusal_is_read_through_its_error_element() {
        let document = as_document(
            b"<ErrorResponse><Error><Type>Sender</Type><Code>ValidationError</Code>\
              <Message>1 validation error detected</Message></Error>\
              <RequestId>r-1</RequestId></ErrorResponse>",
        )
        .expect("the refusal reads");
        assert_eq!(
            document.get("Code").and_then(::serde_json::Value::as_str),
            Some("ValidationError"),
            "the code is read through the Error element, got {document:?}"
        );
    }

    #[test]
    fn an_element_tree_is_read_as_the_document_it_carries() {
        let rows: &[(&str, &[u8], ::serde_json::Value)] = &[
            (
                "text only",
                b"<Arn>arn:aws:iam::1:user/u</Arn>",
                ::serde_json::json!("arn:aws:iam::1:user/u"),
            ),
            (
                "an object of members",
                b"<Result><Account>1</Account><UserId>U</UserId></Result>",
                ::serde_json::json!({ "Account": "1", "UserId": "U" }),
            ),
            (
                "a repeated name is an array",
                b"<Result><Tag>a</Tag><Tag>b</Tag><Tag>c</Tag></Result>",
                ::serde_json::json!({ "Tag": ["a", "b", "c"] }),
            ),
            (
                "nesting is nesting",
                b"<Result><User><Id>U</Id></User></Result>",
                ::serde_json::json!({ "User": { "Id": "U" } }),
            ),
        ];
        for (case, xml, expected) in rows {
            let element = read(xml).expect("the document reads");
            let value = as_value(&element);
            assert_eq!(
                value, *expected,
                "case {case:?}: expected {expected}, got {value}"
            );
        }
    }
}
