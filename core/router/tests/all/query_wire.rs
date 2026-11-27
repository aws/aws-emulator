// SPDX-License-Identifier: CC0-1.0

//! What a suite in this test crate needs to write a request's text and to read an XML
//! answer's: the percent-encoders that build a form body ([`form_encode`]) and a URI path
//! ([`path_encode`], which a REST suite addressing a resource by name needs), the escaper
//! that puts a value safely inside a request's XML text node ([`xml_escape`]), and the
//! readers that pull one element's text — or every occurrence of a repeated one — out of the
//! XML answer.
//!
//! They live here, ONCE, because a suite that writes its own drifts from the others: an
//! encoder that leaves one more byte literal, or a reader that stops at a different tag,
//! changes what the suite is actually asserting.
//!
//! The encoding is written out rather than taken from the maintained `percent-encoding`
//! crate for one reason, and it is not `Cargo.lock`: `core/router/Cargo.toml` is a PINNED
//! path in this repository (only `services/`, `core/router/tests/`, the issue files and
//! `Cargo.lock` are editable by a fix), so no dev-dependency can be added to this test
//! crate at all. The service crates under `services/`, whose manifests a fix may change,
//! take the crate instead — see `percent-encoding` in
//! `services/service-com-amazonaws-docdb-amazonrdsv19/Cargo.toml`. What cannot be taken
//! from a crate here is at least written once, behind one name.
//!
//! `cloudfront_key_value_store_handshake.rs` and `iam_signing_name_routing.rs` still carry
//! their own element readers, written before this module existed; they are to be migrated
//! onto [`opt_element`] / [`element`] by their own suites' issues. Nothing new should write
//! another one — call this module.
//!
//! [`pairs_of`] and the two readers over it ([`outputs_of`], [`physical_ids_of`]) are here
//! for the same reason: a suite that pairs a keyed list by zipping two document-ordered
//! element lists reads the wrong pair the moment one member omits half of it, so the pairing
//! is written once, over the `<member>` that carries both.
//!
//! Beside the wire readers, this module also holds the readings of a VALUE that several
//! suites judge an answer by rather than write a request with — whether a block holds an
//! address ([`block_contains`]), whether an identifier has the shape a service's model fixes
//! for it ([`is_waf_entity_id`]) — for the one reason the rest of it is here: a suite that
//! writes its own masking or its own reading of a pattern asserts something slightly
//! different from the suite beside it. A new such reading belongs here, not in one suite.

use std::collections::BTreeMap;
use std::net::Ipv4Addr;

/// Percent-encode one `application/x-www-form-urlencoded` component, leaving only RFC
/// 3986's unreserved characters (`ALPHA / DIGIT / "-" / "." / "_" / "~"`) literal.
///
/// Byte-wise, so a multi-byte character is escaped byte by byte, which is what the UTF-8
/// form encoding an AWS SDK sends does.
pub(crate) fn form_encode(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                encoded.push(char::from(byte));
            }
            other => encoded.push_str(&format!("%{other:02X}")),
        }
    }
    encoded
}

/// Percent-encode one URI PATH, leaving RFC 3986's unreserved characters and the `/` that
/// separates segments literal.
///
/// The path counterpart of [`form_encode`] — which escapes `/` too, because that is the
/// FORM-BODY encoding, and a path encoded with it would address one key named with literal
/// slashes. Here for the same reason the rest of this module is: a suite addressing a resource
/// whose name carries a byte no URI conveys literally — a non-ASCII object key, a space — has
/// to escape it, and one that writes its own escaping drifts from the others. Byte-wise, so a
/// multi-byte character is escaped byte by byte, which is what an AWS SDK sends and what a
/// greedy `{Key+}` label decodes back.
pub(crate) fn path_encode(path: &str) -> String {
    let mut encoded = String::with_capacity(path.len());
    for byte in path.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => {
                encoded.push(char::from(byte));
            }
            other => encoded.push_str(&format!("%{other:02X}")),
        }
    }
    encoded
}

/// The text of the first `<name>…</name>` element of an XML answer, or `None` when the
/// answer carries no such element.
///
/// A reader, not an XML parser, and only for documents this emulator itself produced:
/// machine-generated element trees with no comments, no CDATA and no same-name nesting.
pub(crate) fn opt_element(body: &str, name: &str) -> Option<String> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = body.find(&open)? + open.len();
    let end = body[start..].find(&close)? + start;
    Some(body[start..end].to_string())
}

/// [`opt_element`] where the element must be there; panics quoting the whole answer.
pub(crate) fn element(body: &str, name: &str) -> String {
    opt_element(body, name).unwrap_or_else(|| panic!("no <{name}> element in {body}"))
}

/// The inner text of the first `<name …>…</name>` element, whose own tag MAY carry
/// attributes.
///
/// [`opt_element`] matches the closing bracket as part of the tag, so it finds NOTHING for an
/// element the serializer wrote with an attribute (`<Metadata xmlns="…">…`, which is how the
/// awsQuery serializer writes a structure member). This reads from the tag's OWN closing
/// bracket instead — the technique [`members_of`] uses for a list member — so a suite
/// reading such an element scopes the read to it rather than falling back on "the first
/// `<Name>` in the whole document". The tag matches only where the NAME ends, so
/// `<MetadataEntry>` is not this element.
pub(crate) fn block_of<'a>(body: &'a str, name: &str) -> Option<&'a str> {
    // ONE tag scanner for both readings, stopped at the first block: dozens of suites call
    // this, so it allocates nothing and reads no further than it has to.
    let (open, close) = (format!("<{name}"), format!("</{name}>"));
    next_block(body, &open, &close).map(|(block, _)| block)
}

/// EVERY `<name …>…</name>` block of an XML answer, in document order — the repeated-element
/// counterpart of [`block_of`].
///
/// For a document that repeats a CONTAINER (`<Version>`, `<DeleteMarker>`, `<Deleted>`): a
/// suite reading one member out of each entry has to scope the read to that entry, or every
/// read finds the first entry's member. Here, once, for the reason this module exists — a
/// suite that writes its own scan drifts from the others. The tag matches only where the NAME
/// ends, so `<VersionId>` is not `<Version>`, and the blocks read through this carry no block
/// of the same name inside one.
pub(crate) fn blocks_of<'a>(body: &'a str, name: &str) -> Vec<&'a str> {
    let (open, close) = (format!("<{name}"), format!("</{name}>"));
    let mut found = Vec::new();
    let mut rest = body;
    while let Some((block, tail)) = next_block(rest, &open, &close) {
        found.push(block);
        rest = tail;
    }
    found
}

/// The next `<name …>…</name>` block of `body` and what follows it, or `None` when `body`
/// carries no further one.
///
/// The ONE tag scan [`block_of`] and [`blocks_of`] both read through, so the two cannot drift:
/// `open` is `<name` and `close` is `</name>`. A tag is this element's only where the NAME ends
/// — at the tag's closing bracket, a `/`, or whitespace before an attribute — so `<VersionId>`
/// is not `<Version>`, and the block runs from the tag's own closing bracket to the matching
/// closing tag.
fn next_block<'a>(body: &'a str, open: &str, close: &str) -> Option<(&'a str, &'a str)> {
    let mut rest = body;
    while let Some(at) = rest.find(open) {
        let after = &rest[at + open.len()..];
        match after.chars().next() {
            Some(next) if next == '>' || next == '/' || next.is_whitespace() => {
                let (_, inner) = after.split_once('>')?;
                return inner.split_once(close);
            }
            _ => rest = after,
        }
    }
    None
}

/// The text of EVERY `<name>…</name>` element of an XML answer, in document order, with
/// the five entities this emulator's XML serializer escapes decoded back — so a value that
/// is itself a document (an attribute carrying JSON, a policy) compares as the service
/// wrote it rather than as its escaping.
///
/// The repeated-element and decoding counterpart of [`opt_element`], for a suite reading a
/// LIST of a tag (every `<OutputValue>`, every `<key>`); the same reader, not an XML
/// parser, and under the same assumption about the documents it is pointed at.
pub(crate) fn elements(body: &str, name: &str) -> Vec<String> {
    let (open, close) = (format!("<{name}>"), format!("</{name}>"));
    let mut found = Vec::new();
    let mut rest = body;
    while let Some(start) = rest.find(&open) {
        let after = &rest[start + open.len()..];
        let Some(end) = after.find(&close) else { break };
        found.push(unescape(&after[..end]));
        rest = &after[end + close.len()..];
    }
    found
}

/// Escape the five entities an XML text node cannot carry literally, the way a client's XML
/// writer does — so a value a suite interpolates into a request document (an ARN, a policy, a
/// document that is itself JSON) keeps that request well-formed.
///
/// The inverse of [`unescape`] below, and here beside it for the reason this module exists: a
/// suite that writes its own escaper drifts from the others, and an escaper that leaves one
/// more byte literal changes what the suite is actually sending. `&` is replaced FIRST, so the
/// ampersands this introduces are not escaped again.
pub(crate) fn xml_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// The five entities an XML serializer escapes, decoded back. `&amp;` LAST, so a value
/// that carried a literal `&lt;` is not turned into a `<`.
fn unescape(text: &str) -> String {
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

/// Each `<member>` block of an awsQuery answer's LIST, in document order.
///
/// The serializer writes the FIRST member of a list with the service's namespace attribute
/// (`<member xmlns="…">`) and the rest bare, so a block starts after its tag's own closing
/// bracket and ends at its closing tag. The lists read through this carry no member inside a
/// member.
pub(crate) fn members_of(xml: &str) -> Vec<&str> {
    xml.split("<member")
        .skip(1)
        .filter_map(|block| block.split_once('>'))
        .filter_map(|(_, body)| body.split("</member>").next())
        .collect()
}

/// The `TargetHealth` block of one Elastic Load Balancing target health description — the
/// element a reported health's own `State`, `Reason` and `Description` are read out of.
///
/// Scoped, for the reason this module exists: a `TargetHealthDescription` also carries an
/// `AdministrativeOverride`, whose members are named `State`, `Reason` and `Description`
/// too, so a read that is not scoped to the health's own block takes the override's values
/// for a `healthy` target — whose health carries neither a reason nor a description. Answers
/// the empty string for a description carrying no health at all, which reads as "none" in
/// every suite below.
pub(crate) fn target_health_of(description: &str) -> &str {
    block_of(description, "TargetHealth").unwrap_or_default()
}

/// Each `<item>` block of an ec2Query answer's LIST, in document order.
///
/// [`members_of`] reads the awsQuery `<member>` form; an ec2Query list wraps each entry in
/// `<item>` instead, so a suite reading one route or one association out of a route table
/// scopes the read to that entry rather than to the first element of that name anywhere in
/// the answer. Here for the reason this module exists: a suite that writes its own drifts
/// from the others.
pub(crate) fn items_of(xml: &str) -> Vec<&str> {
    xml.split("<item")
        .skip(1)
        .filter_map(|block| block.split_once('>'))
        .filter_map(|(_, body)| body.split("</item>").next())
        .collect()
}

/// One pair of elements per list member, as the map a client reads a keyed list as.
///
/// Each pair is read from the member that carries BOTH, rather than by zipping two
/// document-ordered lists: a member that ever omitted one of them would silently misalign the
/// two lists, so a suite would compare the wrong pair instead of failing. A member carrying
/// only one half contributes nothing.
pub(crate) fn pairs_of(xml: &str, key: &str, value: &str) -> BTreeMap<String, String> {
    members_of(xml)
        .into_iter()
        .filter_map(|member| {
            let key = elements(member, key).into_iter().next()?;
            let value = elements(member, value).into_iter().next()?;
            Some((key, value))
        })
        .collect()
}

/// An awsQuery answer's MAP as the map a client reads it as: each entry's key paired with
/// its own value, with the escaped entities decoded — so a value that is itself a document
/// (a topic's access policy, a queue's redrive policy) compares as the service wrote it.
///
/// The decoding counterpart of [`entries_of`], and built on it rather than by zipping a list
/// of keys against a list of values, for the reason that reader records: an entry carrying
/// only one half would misalign every pair after it instead of being left out.
pub(crate) fn decoded_entries(xml: &str) -> BTreeMap<String, String> {
    entries_of(xml)
        .into_iter()
        .map(|(key, value)| (key, unescape(value)))
        .collect()
}

/// Each `<entry>` block of an awsQuery answer's MAP, as `(key, the value block)`.
///
/// The serializer writes a map as `<entry><key>…</key><value>…</value></entry>`, with the
/// first entry (and the first member inside it) carrying the service's namespace
/// attribute, so both halves are read through [`block_of`] rather than by tag text. Here
/// for the same reason as [`pairs_of`]: each pair is read from the entry that carries
/// BOTH, so an entry missing one half contributes nothing instead of misaligning the rest.
pub(crate) fn entries_of(xml: &str) -> Vec<(String, &str)> {
    xml.split("<entry")
        .skip(1)
        .filter_map(|block| block.split_once('>'))
        .filter_map(|(_, body)| body.split("</entry>").next())
        .filter_map(|entry| {
            let key = block_of(entry, "key")?.to_string();
            let value = block_of(entry, "value")?;
            Some((key, value))
        })
        .collect()
}

/// A `DescribeStacks` answer's `Outputs`, as the key/value map a client reads it as.
pub(crate) fn outputs_of(xml: &str) -> BTreeMap<String, String> {
    pairs_of(xml, "OutputKey", "OutputValue")
}

/// A `DescribeStackResources` answer as the logical-id -> physical-id map a client reads it
/// as: each member carries one of each.
pub(crate) fn physical_ids_of(xml: &str) -> BTreeMap<String, String> {
    pairs_of(xml, "LogicalResourceId", "PhysicalResourceId")
}

/// An Amazon S3 answer's `TagSet`, as the key-to-value map a client reads it as: every `<Tag>`
/// the document carries, each paired from the halves that ONE tag states.
///
/// Here, once, for the reason this module exists: several suites read a bucket's tag set out of
/// a `GetBucketTagging` answer, and one that writes its own scan drifts from the others. Each
/// pair is read from the `<Tag>` carrying BOTH halves rather than by zipping two
/// document-ordered lists, for the reason [`pairs_of`] records: a tag that ever omitted one half
/// would silently misalign every pair after it, so a suite would compare the wrong pair instead
/// of failing. A tag carrying only one half contributes nothing.
pub(crate) fn tag_set_of(xml: &str) -> BTreeMap<String, String> {
    blocks_of(xml, "Tag")
        .into_iter()
        .filter_map(|tag| {
            let key = elements(tag, "Key").into_iter().next()?;
            let value = elements(tag, "Value").into_iter().next()?;
            Some((key, value))
        })
        .collect()
}

/// Whether the IPv4 block `block` (`a.b.c.d/len`) holds the IPv4 address `address`.
///
/// Here, ONCE, for the reason this module exists: several suites judge an address a service
/// CHOSE against the block the subnet it sits in was created with — an interface's private
/// address, a directory's DNS address — and a suite that writes its own masking drifts from the
/// others. Both values come from the services' own answers, so a malformed one is a defect in
/// what was reported rather than a reason to answer "not inside": it panics quoting the value.
pub(crate) fn block_contains(block: &str, address: &str) -> bool {
    let (base, prefix) = block
        .split_once('/')
        .unwrap_or_else(|| panic!("expected a CIDR block of the form a.b.c.d/len, got {block:?}"));
    let base: Ipv4Addr = base
        .parse()
        .unwrap_or_else(|_| panic!("expected an IPv4 base address, got {block:?}"));
    let prefix: u32 = prefix
        .parse()
        .unwrap_or_else(|_| panic!("expected an IPv4 prefix length, got {block:?}"));
    assert!(
        prefix <= 32,
        "expected a prefix length of at most 32, got {block:?}"
    );
    let address: Ipv4Addr = address
        .parse()
        .unwrap_or_else(|_| panic!("expected an IPv4 address, got {address:?}"));
    // The whole block when the prefix covers no bits, which `u32::MAX << 32` would overflow on.
    let mask = if prefix == 0 {
        0
    } else {
        u32::MAX << (32 - prefix)
    };
    (u32::from(address) & mask) == (u32::from(base) & mask)
}

/// Whether `id` has the shape AWS WAF's `EntityId` takes:
/// `^[0-9a-f]{8}-(?:[0-9a-f]{4}-){3}[0-9a-f]{12}$`
/// (`smithy_query(query="shape",shape="EntityId")`), the id every WAFv2 entity is addressed
/// by — and what distinguishes an id that service MINTED from a name a template or
/// CloudFormation chose.
///
/// Here, ONCE, for the reason this module exists: several suites judge the identifier a
/// CloudFormation stack published for a WAFv2 resource against the form that service's own
/// model fixes, and a suite that writes its own reading of the pattern drifts from the others
/// — one of the three copies this replaced accepted upper-case hex digits the pattern does
/// not allow. The prefix is deliberately not checked for: the pattern is the whole rule.
pub(crate) fn is_waf_entity_id(id: &str) -> bool {
    is_lowercase_uuid(id)
}

/// Whether `id` has the form of an identifier this emulator MINTS per call or per resource: a
/// uuid rendered 8-4-4-4-12 in lower-case hex (`crate::state::new_request_id`'s own form).
///
/// The same reading [`is_waf_entity_id`] performs, named for the other thing it judges, and
/// delegating to the one implementation below so the two cannot drift: a suite checking the
/// id of a call the emulator reported — the `Request ID` an SDK diagnostic states, the token
/// of a resource request — is asking about a minted identifier rather than about a WAFv2
/// entity, and is asking it of the same form.
pub(crate) fn is_minted_uuid(id: &str) -> bool {
    is_lowercase_uuid(id)
}

/// The ONE reading both of the two above are: five hyphen-separated groups of 8, 4, 4, 4 and
/// 12 digits, each from `[0-9a-f]` — so the upper-case hex neither form admits is rejected
/// once rather than in each caller.
fn is_lowercase_uuid(id: &str) -> bool {
    let groups: Vec<&str> = id.split('-').collect();
    groups.len() == 5
        && [8, 4, 4, 4, 12]
            .iter()
            .zip(&groups)
            .all(|(width, group)| {
                group.len() == *width
                    && group
                        .bytes()
                        .all(|digit| matches!(digit, b'0'..=b'9' | b'a'..=b'f'))
            })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The `EntityId` reading answers for each way a value can miss that pattern: a group of
    /// the wrong width, a group count other than five, a digit outside `[0-9a-f]` (including
    /// the upper-case hex the pattern excludes), and an empty value.
    #[test]
    fn the_entity_id_reading_holds_to_the_pattern_the_model_fixes() {
        // (case, the value, whether it is an EntityId)
        let rows: [(&str, &str, bool); 8] = [
            (
                "the pattern's own shape",
                "0a1b2c3d-4e5f-6a7b-8c9d-0e1f2a3b4c5d",
                true,
            ),
            (
                "every digit a decimal one",
                "00000000-0000-0000-0000-000000000000",
                true,
            ),
            (
                "every digit the last hex one",
                "ffffffff-ffff-ffff-ffff-ffffffffffff",
                true,
            ),
            (
                "an upper-case hex digit, which the pattern excludes",
                "0A1b2c3d-4e5f-6a7b-8c9d-0e1f2a3b4c5d",
                false,
            ),
            (
                "a digit outside the hex alphabet",
                "0g1b2c3d-4e5f-6a7b-8c9d-0e1f2a3b4c5d",
                false,
            ),
            (
                "the first group one digit short",
                "0a1b2c3-4e5f-6a7b-8c9d-0e1f2a3b4c5d",
                false,
            ),
            (
                "four groups rather than five",
                "0a1b2c3d-4e5f-6a7b-8c9d0e1f2a3b4c5d",
                false,
            ),
            ("an empty value", "", false),
        ];

        for (case, id, expected) in rows {
            let read = is_waf_entity_id(id);
            assert_eq!(
                read, expected,
                "case {case:?}: expected is_waf_entity_id({id:?}) = {expected}, got {read}"
            );
        }
    }

    /// The containment reader answers for each bound of a block, over the block sizes a VPC's
    /// subnets carry between the two published ones (`/16` is the largest block a VPC takes and
    /// `/28` the smallest a subnet takes): each block's first and last address, and the address
    /// just outside either end.
    #[test]
    fn the_containment_reader_answers_for_each_bound_of_a_block() {
        // (case, the block, the address, whether the block holds it)
        let rows: [(&str, &str, &str, bool); 11] = [
            ("the first address of a /24", "10.80.1.0/24", "10.80.1.0", true),
            ("the last address of a /24", "10.80.1.0/24", "10.80.1.255", true),
            ("one past the end of a /24", "10.80.1.0/24", "10.80.2.0", false),
            (
                "one before the start of a /24",
                "10.80.1.0/24",
                "10.80.0.255",
                false,
            ),
            ("an address inside a /28", "10.81.0.16/28", "10.81.0.20", true),
            (
                "one past the end of a /28",
                "10.81.0.16/28",
                "10.81.0.32",
                false,
            ),
            (
                "the last address of a /20",
                "10.81.16.0/20",
                "10.81.31.255",
                true,
            ),
            (
                "one past the end of a /20",
                "10.81.16.0/20",
                "10.81.32.0",
                false,
            ),
            (
                "the last address of a /16",
                "10.82.0.0/16",
                "10.82.255.255",
                true,
            ),
            ("one past the end of a /16", "10.82.0.0/16", "10.83.0.0", false),
            ("every address of a /0", "0.0.0.0/0", "198.51.100.7", true),
        ];

        for (case, block, address, expected) in rows {
            let held = block_contains(block, address);
            assert_eq!(
                held, expected,
                "case {case:?}: expected {block} to hold {address} = {expected}, got {held}"
            );
        }
    }

    /// The encoder escapes everything outside the unreserved set and nothing inside it,
    /// over the classes an awsQuery component runs into.
    #[test]
    fn a_form_component_keeps_only_the_unreserved_characters() {
        // (case, the value, its encoding)
        let rows: [(&str, &str, &str); 6] = [
            ("the unreserved set", "aZ09-._~", "aZ09-._~"),
            ("a space", "a b", "a%20b"),
            ("an ARN's colons and slashes", "arn:aws:rds:x/y", "arn%3Aaws%3Ards%3Ax%2Fy"),
            ("the form separators", "a&b=c", "a%26b%3Dc"),
            ("XML markup", "<a>&</a>", "%3Ca%3E%26%3C%2Fa%3E"),
            ("a non-ASCII character", "é", "%C3%A9"),
        ];

        for (case, value, expected) in rows {
            let encoded = form_encode(value);
            assert_eq!(
                encoded, expected,
                "case {case:?}: expected {expected:?}, got {encoded:?}"
            );
        }
    }

    /// The PATH encoder keeps the segment separator literal and escapes everything else
    /// outside the unreserved set, over the classes a resource name in a path runs into.
    #[test]
    fn a_path_keeps_its_separators_and_escapes_the_rest() {
        // (case, the path, its encoding)
        let rows: [(&str, &str, &str); 6] = [
            ("the unreserved set", "/bucket/aZ09-._~", "/bucket/aZ09-._~"),
            ("a space in a segment", "/bucket/a b", "/bucket/a%20b"),
            ("a non-ASCII character", "/bucket/é", "/bucket/%C3%A9"),
            ("a query separator in a name", "/bucket/a?b", "/bucket/a%3Fb"),
            ("an already-escaped escape", "/bucket/a%20b", "/bucket/a%2520b"),
            ("every separator kept", "/a/b/c", "/a/b/c"),
        ];

        for (case, path, expected) in rows {
            let encoded = path_encode(path);
            assert_eq!(
                encoded, expected,
                "case {case:?}: expected {expected:?}, got {encoded:?}"
            );
        }
    }

    /// The attribute-tolerant reader scopes the read to the element it names, over the tag
    /// forms this emulator's awsQuery serializer writes: a bare tag, one carrying the
    /// service's namespace attribute, and a tag whose name is a PREFIX of another's.
    #[test]
    fn the_block_reader_finds_an_element_whose_tag_carries_attributes() {
        // (case, the answer, the tag read, its inner text)
        let rows: [(&str, &str, &str, Option<&str>); 5] = [
            (
                "a bare tag",
                "<R><M><Name>a</Name></M></R>",
                "M",
                Some("<Name>a</Name>"),
            ),
            (
                "a tag carrying the service's namespace",
                "<R><M xmlns=\"http://x/doc/\"><Name>a</Name></M></R>",
                "M",
                Some("<Name>a</Name>"),
            ),
            (
                "a tag whose name is a prefix of the one asked for",
                "<R><MEntry><Name>b</Name></MEntry><M><Name>a</Name></M></R>",
                "M",
                Some("<Name>a</Name>"),
            ),
            (
                "the first of two of a tag",
                "<R><M>first</M><M>second</M></R>",
                "M",
                Some("first"),
            ),
            (
                "a tag the answer does not carry",
                "<R><M>a</M></R>",
                "Other",
                None,
            ),
        ];

        for (case, body, tag, expected) in rows {
            let read = block_of(body, tag);
            assert_eq!(
                read, expected,
                "case {case:?}: expected {expected:?}, got {read:?}"
            );
        }
    }

    /// The repeated-block reader scopes each read to ONE entry, over the container forms a
    /// repeated XML list arrives in: bare tags, a tag carrying a namespace attribute, a tag
    /// whose name is a prefix of the one asked for, and a name the document does not carry.
    #[test]
    fn the_repeated_block_reader_finds_every_entry() {
        // (case, the answer, the container read, the blocks it carries)
        let rows: [(&str, &str, &str, &[&str]); 4] = [
            (
                "two entries of a container",
                "<R><Version><Key>a</Key><VersionId>v1</VersionId></Version>\
                 <Version><Key>b</Key><VersionId>v2</VersionId></Version></R>",
                "Version",
                &[
                    "<Key>a</Key><VersionId>v1</VersionId>",
                    "<Key>b</Key><VersionId>v2</VersionId>",
                ],
            ),
            (
                "an entry whose tag carries an attribute",
                "<R><Version xmlns=\"http://x/doc/\"><Key>a</Key></Version></R>",
                "Version",
                &["<Key>a</Key>"],
            ),
            (
                "a container whose name is a prefix of another tag",
                "<R><VersionId>v1</VersionId><Version><Key>a</Key></Version></R>",
                "Version",
                &["<Key>a</Key>"],
            ),
            (
                "a container the answer does not carry",
                "<R><Version><Key>a</Key></Version></R>",
                "DeleteMarker",
                &[],
            ),
        ];

        for (case, body, container, expected) in rows {
            let read = blocks_of(body, container);
            assert_eq!(
                read, expected,
                "case {case:?}: expected {expected:?}, got {read:?}"
            );
        }
    }

    /// The escaper and the decoder are inverses over every entity either of them knows, so a
    /// value a suite interpolates into a request comes back out of an answer as it went in.
    #[test]
    fn escaping_a_value_and_decoding_it_again_is_the_value() {
        // (case, the value)
        let rows: [(&str, &str); 6] = [
            ("text carrying none of the five", "a plain value"),
            ("an ampersand", "a & b"),
            ("markup", "<a>text</a>"),
            ("quotes of both kinds", "he said \"it's here\""),
            (
                "a document that is itself JSON",
                r#"{"AWSPolicyFormatVersion":"2015-10-01","RecordType":"A"}"#,
            ),
            ("an already-escaped entity", "&lt;a&gt;"),
        ];

        for (case, value) in rows {
            let escaped = xml_escape(value);
            assert!(
                !escaped.contains('<') && !escaped.contains('>'),
                "case {case:?}: an escaped value carries no markup, got {escaped:?}"
            );
            let decoded = unescape(&escaped);
            assert_eq!(
                decoded, value,
                "case {case:?}: expected {value:?} back, got {decoded:?}"
            );
        }
    }

    /// The reader answers the first element's text, and `None` for one that is absent.
    #[test]
    fn the_reader_finds_the_first_element_or_nothing() {
        let body = "<Root><Code>A</Code><Code>B</Code></Root>";
        assert_eq!(
            opt_element(body, "Code"),
            Some("A".to_string()),
            "case \"the first of two\": got {body}"
        );
        assert_eq!(
            opt_element(body, "Message"),
            None,
            "case \"an element the answer does not carry\": got {body}"
        );
        assert_eq!(
            element(body, "Root"),
            "<Code>A</Code><Code>B</Code>",
            "case \"a parent element\": got {body}"
        );
    }

    /// The repeated-element reader answers every one of them, in document order, with the
    /// serializer's escaping undone.
    #[test]
    fn the_repeated_element_reader_finds_every_one_decoded() {
        // (case, the answer, the tag read, the texts it carries)
        let rows: [(&str, &str, &str, &[&str]); 4] = [
            (
                "two of a tag",
                "<R><Code>A</Code><Code>B</Code></R>",
                "Code",
                &["A", "B"],
            ),
            (
                "a tag the answer does not carry",
                "<R><Code>A</Code></R>",
                "Message",
                &[],
            ),
            (
                "an escaped document value",
                "<R><value>{&quot;a&quot;:1}</value></R>",
                "value",
                &[r#"{"a":1}"#],
            ),
            (
                "every entity the serializer escapes",
                "<R><value>&lt;a&gt; &amp; &apos;b&apos;</value></R>",
                "value",
                &["<a> & 'b'"],
            ),
        ];

        for (case, body, tag, expected) in rows {
            let read = elements(body, tag);
            assert_eq!(
                read, expected,
                "case {case:?}: expected {expected:?}, got {read:?}"
            );
        }
    }

    /// The list reader pairs the two elements of each member with EACH OTHER, over the member
    /// forms this emulator's awsQuery serializer writes: the first member of a list carries
    /// the service's namespace attribute, the rest are bare, a member missing one half of the
    /// pair contributes nothing rather than stealing the next member's value, and a value the
    /// serializer escaped comes back decoded.
    #[test]
    fn the_list_reader_pairs_within_each_member() {
        let answer = concat!(
            "<StackResources>",
            "<member xmlns=\"http://cloudformation.amazonaws.com/doc/2010-05-15/\">",
            "<LogicalResourceId>Record</LogicalResourceId>",
            "<PhysicalResourceId>a.example.</PhysicalResourceId></member>",
            "<member><LogicalResourceId>Zone</LogicalResourceId>",
            "<PhysicalResourceId>Z0123456789ABCD</PhysicalResourceId></member>",
            "<member><LogicalResourceId>Pending</LogicalResourceId></member>",
            "</StackResources>"
        );
        let read = physical_ids_of(answer);
        let expected: BTreeMap<String, String> =
            [("Record", "a.example."), ("Zone", "Z0123456789ABCD")]
                .into_iter()
                .map(|(key, value)| (key.to_string(), value.to_string()))
                .collect();
        assert_eq!(
            read, expected,
            "case \"a namespaced member, a bare one and one with no physical id\": expected \
             {expected:?}, got {read:?}"
        );

        let outputs = concat!(
            "<Outputs><member xmlns=\"http://cloudformation.amazonaws.com/doc/2010-05-15/\">",
            "<OutputKey>Zone</OutputKey><OutputValue>Z0123456789ABCD</OutputValue></member>",
            "<member><OutputKey>Quoted</OutputValue></member>",
            "<member><OutputKey>Escaped</OutputKey>",
            "<OutputValue>a &amp; b</OutputValue></member></Outputs>"
        );
        let read = outputs_of(outputs);
        let expected: BTreeMap<String, String> =
            [("Zone", "Z0123456789ABCD"), ("Escaped", "a & b")]
                .into_iter()
                .map(|(key, value)| (key.to_string(), value.to_string()))
                .collect();
        assert_eq!(
            read, expected,
            "case \"an output list with a malformed member\": expected {expected:?}, got \
             {read:?}"
        );
    }
}
