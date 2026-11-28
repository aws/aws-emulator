// SPDX-License-Identifier: CC0-1.0

//! What the multi-item paths share: `BatchWriteItem`, `BatchGetItem`,
//! `TransactWriteItems` and `TransactGetItems`.
//!
//! The documented ceilings and refusal wordings those four operations hold a request
//! to, the per-table capacity and item-collection ledgers each of them reports from,
//! what a `ClientRequestToken` stands for, and the canonical encoding a repeated token
//! is compared through. They live beside [`super`] rather than inside it because they
//! are about a REQUEST that spans several items, while the parent module is the table
//! and item store itself; `super` re-exports them, so every caller still names them
//! `crate::state::…`.

use super::*;
// The shape of one `BatchGetItem` `RequestItems` entry, which the rendering of that
// member walks ([`rendered_batch_get_request_items`]); `super` reaches only the shapes it
// names itself.
use crate::ops::types::{
    ConditionCheck, Delete, KeysAndAttributes, ParameterizedStatement, Put, TransactWriteItem,
    Update, WriteRequest,
};

/// How many put-or-delete operations one `BatchWriteItem` may carry: "up to 25 item
/// put or delete operations" (the `BatchWriteItem` doc), which is also the length the
/// model gives `BatchWriteItemRequestMap`. The ceiling is on the OPERATIONS, counted
/// across every table the request groups them under, not on the tables.
pub const MAX_BATCH_WRITE_REQUESTS: usize = 25;

/// How many keys one `BatchGetItem` may address: "A single operation can retrieve up
/// to 16 MB of data, which can contain as many as 100 items" (the `BatchGetItem`
/// doc), again counted across every table in the request.
pub const MAX_BATCH_GET_KEYS: usize = 100;

/// The most item data ONE `BatchGetItem` response carries, 16 MB: "A single operation
/// can retrieve up to 16 MB of data, which can contain as many as 100 items.
/// BatchGetItem returns a partial result if the response size limit is exceeded [...]
/// If a partial result is returned, the operation returns a value for UnprocessedKeys"
/// (the `BatchGetItem` doc). The two ceilings are independent: a batch of 100 small
/// items is answered whole, while "if you ask to retrieve 100 items, but each
/// individual item is 300 KB in size, the system returns 52 items (so as not to exceed
/// the 16 MB limit)".
///
/// A batch that reaches it stops on a WHOLE-item boundary and hands the keys it did not
/// read back under `UnprocessedKeys`, so no item is ever truncated and one request
/// cannot amplify into a response no client of the real service would ever receive.
pub const MAX_BATCH_GET_RESPONSE_BYTES: i64 = 16 * 1024 * 1024;

/// How a batch that addresses one key twice is refused: AWS's FIXED sentence for that
/// case, naming neither the key nor the table — "you cannot put and delete the same
/// item in the same BatchWriteItem request" (the `BatchWriteItem` doc), and
/// `BatchGetItem` refuses a repeated key the same way.
pub const DUPLICATE_BATCH_KEYS_MESSAGE: &str = "Provided list of item keys contains duplicates";

/// How a batch that addresses one table twice is refused: the rule the model states,
/// "Each table name or ARN can be used only once per BatchGetItem request"
/// (`BatchGetItem$RequestItems`) — two map entries that resolve to the same table are
/// one table addressed twice, whichever spelling each used.
pub const DUPLICATE_BATCH_TABLE_MESSAGE: &str =
    "Each table name or ARN can be used only once per request";

/// The member path the batched operations' `RequestItems` map is reported under: the
/// lowerCamel path the request validator builds, which is what the recorded refusals of
/// that member state.
pub const REQUEST_ITEMS_MEMBER_PATH: &str = "requestItems";

/// The table one entry of a `RequestItems` map addresses, or how a key that addresses
/// none is refused.
///
/// The key accepts the same two spellings the `TableName` member does — "each table name
/// or ARN can be used only once per BatchGetItem request"
/// (`BatchGetItem$RequestItems`) — so it is resolved through the one function that reads
/// either in the region the request reached ([`resolve_table_name_in`]).
///
/// What differs is the REFUSAL of a key that is neither. A map key is not a member of the
/// request: the validating walk treats it as a path segment, so there is no `tableName`
/// member to report a bound against, and the constraint that was broken is the one the
/// map's KEYS are held to. The clause therefore names the MAP member and lists the whole
/// set a key must satisfy, which is how every service states a refused map key
/// (`service_common::validation_message::map_key_refusal`, the tree's one rendering of
/// that clause).
///
/// The VALUE half of that clause is the map the offending key was held in, quoted as the
/// request carried it, and `quoted_map` is what renders it: a request-validator clause
/// states the value it refused (`Value '<the submitted map>' at 'requestItems' …`), so the
/// caller can see WHICH map its rejected key sat in. Each of the two batched operations
/// renders its own `RequestItems` the way its recorded refusals state it:
/// `BatchGetItem` as the JSON the request carried
/// ([`rendered_batch_get_request_items`]), and `BatchWriteItem` as the object form the
/// service's own request validator states a refused structure value with
/// ([`rendered_batch_write_request_items`]).
///
/// It is a closure rather than a value because it is needed only by the refusal: a served
/// batch renders nothing.
pub fn batch_table_key(
    key: &str,
    region: &str,
    quoted_map: impl FnOnce() -> Option<String>,
) -> Result<String, RequestError> {
    resolve_table_name_in(key, region).map_err(|_| {
        RequestError::Validation(
            ::service_common::validation_message::map_key_refusal_quoting(
                REQUEST_ITEMS_MEMBER_PATH,
                quoted_map().as_deref(),
                i64::try_from(MIN_TABLE_NAME_LENGTH).unwrap_or(0),
                i64::try_from(MAX_TABLE_NAME_LENGTH).unwrap_or(0),
                // The expression a key must satisfy, without the anchoring that makes the
                // match total — the tree's one rendering of that, which the clause above
                // comes from too (`service_common::validation::unanchored`).
                Some(&::service_common::validation::unanchored(
                    TABLE_NAME_PATTERN,
                )),
            ),
        )
    })
}

/// How many write requests one `BatchWriteItem` must carry at least: the other end of
/// the bound [`MAX_BATCH_WRITE_REQUESTS`] closes. "A map of one or more table names"
/// (`BatchWriteItem$RequestItems`), and the model gives that member `@length` min 1, so a
/// call that carries no operation at all — an empty map, or a map whose every table was
/// given an empty list — asks for nothing and is refused rather than answered with an
/// empty residue.
pub const MIN_BATCH_WRITE_REQUESTS: usize = 1;

/// The whole requirement set the write-request list one table was given must satisfy, as a
/// clause about the MAP states it.
///
/// The bound 1..25 is declared on the map's VALUE shape (`smithy_query(query="shape",
/// shape="com.amazonaws.dynamodb#BatchWriteItemRequestMap")` gives a value of
/// `WriteRequests`, itself `@length{min:1,max:25}`), and a constraint a map's VALUES broke
/// is stated at the MAP member carrying the value shape's WHOLE declared set, rather than at
/// the entry whose own list broke it: `Map value must satisfy constraint: [ … ]`, the sibling
/// of the `Map keys must satisfy constraint: [ … ]` a key that names no table earns
/// ([`batch_table_key`]). The sentence around the set is the tree's one rendering of it
/// (`service_common::validation_message::map_value_set_requirement`) and the requirements
/// inside it are this service's own phrases ([`max_length_requirement`],
/// [`min_length_requirement`]), stated maximum first, which is the order every constraint set
/// this service states lists its bounds in.
fn write_requests_requirement_set() -> String {
    ::service_common::validation_message::map_value_set_requirement(&[
        max_length_requirement(MAX_BATCH_WRITE_REQUESTS),
        min_length_requirement(MIN_BATCH_WRITE_REQUESTS),
    ])
}

/// How a `BatchWriteItem` whose `RequestItems` carries a number of write requests outside
/// its declared bound is refused.
///
/// TWO BOUNDS, TWO REFUSALS, because they are broken by two different things. The `@length`
/// 1..25 the model declares is on the write-request LIST one table was given, and on the
/// map's own entries (`smithy_query(query="shape",
/// shape="com.amazonaws.dynamodb#BatchWriteItemRequestMap")`): a list outside it is a
/// constraint the map's VALUES broke, so it is reported at the MAP member — the lowerCamel
/// path the request validator builds ([`REQUEST_ITEMS_MEMBER_PATH`]) — carrying the value
/// shape's whole declared set ([`write_requests_requirement_set`]) rather than the one end of
/// the bound the request happened to break, inside the envelope that counts the clauses
/// ([`validation_message`]). A list outside EITHER end is that same violation, whichever
/// table of the map carries it, so an entry given no request at all is refused even when
/// another entry of the same map carries some.
///
/// The ceiling on the OPERATIONS, "up to 25 item put or delete operations" counted across
/// every table the request grouped them under, is the CALL's own quota rather than a bound
/// on any one member it carries, so a request whose every list is inside its bound and whose
/// operations still sum past that ceiling is refused with the fixed sentence this service
/// states for a batch that asks for more than one call may carry
/// ([`too_many_batch_items`]) — which is how the batched READ refuses its own aggregate
/// ceiling, and which echoes nothing of the request.
///
/// The MAP's own `@length` minimum is the third refusal, and the only one a map whose every
/// entry is inside the value bound can still break: a map carrying no entry at all. Its
/// clause states the member and the bound alone, because what a clause echoes is the map the
/// request sent ([`rendered_batch_write_request_items`]) and an empty map has nothing to
/// show.
///
/// WHY THIS SEAM QUOTES THE MAP AND THE FRAMEWORK'S DOES NOT. The bound on the write
/// REQUESTS is the handler's to apply — the validating walk ahead of it length-checks the
/// map's ENTRIES, which is a different count — and the handler has the request in hand, so
/// its clause states the value it refused. A clause the walk raises about the same member
/// (a map past the entry count the model gives it) reaches
/// `Service::override_refusal` with the walk's own summary of the collection rather than
/// the collection, so there is nothing there to render from and that clause states the
/// member and the bound alone (see "Which form a constraint clause takes" in
/// `src/service.rs`). One member, one renderer, and the value half follows which seam
/// could see the map.
pub fn batch_write_requests_outside_bound(
    request_items: &HashMap<String, Vec<WriteRequest>>,
    requested: usize,
) -> Option<RequestError> {
    // A list outside the bound the model declares for the map's VALUES, whichever table
    // carries it: the clause is about the MAP, so it states that shape's whole set over the
    // map the request sent.
    let bound = MIN_BATCH_WRITE_REQUESTS..=MAX_BATCH_WRITE_REQUESTS;
    if request_items
        .values()
        .any(|requests| !bound.contains(&requests.len()))
    {
        return Some(invalid_member(
            REQUEST_ITEMS_MEMBER_PATH,
            Some(&rendered_batch_write_request_items(request_items)),
            &write_requests_requirement_set(),
        ));
    }
    // The map's own minimum, which only a map carrying no entry at all reaches here.
    if requested < MIN_BATCH_WRITE_REQUESTS {
        return Some(invalid_member(
            REQUEST_ITEMS_MEMBER_PATH,
            None,
            &min_length_requirement(MIN_BATCH_WRITE_REQUESTS),
        ));
    }
    if requested > MAX_BATCH_WRITE_REQUESTS {
        return Some(too_many_batch_items(BATCH_WRITE_CALL));
    }
    None
}

/// A `BatchWriteItem`'s `RequestItems` map as the refusal of that member quotes it.
///
/// This is the OBJECT rendering the service's own request validator states a refused
/// structure value with — `{<table>=[WriteRequest(putRequest=PutRequest(item={<attribute>=
/// AttributeValue(…)}, …), deleteRequest=null)]}` — which is the form the recorded
/// refusals of this member carry, and NOT the JSON the request arrived as (that is what
/// `BatchGetItem`'s own member is quoted as — [`rendered_batch_get_request_items`]).
/// Every value in it comes from the request; the field names and their order are the
/// renderer's fixed frame ([`CORAL_ATTRIBUTE_VALUE_FIELDS`]).
///
/// A map arrives as a `HashMap`, whose iteration order is not the request's, so the table
/// entries and an item's attributes are written in NAME order: one submitted map renders
/// one way, whatever order its JSON arrived in. Each table's requests keep the order the
/// request listed them in, which is the order they would have been applied in.
///
/// WHAT THE REQUEST CANNOT GROW. The map, its tables, its write requests, its items'
/// attributes and the nesting inside them are all the caller's, so the rendering is
/// written straight into one [`BoundedText`] — every byte goes through a buffer that stops
/// at [`MAX_RENDERED_VALUE_BYTES`], so the clamp PRECEDES the bytes rather than clipping
/// them afterwards, and no part of this walk collects a value or a rendering of its own on
/// the side. The nesting is walked over an explicit stack bounded by
/// [`MAX_RENDER_STEPS`] rather than by recursion, so neither the stack nor the heap this
/// rendering uses is a function of the request.
pub fn rendered_batch_write_request_items(
    request_items: &HashMap<String, Vec<WriteRequest>>,
) -> String {
    let mut out = BoundedText::default();
    out.push("{");
    for (at, (table, requests)) in request_items
        .iter()
        .collect::<BTreeMap<_, _>>()
        .into_iter()
        .enumerate()
    {
        if !out.has_room() {
            break;
        }
        if at > 0 {
            out.push(", ");
        }
        out.push(table);
        out.push("=[");
        for (at, request) in requests.iter().enumerate() {
            if !out.has_room() {
                break;
            }
            if at > 0 {
                out.push(", ");
            }
            push_write_request(&mut out, request);
        }
        out.push("]");
    }
    out.push("}");
    out.into_text()
}

/// How much of a refused collection a clause echoes at most.
///
/// A clause of AWS's request validator quotes the value the request carried, so the ceiling
/// is not a wording choice: it is the bound that keeps one request from amplifying into a
/// response of its own choosing. The map, its tables, its write requests, its items'
/// attributes and the nesting inside them are all the caller's, and the rendering writes a
/// field frame per attribute value in it, so a message carrying all of a very large one
/// would grow faster than the request that asked for it. It is far above what the refusals
/// of these members earn in practice — a batch at the documented 25 operations of a few
/// attributes each renders a few kilobytes — so what it bounds is the amplification and not
/// the clause.
const MAX_RENDERED_VALUE_BYTES: usize = 64 * 1024;

/// The bytes one rendering is written into, holding at most
/// [`MAX_RENDERED_VALUE_BYTES`] of them.
///
/// EVERY byte of a rendering goes through [`Self::push`], and a walk stops as soon as
/// [`Self::has_room`] answers `false`, so what the clause quotes is bounded BEFORE it is
/// produced: nothing in this module renders a value in full and then cuts it down.
#[derive(Debug, Default)]
struct BoundedText {
    text: String,
}

impl BoundedText {
    /// Whether the rendering has room for more.
    fn has_room(&self) -> bool {
        self.text.len() < MAX_RENDERED_VALUE_BYTES
    }

    /// Append what of `fragment` fits, cut on a character boundary so the message stays
    /// valid UTF-8.
    fn push(&mut self, fragment: &str) {
        let room = MAX_RENDERED_VALUE_BYTES.saturating_sub(self.text.len());
        if room == 0 {
            return;
        }
        if fragment.len() <= room {
            self.text.push_str(fragment);
            return;
        }
        let mut end = room;
        while end > 0 && !fragment.is_char_boundary(end) {
            end -= 1;
        }
        self.text.push_str(&fragment[..end]);
    }

    /// The rendering.
    fn into_text(self) -> String {
        self.text
    }
}

/// One `WriteRequest` of that map: both members, the one it carries rendered and the
/// other `null`, in the order the model declares them (`smithy_query(query="shape",
/// shape="com.amazonaws.dynamodb#WriteRequest")` gives `PutRequest DeleteRequest`).
fn push_write_request(out: &mut BoundedText, request: &WriteRequest) {
    out.push("WriteRequest(putRequest=");
    match request.put_request.as_ref() {
        Some(put) => {
            out.push("PutRequest(item=");
            push_coral_item(out, &put.item);
            out.push(", ");
            out.push(TRAILING_REQUEST_FIELDS);
            out.push(")");
        }
        None => out.push(NULL_FIELD),
    }
    out.push(", deleteRequest=");
    match request.delete_request.as_ref() {
        Some(delete) => {
            out.push("DeleteRequest(key=");
            push_coral_item(out, &delete.key);
            out.push(", ");
            out.push(TRAILING_REQUEST_FIELDS);
            out.push(")");
        }
        None => out.push(NULL_FIELD),
    }
    out.push(")");
}

/// How the rendering writes a field the request left unset.
const NULL_FIELD: &str = "null";

/// The fields a `PutRequest`/`DeleteRequest` carries BESIDE the item or key it addresses,
/// as the refusals of this member render them: the service's own request objects carry a
/// workload-profile field this model does not declare, and no request this emulator is
/// sent can set it, so it renders unset.
const TRAILING_REQUEST_FIELDS: &str = "workloadProfileName=null";

/// An `Item` or a `Key` map in that rendering: `{<attribute>=<value>}`, the attributes in
/// name order for the reason the tables are, each value over every field of the service's
/// own attribute value object ([`CORAL_ATTRIBUTE_VALUE_FIELDS`]).
fn push_coral_item(out: &mut BoundedText, item: &Item) {
    // Nothing is walked once the rendering is full: the budget that stops the walk is the
    // one shared by the WHOLE map, so what one item can make this module hold is the step
    // stack below and nothing else.
    if !out.has_room() {
        return;
    }
    let mut work: Vec<RenderStep<'_>> = Vec::new();
    let mut pushed = 0usize;
    push_map_steps(&mut work, &mut pushed, item, None);
    render(out, work, pushed);
}

/// One step of the walk the rendering above runs.
///
/// `M` and `L` nest as deep as the REQUEST chose to nest them, so the walk is ITERATIVE
/// over this explicit stack rather than recursive — the same rule, and for the same
/// reason, as the canonical item encoding below ([`canonical_item_text`]): recursing on
/// caller-chosen depth would make STACK use a function of the request, and a stack that
/// runs out aborts the process rather than refusing the request. Every step BORROWS what
/// it writes, so the stack itself is as many steps as it holds and nothing more
/// ([`MAX_RENDER_STEPS`] is how many that is).
enum RenderStep<'a> {
    /// Write one fixed fragment of the rendering.
    Text(&'static str),
    /// Write `<name>=` for an attribute the request named.
    Name(&'a str),
    /// Write the frame's fields up to `field`, ending in `<field>=`, so what comes next is
    /// written as that field's value.
    FramePrefix(&'static str),
    /// Write the frame's fields after `field`, closing the frame.
    FrameSuffix(&'static str),
    /// Write one value: a scalar writes its own frame, a composite writes its frame around
    /// the steps for its children.
    Value(&'a AttributeValue),
}

/// How many steps one rendering walks at most.
///
/// Derived from the ceiling on what a clause quotes ([`MAX_RENDERED_VALUE_BYTES`]) and the
/// fewest bytes one value writes ([`MIN_RENDERED_VALUE_BYTES`]): a walk past this many
/// steps is a walk whose text that ceiling has already cut. A map or a list stops adding
/// entries once the budget is spent, and a composite the budget cannot hold the children of
/// is written as the EMPTY collection instead of walked into, so every value still
/// contributes its frame and the walk's own memory is a constant of this module rather
/// than a function of the request.
const MAX_RENDER_STEPS: usize = MAX_RENDERED_VALUE_BYTES / MIN_RENDERED_VALUE_BYTES;

/// The fewest bytes one value's rendering writes: the attribute-value frame's own field
/// names, the `=` and `, ` between them and the `null`s for the fields it leaves unset come
/// to several times this, so it is a conservative floor rather than a measurement.
const MIN_RENDERED_VALUE_BYTES: usize = 64;

/// The steps of one MAP, pushed onto `work` so they run in order: the frame around it when
/// `field` names one, the braces, and each entry's name and value in name order.
///
/// Entries are added while the step budget holds them, so a map with more entries than one
/// rendering can carry is written as far as it goes — which is past the byte ceiling in
/// every case the budget is reached at all.
fn push_map_steps<'a>(
    work: &mut Vec<RenderStep<'a>>,
    pushed: &mut usize,
    entries: &'a Item,
    field: Option<&'static str>,
) {
    let ordered: BTreeMap<&'a String, &'a AttributeValue> = entries.iter().collect();
    let mut sequence: Vec<RenderStep<'a>> = Vec::new();
    if let Some(field) = field {
        sequence.push(RenderStep::FramePrefix(field));
    }
    sequence.push(RenderStep::Text("{"));
    for (at, (name, value)) in ordered.into_iter().enumerate() {
        if !within_step_budget(*pushed, sequence.len() + 3) {
            break;
        }
        if at > 0 {
            sequence.push(RenderStep::Text(", "));
        }
        sequence.push(RenderStep::Name(name.as_str()));
        sequence.push(RenderStep::Value(value));
    }
    sequence.push(RenderStep::Text("}"));
    if let Some(field) = field {
        sequence.push(RenderStep::FrameSuffix(field));
    }
    extend_in_order(work, pushed, sequence);
}

/// The steps of one LIST, pushed the same way and bounded the same way: the frame around
/// it, the brackets, and each element in the order the request listed them.
fn push_list_steps<'a>(
    work: &mut Vec<RenderStep<'a>>,
    pushed: &mut usize,
    values: &'a [AttributeValue],
    field: &'static str,
) {
    let mut sequence: Vec<RenderStep<'a>> = Vec::new();
    sequence.push(RenderStep::FramePrefix(field));
    sequence.push(RenderStep::Text("["));
    for (at, value) in values.iter().enumerate() {
        if !within_step_budget(*pushed, sequence.len() + 2) {
            break;
        }
        if at > 0 {
            sequence.push(RenderStep::Text(", "));
        }
        sequence.push(RenderStep::Value(value));
    }
    sequence.push(RenderStep::Text("]"));
    sequence.push(RenderStep::FrameSuffix(field));
    extend_in_order(work, pushed, sequence);
}

/// Push `sequence` so the walk runs it in the order it was built, counting the steps
/// against [`MAX_RENDER_STEPS`].
fn extend_in_order<'a>(
    work: &mut Vec<RenderStep<'a>>,
    pushed: &mut usize,
    sequence: Vec<RenderStep<'a>>,
) {
    *pushed = pushed.saturating_add(sequence.len());
    work.extend(sequence.into_iter().rev());
}

/// Whether the walk may push `steps` more steps.
fn within_step_budget(pushed: usize, steps: usize) -> bool {
    pushed.saturating_add(steps) <= MAX_RENDER_STEPS
}

/// Run that walk into `out`, stopping as soon as the rendering has no room left.
fn render(out: &mut BoundedText, mut work: Vec<RenderStep<'_>>, mut pushed: usize) {
    while let Some(step) = work.pop() {
        if !out.has_room() {
            return;
        }
        match step {
            RenderStep::Text(text) => out.push(text),
            RenderStep::Name(name) => {
                out.push(name);
                out.push("=");
            }
            RenderStep::FramePrefix(field) => push_frame_prefix(out, field),
            RenderStep::FrameSuffix(field) => push_frame_suffix(out, field),
            RenderStep::Value(value) => match value {
                // A composite is walked into while the step budget holds its children,
                // and written as the empty collection it would otherwise be walked into
                // once it does not.
                AttributeValue::M(entries)
                    if within_step_budget(pushed, entries.len() * 3 + 4) =>
                {
                    push_map_steps(&mut work, &mut pushed, entries, Some("m"));
                }
                AttributeValue::L(values)
                    if within_step_budget(pushed, values.len() * 2 + 4) =>
                {
                    push_list_steps(&mut work, &mut pushed, values, "l");
                }
                AttributeValue::M(_) => push_field(out, "m", "{}"),
                AttributeValue::L(_) => push_field(out, "l", "[]"),
                AttributeValue::S(text) => push_field(out, "s", text),
                AttributeValue::N(text) => push_field(out, "n", text),
                AttributeValue::B(bytes) => {
                    push_frame_prefix(out, "b");
                    push_coral_blob(out, bytes);
                    push_frame_suffix(out, "b");
                }
                AttributeValue::Ss(texts) => push_text_list(out, "sS", texts),
                AttributeValue::Ns(texts) => push_text_list(out, "nS", texts),
                AttributeValue::Bs(blobs) => {
                    push_frame_prefix(out, "bS");
                    out.push("[");
                    for (at, bytes) in blobs.iter().enumerate() {
                        if !out.has_room() {
                            break;
                        }
                        if at > 0 {
                            out.push(", ");
                        }
                        push_coral_blob(out, bytes);
                    }
                    out.push("]");
                    push_frame_suffix(out, "bS");
                }
                AttributeValue::Null(flag) => push_field(out, "nULL", bool_text(*flag)),
                AttributeValue::Bool(flag) => push_field(out, "bOOL", bool_text(*flag)),
            },
        }
    }
}

/// How the rendering writes a boolean.
fn bool_text(flag: bool) -> &'static str {
    if flag {
        "true"
    } else {
        "false"
    }
}

/// One value's whole frame, with `field` carrying `value`.
fn push_field(out: &mut BoundedText, field: &'static str, value: &str) {
    push_frame_prefix(out, field);
    out.push(value);
    push_frame_suffix(out, field);
}

/// One `SS`/`NS` value: its frame around the texts the request listed.
fn push_text_list(out: &mut BoundedText, field: &'static str, texts: &[String]) {
    push_frame_prefix(out, field);
    out.push("[");
    for (at, text) in texts.iter().enumerate() {
        if !out.has_room() {
            break;
        }
        if at > 0 {
            out.push(", ");
        }
        out.push(text);
    }
    out.push("]");
    push_frame_suffix(out, field);
}

/// The frame's fields up to `field`, ending in `<field>=`: every field before it unset.
fn push_frame_prefix(out: &mut BoundedText, field: &str) {
    out.push("AttributeValue(");
    for declared in CORAL_ATTRIBUTE_VALUE_FIELDS {
        out.push(declared);
        out.push("=");
        if *declared == field {
            return;
        }
        out.push(NULL_FIELD);
        out.push(", ");
    }
}

/// The frame's fields AFTER `field`, each unset, closing the frame.
fn push_frame_suffix(out: &mut BoundedText, field: &str) {
    let mut past = false;
    for declared in CORAL_ATTRIBUTE_VALUE_FIELDS {
        if past {
            out.push(", ");
            out.push(declared);
            out.push("=");
            out.push(NULL_FIELD);
        }
        past = past || *declared == field;
    }
    out.push(")");
}

/// The fields the service's own attribute value object states, in the order it states
/// them. Six of them (`iNT`, `dECIMAL`, `dOUBLE` and the three set fields beside them)
/// belong to no member this model declares, so nothing a request sends can fill one; they
/// are part of the frame all the same, because the frame is what the refusal states.
const CORAL_ATTRIBUTE_VALUE_FIELDS: &[&str] = &[
    "s", "n", "b", "iNT", "dECIMAL", "dOUBLE", "sS", "nS", "bS", "iNTSET", "dECIMALSET",
    "dOUBLESET", "m", "l", "nULL", "bOOL",
];

/// A binary value in that rendering: the base64 the wire carries it as, which is the
/// value the request sent. Written through the bounded text, so the rendering of one blob
/// is the only encoding this walk holds at a time.
fn push_coral_blob(out: &mut BoundedText, bytes: &[u8]) {
    out.push(&::aws_smithy_types::base64::encode(bytes));
}

/// An `ExecuteTransaction`'s `TransactStatements` list as the refusal of that member
/// quotes it.
///
/// The same OBJECT rendering [`rendered_batch_write_request_items`] states a refused
/// structure value in, over the
/// statements this operation carries —
/// `[ParameterizedStatement(statement=<the statement the request sent>, parameters=null,
/// returnValuesOnConditionCheckFailure=null)]`. Every value in it comes from the request;
/// the field names and their order are the model's own
/// (`smithy_query(query="shape", shape="com.amazonaws.dynamodb#ParameterizedStatement")`
/// gives `Statement Parameters ReturnValuesOnConditionCheckFailure`), and a member the
/// request left out renders as the unset field it is. A parameter is an `AttributeValue`,
/// so each one is written in the attribute-value frame every value of this rendering is
/// written in.
///
/// Bounded exactly as those renderings are: every byte goes through one [`BoundedText`], so
/// what the clause quotes is clamped before it is produced rather than clipped afterwards.
pub fn rendered_parameterized_statements(statements: &[ParameterizedStatement]) -> String {
    let mut out = BoundedText::default();
    out.push("[");
    for (at, statement) in statements.iter().enumerate() {
        if !out.has_room() {
            break;
        }
        if at > 0 {
            out.push(", ");
        }
        out.push("ParameterizedStatement(statement=");
        out.push(&statement.statement);
        push_rendered_value_list(&mut out, "parameters", statement.parameters.as_deref());
        push_rendered_text(
            &mut out,
            "returnValuesOnConditionCheckFailure",
            statement.return_values_on_condition_check_failure.as_deref(),
        );
        out.push(")");
    }
    out.push("]");
    out.into_text()
}

/// One field carrying a LIST of attribute values: the bracketed form a list takes, each
/// element in the attribute-value frame, and the unset field for a member the request left
/// out.
fn push_rendered_value_list(
    out: &mut BoundedText,
    field: &str,
    values: Option<&[AttributeValue]>,
) {
    out.push(", ");
    out.push(field);
    out.push("=");
    let Some(values) = values else {
        out.push(NULL_FIELD);
        return;
    };
    out.push("[");
    for (at, value) in values.iter().enumerate() {
        if !out.has_room() {
            break;
        }
        if at > 0 {
            out.push(", ");
        }
        render(out, vec![RenderStep::Value(value)], 1);
    }
    out.push("]");
}

/// One field of such a frame carrying TEXT: `, <field>=<the text the request sent>`, and
/// the unset field for a member the request left out.
fn push_rendered_text(out: &mut BoundedText, field: &str, value: Option<&str>) {
    out.push(", ");
    out.push(field);
    out.push("=");
    match value {
        Some(text) => out.push(text),
        None => out.push(NULL_FIELD),
    }
}

/// A `TransactWriteItems`' `TransactItems` list as the refusal of that member quotes it.
///
/// The same OBJECT rendering [`rendered_batch_write_request_items`] states a refused
/// structure value in, over the actions this operation carries —
/// `[TransactWriteItem(conditionCheck=null, put=Put(item={<attribute>=AttributeValue(…)},
/// tableName=<the table the action named>, conditionExpression=null,
/// expressionAttributeNames=null, expressionAttributeValues=null,
/// returnValuesOnConditionCheckFailure=null, workloadProfileName=null), delete=null,
/// update=null), …]`. Every value in it comes from the request; the field names and their
/// order are the model's own (`smithy_query(query="shape",
/// shape="com.amazonaws.dynamodb#TransactWriteItem")` gives `ConditionCheck Put Delete
/// Update`, and the four action structures each declare their own members in the order
/// written below), a member the request left out renders as the unset field it is, and each
/// action carries the workload-profile field the service's own request objects have and this
/// model does not declare ([`TRAILING_REQUEST_FIELDS`]).
///
/// The transactional READ's own list is NOT quoted this way: its refusal states the member
/// and the bound alone (`tests/all/transact_items_cap_refusal.rs` holds the two side by
/// side), exactly as the batched read's request map is quoted differently from the batched
/// write's.
///
/// Bounded exactly as those renderings are: every byte goes through one [`BoundedText`], so
/// what the clause quotes is clamped before it is produced rather than clipped afterwards.
pub fn rendered_transact_write_items(items: &[TransactWriteItem]) -> String {
    let mut out = BoundedText::default();
    out.push("[");
    for (at, item) in items.iter().enumerate() {
        if !out.has_room() {
            break;
        }
        if at > 0 {
            out.push(", ");
        }
        out.push("TransactWriteItem(conditionCheck=");
        push_condition_check(&mut out, item.condition_check.as_ref());
        out.push(", put=");
        push_put(&mut out, item.put.as_ref());
        out.push(", delete=");
        push_delete(&mut out, item.delete.as_ref());
        out.push(", update=");
        push_update(&mut out, item.update.as_ref());
        out.push(")");
    }
    out.push("]");
    out.into_text()
}

/// One `ConditionCheck` action of that list, or the unset field for an element that states
/// another action.
fn push_condition_check(out: &mut BoundedText, check: Option<&ConditionCheck>) {
    let Some(check) = check else {
        out.push(NULL_FIELD);
        return;
    };
    out.push("ConditionCheck(key=");
    push_coral_item(out, &check.key);
    push_rendered_text(out, "tableName", Some(&check.table_name));
    push_rendered_text(
        out,
        "conditionExpression",
        Some(&check.condition_expression),
    );
    push_expression_fields(
        out,
        check.expression_attribute_names.as_ref(),
        check.expression_attribute_values.as_ref(),
        check.return_values_on_condition_check_failure.as_deref(),
    );
}

/// One `Put` action of that list, or the unset field.
fn push_put(out: &mut BoundedText, put: Option<&Put>) {
    let Some(put) = put else {
        out.push(NULL_FIELD);
        return;
    };
    out.push("Put(item=");
    push_coral_item(out, &put.item);
    push_rendered_text(out, "tableName", Some(&put.table_name));
    push_rendered_text(out, "conditionExpression", put.condition_expression.as_deref());
    push_expression_fields(
        out,
        put.expression_attribute_names.as_ref(),
        put.expression_attribute_values.as_ref(),
        put.return_values_on_condition_check_failure.as_deref(),
    );
}

/// One `Delete` action of that list, or the unset field.
fn push_delete(out: &mut BoundedText, delete: Option<&Delete>) {
    let Some(delete) = delete else {
        out.push(NULL_FIELD);
        return;
    };
    out.push("Delete(key=");
    push_coral_item(out, &delete.key);
    push_rendered_text(out, "tableName", Some(&delete.table_name));
    push_rendered_text(
        out,
        "conditionExpression",
        delete.condition_expression.as_deref(),
    );
    push_expression_fields(
        out,
        delete.expression_attribute_names.as_ref(),
        delete.expression_attribute_values.as_ref(),
        delete.return_values_on_condition_check_failure.as_deref(),
    );
}

/// One `Update` action of that list, or the unset field.
fn push_update(out: &mut BoundedText, update: Option<&Update>) {
    let Some(update) = update else {
        out.push(NULL_FIELD);
        return;
    };
    out.push("Update(key=");
    push_coral_item(out, &update.key);
    push_rendered_text(out, "updateExpression", Some(&update.update_expression));
    push_rendered_text(out, "tableName", Some(&update.table_name));
    push_rendered_text(
        out,
        "conditionExpression",
        update.condition_expression.as_deref(),
    );
    push_expression_fields(
        out,
        update.expression_attribute_names.as_ref(),
        update.expression_attribute_values.as_ref(),
        update.return_values_on_condition_check_failure.as_deref(),
    );
}

/// The fields every one of those four actions ends with: the two substitution tables, what
/// it asks for on a failed condition, the workload-profile field no request can set, and the
/// closing bracket.
fn push_expression_fields(
    out: &mut BoundedText,
    names: Option<&HashMap<String, String>>,
    values: Option<&Item>,
    return_values: Option<&str>,
) {
    push_rendered_name_map(out, "expressionAttributeNames", names);
    push_rendered_value_map(out, "expressionAttributeValues", values);
    push_rendered_text(out, "returnValuesOnConditionCheckFailure", return_values);
    out.push(", ");
    out.push(TRAILING_REQUEST_FIELDS);
    out.push(")");
}

/// One field carrying a substitution table of NAMES: `, <field>={<token>=<attribute name>}`,
/// and the unset field for a member the request left out. The entries are written in KEY
/// order, for the reason every map in these renderings is
/// ([`rendered_batch_write_request_items`]).
fn push_rendered_name_map(
    out: &mut BoundedText,
    field: &str,
    entries: Option<&HashMap<String, String>>,
) {
    out.push(", ");
    out.push(field);
    out.push("=");
    let Some(entries) = entries else {
        out.push(NULL_FIELD);
        return;
    };
    out.push("{");
    for (at, (token, name)) in entries
        .iter()
        .collect::<BTreeMap<_, _>>()
        .into_iter()
        .enumerate()
    {
        if !out.has_room() {
            break;
        }
        if at > 0 {
            out.push(", ");
        }
        out.push(token);
        out.push("=");
        out.push(name);
    }
    out.push("}");
}

/// One field carrying a substitution table of VALUES, each value in the attribute-value
/// frame every value of these renderings is written in — and the unset field for a member the
/// request left out.
fn push_rendered_value_map(out: &mut BoundedText, field: &str, entries: Option<&Item>) {
    out.push(", ");
    out.push(field);
    out.push("=");
    match entries {
        Some(entries) => push_coral_item(out, entries),
        None => out.push(NULL_FIELD),
    }
}

/// A `BatchGetItem`'s `RequestItems` map as the request carried it: the JSON rendering of
/// that member, which is the value a refusal of one of its KEYS quotes
/// ([`batch_table_key`]).
///
/// It is the request's own value re-rendered rather than the request bytes, which no
/// handler is handed: every member the request sent is written and no member it left out
/// is, each under the name the model spells it on the wire, with the entry members in the
/// order the model declares them (`smithy_query(query="shape",
/// shape="com.amazonaws.dynamodb#KeysAndAttributes")`). A map arrives as a `HashMap`, whose
/// iteration order is not the request's, so the table entries and an entry's substitution
/// names are written in NAME order: one submitted map renders one way, whatever order its
/// JSON arrived in.
///
/// Every VALUE is rendered by `serde_json` — which is also what escapes every string here —
/// and each KEY by the one renderer this crate states DynamoDB JSON with ([`item_json`],
/// which orders an item's attributes by name and encodes a blob the way the wire form does),
/// so a key quoted back in a refusal and an item written out by an export read the same way.
/// What is assembled here rather than by `serde_json` is the ORDER of an entry's members: a
/// `serde_json::Map` keeps insertion order only where the build resolves that crate's
/// `preserve_order` feature, which this crate's does not, so the five members would come out
/// alphabetically instead of as the model declares them.
pub fn rendered_batch_get_request_items(
    request_items: &HashMap<String, KeysAndAttributes>,
) -> String {
    let entries: Vec<String> = request_items
        .iter()
        .collect::<BTreeMap<_, _>>()
        .into_iter()
        .map(|(table, entry)| rendered_json_member(table, &rendered_keys_and_attributes(entry)))
        .collect();
    format!("{{{}}}", entries.join(","))
}

/// One `RequestItems` entry, as the JSON object the request carried: its members in the
/// order the model declares them, and only the ones it sent.
fn rendered_keys_and_attributes(entry: &KeysAndAttributes) -> String {
    let mut members: Vec<String> = vec![rendered_json_member(
        "Keys",
        &::serde_json::Value::Array(entry.keys.iter().map(item_json).collect()).to_string(),
    )];
    if let Some(attributes) = entry.attributes_to_get.as_deref() {
        members.push(rendered_json_member(
            "AttributesToGet",
            &::serde_json::json!(attributes).to_string(),
        ));
    }
    if let Some(consistent) = entry.consistent_read {
        members.push(rendered_json_member(
            "ConsistentRead",
            &::serde_json::json!(consistent).to_string(),
        ));
    }
    if let Some(projection) = entry.projection_expression.as_deref() {
        members.push(rendered_json_member(
            "ProjectionExpression",
            &::serde_json::json!(projection).to_string(),
        ));
    }
    if let Some(names) = entry.expression_attribute_names.as_ref() {
        // In name order, for the reason the entries above are: the map does not keep the
        // order the request wrote it in.
        let aliases: BTreeMap<&String, &String> = names.iter().collect();
        members.push(rendered_json_member(
            "ExpressionAttributeNames",
            &::serde_json::json!(aliases).to_string(),
        ));
    }
    format!("{{{}}}", members.join(","))
}

/// One member of a JSON object: `"<name>":<the rendered value>`, with the name escaped the
/// way `serde_json` escapes a string, so nothing here restates the escaping rules.
fn rendered_json_member(name: &str, rendered_value: &str) -> String {
    format!(
        "{}:{rendered_value}",
        ::serde_json::Value::String(name.to_string())
    )
}

/// How a batch that asks for more operations than it may carry is refused: AWS's
/// FIXED sentence for that quota, naming the call and neither the ceiling nor the
/// number asked for ([`MAX_BATCH_WRITE_REQUESTS`]/[`MAX_BATCH_GET_KEYS`] are internal
/// to the check).
pub fn too_many_batch_items(call: &str) -> RequestError {
    RequestError::Validation(format!("Too many items requested for the {call} call"))
}

/// The name the sentence above calls the batched WRITE by, which is the operation's own.
pub const BATCH_WRITE_CALL: &str = "BatchWriteItem";

/// The same for the batched READ.
pub const BATCH_GET_CALL: &str = "BatchGetItem";

/// How a member that must carry exactly one of a set of operations — a `WriteRequest`
/// (`PutRequest` or `DeleteRequest`) or a `TransactWriteItem` (`ConditionCheck`,
/// `Put`, `Delete` or `Update`) — is refused when it carries none of them, or more
/// than one: "You can only request one of these operations, not both, in a single
/// WriteRequest" (the `WriteRequest` doc).
pub fn exactly_one_operation(member: &str, choices: &str) -> RequestError {
    refused(format!(
        "Value at '{member}' failed to satisfy constraint: Member must contain exactly one of \
         {choices}"
    ))
}

/// The member a transaction's actions arrive in, as the request validator spells a member
/// in the paths it reports (lowerCamel) — the member a `TransactWriteItem` that states no
/// action at all violates.
pub const TRANSACT_ITEMS_MEMBER_PATH: &str = "transactItems";

/// The member a PartiQL transaction's statements arrive in, spelled the same way — the
/// member whose own `@length` maximum a list of more statements than the model admits
/// breaks (`smithy_query(query="operation", operation="ExecuteTransaction")` gives
/// `TransactStatements` with `@length{min:1,max:100}`).
pub const TRANSACT_STATEMENTS_MEMBER_PATH: &str = "transactStatements";

/// The constraint an element of a transactional write that names NONE of the four actions
/// broke, as AWS's request validator words the requirement: the shape that must carry one
/// of them and the four it may carry, in the order AWS lists them.
///
/// It is a constraint on the `TransactItems` MEMBER — a `TransactWriteItem` is a choice of
/// four optional members with nothing in the model to say one must be present
/// (`smithy_query(query="shape", shape="com.amazonaws.dynamodb#TransactWriteItem")` gives
/// `ConditionCheck Put Delete Update`, none of them `@required`) — so an element that
/// states none of them is an input-constraint violation this service finds itself and
/// reports as a clause like any other, counted in the `<N> validation error(s) detected: `
/// envelope beside every violation the model's own traits find
/// ([`transact_item_states_no_action_clause`]).
pub const TRANSACT_ITEM_ACTION_REQUIREMENT: &str =
    "TransactWriteRequest should contain Delete or Put or Update or ConditionCheck";

/// That violation as ONE clause of the envelope: the member the walk names it under, with
/// the empty value AWS echoes for an element that states nothing.
///
/// The one place the sentence is written, for both sides that can report it — the
/// aggregate the request's own constraint refusal counts, and the handler's refusal of a
/// request whose only violation this is — so the two cannot drift.
pub fn transact_item_states_no_action_clause() -> String {
    constraint_clause(
        TRANSACT_ITEMS_MEMBER_PATH,
        Some(""),
        TRANSACT_ITEM_ACTION_REQUIREMENT,
    )
}

/// That violation as the whole refusal, for the request whose ONLY violation it is: the
/// envelope over that one clause.
pub fn transact_item_states_no_action() -> RequestError {
    refused(transact_item_states_no_action_clause())
}

/// How a transaction that addresses one item twice is refused: "Transaction requests
/// cannot include multiple operations on one item" (the `TransactWriteItems` doc).
pub const DUPLICATE_TRANSACT_ITEM_MESSAGE: &str =
    "Transaction request cannot include multiple operations on one item";

impl Substitutions<'_> {
    /// Hold ONE ELEMENT's expressions and its substitution tables to each other — every
    /// token its expressions dereference defined, every entry it defines referenced — and
    /// state what is wrong with them as a refusal about an element is stated.
    ///
    /// The CHECK is the request-level one
    /// ([`Substitutions::check_referenced_within_element`]), because the rule is the same
    /// wherever the members sit and stating it twice would let the two drift. What differs
    /// is the ENVELOPE: `<N> validation error(s) detected: ` reports violations of the
    /// REQUEST's own members, which is what the request validator is handed, and the
    /// expression members of one `TransactItems` action or one `RequestItems` entry are
    /// members of that element — so the sentence an element earns is stated on its own,
    /// like every other refusal this service raises about an element
    /// ([`DUPLICATE_TRANSACT_ITEM_MESSAGE`], [`too_many_batch_items`]). What else differs
    /// is the one rule that is the REQUEST's: a table an element sends with no expression
    /// of its own is admitted, because it is the request validator that holds a call's own
    /// expression members to the tables beside them.
    pub fn check_all_referenced_in_element(
        &self,
        expressions: &[(&str, Option<&str>)],
    ) -> Result<(), RequestError> {
        self.check_referenced_within_element(expressions)
            .map_err(stated_outside_the_envelope)
    }
}

/// One refusal the expressions of ONE ELEMENT of a multi-item request earn when they are
/// PARSED, APPLIED or EVALUATED, as that element states it: outside the
/// `<N> validation error(s) detected: ` envelope.
///
/// The same rule [`Substitutions::check_all_referenced_in_element`] states, over the
/// refusals that come after that check — an expression past the documented size ceiling,
/// one the grammar cannot complete, an operand of the wrong type, two actions writing one
/// location: the envelope reports violations of the REQUEST's own members, and the
/// expression members of one `TransactItems` action are members of that element, so an
/// element's refusal is stated on its own whichever member it names. The members a READ
/// narrows its result with are already stated on their own wherever they sit
/// (`crate::state`'s enveloped-members rule), so this changes nothing for them.
pub fn refused_in_element(error: RequestError) -> RequestError {
    stated_outside_the_envelope(error)
}

/// One refusal, stated outside the `<N> validation error(s) detected: ` envelope.
///
/// The envelope over an EMPTY clause is exactly the text it puts in front of a clause, so
/// the prefix comes from the one function that writes it ([`validation_message`]) rather than
/// being spelled a second time here. A refusal that carries no envelope is already stated on
/// its own and is left exactly as it came.
fn stated_outside_the_envelope(error: RequestError) -> RequestError {
    match error {
        RequestError::Validation(message) => {
            let envelope = validation_message([String::new()]).unwrap_or_default();
            let clause = message.strip_prefix(&envelope).map(str::to_string);
            RequestError::Validation(clause.unwrap_or(message))
        }
        other => other,
    }
}

/// How a transaction that no item of could be applied is reported: the
/// `TransactionCanceledException` message, which lists the per-action reason codes the
/// `CancellationReasons` member carries in the same order ("Cancellation reason codes
/// and possible error messages" in the `TransactWriteItems` doc).
pub fn transaction_cancelled_message(codes: &[&str]) -> String {
    format!(
        "Transaction cancelled, please refer cancellation reasons for specific reasons [{}]",
        codes.join(", ")
    )
}

/// The `CancellationReason$Code` an action that nothing was wrong with carries: "None
/// - No conditions ... were violated" (the `CancellationReason` doc).
pub const CANCELLATION_CODE_NONE: &str = "None";

/// The `CancellationReason$Code` an action whose condition was not met carries.
pub const CANCELLATION_CODE_CONDITION_FAILED: &str = "ConditionalCheckFailed";

/// The `CancellationReason$Code` an action the transaction refused on its own merits
/// carries: "ValidationError - Code: ValidationError, Message: One or more parameter
/// values were invalid" ("Cancellation reason codes and possible error messages", the
/// `TransactWriteItems` doc). A failure the transaction decides PER ACTION is reported
/// this way rather than as a bare `ValidationException`, which is how a malformed
/// REQUEST — one addressing an item twice, or an entry naming no operation — is
/// refused.
pub const CANCELLATION_CODE_VALIDATION_ERROR: &str = "ValidationError";

/// The `CancellationReason$Code` an action whose own table could not be resolved carries:
/// the element names a resource the service does not hold, which is a failure of that
/// element and not of the request, so the transaction is cancelled with this reason rather
/// than answered a top-level lookup failure ([`cancelled_element_reasons`]).
pub const CANCELLATION_CODE_RESOURCE_NOT_FOUND: &str = "ResourceNotFound";

/// The code a statement that would store an item at a key ALREADY HOLDING one reports:
/// a PartiQL `INSERT` is a put that must not replace, so its failure is a duplicate
/// primary key rather than a condition the statement stated.
///
/// It is the code whichever PartiQL surface ran the statement — a slot's
/// `BatchStatementError$Code` in a batch and a `CancellationReason$Code` in a transaction
/// — so the two cannot disagree about what a key collision IS.
pub const DUPLICATE_ITEM_CODE: &str = "DuplicateItem";

/// The sentence that collision is stated with, beside [`DUPLICATE_ITEM_CODE`].
pub const DUPLICATE_ITEM_MESSAGE: &str = "Duplicate primary key exists in table";

/// The clause every refusal of a parameter value in this service opens with, which the
/// documented `ValidationError` cancellation reason also states ("Message: One or more
/// parameter values were invalid").
pub const INVALID_PARAMETER_CLAUSE: &str = "One or more parameter values were invalid";

/// WHICH side decided a key the request carried is no key of the table it addressed.
///
/// A transactional element's key is read against the TABLE it names, so which of the two
/// answers a transaction gives for a bad key follows from WHAT was wrong with it:
///
/// * [`Self::Schema`] — the value is a well-formed attribute value and the table's KEY
///   SCHEMA is what does not admit it: a type the schema does not declare for that
///   attribute, a value past the size limit of the role the schema gives it, an attribute
///   the schema declares no key for, or a key element a `Key`-shaped member leaves out.
///   Deciding it needs the table, so it is a refusal OF THAT ELEMENT and the transaction
///   cancels with the documented `ValidationError` reason
///   ([`cancelled_by_element_reasons`]).
/// * [`Self::Request`] — the member the request sent is not shaped like a key for any
///   schema to be read against: a member carrying no attribute at all, an `Item` that
///   gives no value for a key attribute, a value that is no value of its own type (a
///   number whose text is not a number), or an empty key value. Deciding it is deciding
///   about the REQUEST, so the whole call is refused with that `ValidationException` and
///   nothing is cancelled.
///
/// The SENTENCE is the same either way — each path words its refusal exactly as the
/// single-item one does — so what this carries is only which answer states it.
#[derive(Debug, Clone)]
pub enum KeyRefusal {
    Schema(RequestError),
    Request(RequestError),
}

impl KeyRefusal {
    /// Whether the element that carried this key is what a transaction cancels for it.
    pub fn cancels(&self) -> bool {
        matches!(self, Self::Schema(_))
    }

    /// The refusal itself, for a surface that words both classes the same way.
    pub fn into_error(self) -> RequestError {
        match self {
            Self::Schema(error) | Self::Request(error) => error,
        }
    }
}

impl From<KeyRefusal> for RequestError {
    fn from(refusal: KeyRefusal) -> Self {
        refusal.into_error()
    }
}

/// What a transaction reports when IT refused ONE of its elements while preparing that
/// element against its table: the `TransactionCanceledException` message and the
/// `CancellationReasons`, which name the element at `at` with
/// [`CANCELLATION_CODE_VALIDATION_ERROR`] and `detail`, and every other element of
/// `elements` with [`CANCELLATION_CODE_NONE`] and no message, since nothing was wrong
/// with those ("Cancellation reason codes and possible error messages").
///
/// `detail` is stated VERBATIM — the sentence the same defect earns on the single-item
/// path, whatever that sentence is. The documented reason shows one such sentence
/// ("Message: One or more parameter values were invalid"), which is the clause a refusal
/// of a parameter VALUE opens with and which those sentences already carry
/// ([`INVALID_PARAMETER_CLAUSE`]); a reason is not a second wording, so nothing is added
/// to a sentence that does not. Every transactional operation that prepares elements
/// against a table reaches this, so the documented reason shape has one implementation.
pub fn cancelled_by_element_reasons(
    at: usize,
    elements: usize,
    detail: &str,
) -> (String, Vec<CancellationReason>) {
    cancelled_element_reasons(at, elements, CANCELLATION_CODE_VALIDATION_ERROR, detail)
}

/// The same for a cancellation whose failing element carries a reason code of its own:
/// `code` is the `CancellationReason$Code` the element at `at` reports, beside `detail`,
/// and every other element of `elements` still reports [`CANCELLATION_CODE_NONE`] with no
/// message.
///
/// The message lists the codes in the same order, which is what makes the top-level sentence
/// name the reason the caller then finds in `CancellationReasons`.
pub fn cancelled_element_reasons(
    at: usize,
    elements: usize,
    code: &str,
    detail: &str,
) -> (String, Vec<CancellationReason>) {
    let codes: Vec<&str> = (0..elements)
        .map(|element| {
            if element == at {
                code
            } else {
                CANCELLATION_CODE_NONE
            }
        })
        .collect();
    let reasons: Vec<CancellationReason> = codes
        .iter()
        .enumerate()
        .map(|(element, code)| CancellationReason {
            code: Some((*code).to_string()),
            message: (element == at).then(|| detail.to_string()),
            item: None,
        })
        .collect();
    (transaction_cancelled_message(&codes), reasons)
}

/// What a transactional read or write costs relative to the same single-item one:
/// "Transactional write requests require two write capacity units" and "Transactional
/// read requests require two read capacity units to perform one read per second for
/// items up to 4 KB" ("Read/write capacity mode", Amazon DynamoDB Developer Guide).
pub const TRANSACTIONAL_CAPACITY_FACTOR: f64 = 2.0;

/// How long a `ClientRequestToken` keeps a completed transaction idempotent: "A client
/// request token is valid for 10 minutes after the first request that uses it is
/// completed. After 10 minutes, any request with the same client token is treated as a
/// new request" (`TransactWriteItems$ClientRequestToken`).
pub const TRANSACTION_IDEMPOTENCY_WINDOW: Duration = Duration::from_secs(10 * 60);

/// How a repeat of a `ClientRequestToken` carrying different actions is refused.
///
/// The modelled error says what happened — "DynamoDB rejected the request because you
/// retried a request with a different payload but with an idempotent token that was
/// already used" (`IdempotentParameterMismatchException`) — and the TRANSACTIONAL
/// operations state it in the one fixed sentence below, which names the token's window and
/// nothing of the request that was sent. Shared by both of them, because to them it is one
/// refusal; the bulk-operation surface words the same modelled error its own way
/// ([`super::BULK_OPERATION_IDEMPOTENT_PARAMETER_MISMATCH_MESSAGE`]), so each surface's
/// sentence is stated where that surface's rules live.
pub const IDEMPOTENT_PARAMETER_MISMATCH_MESSAGE: &str =
    "Specified idempotent token was used with different request parameters within the idempotency \
     window";

/// How many `ClientRequestToken`s this emulator remembers at once.
///
/// The token is caller-chosen, so the number of DISTINCT tokens one client can present
/// inside [`TRANSACTION_IDEMPOTENCY_WINDOW`] is caller-chosen too; like every other
/// growth site in this store ([`MAX_STORE_SIZE_BYTES`], [`MAX_REGIONS`]) it is budgeted,
/// so what the store retains is not a function of what a caller sends. This emulator
/// CHOOSES 1024: far more than a client retrying a
/// handful of transactions needs, and bounded. Each record is a constant size — a
/// digest of the actions, not the actions ([`action_fingerprint`]) — plus the capacity
/// figures of at most one transaction, and the OLDEST record is dropped once the
/// ceiling is reached ([`ComAmazonawsDynamodbDynamodb20120810State::record_transaction`]).
pub const MAX_RECORDED_TRANSACTIONS: usize = 1024;

/// The fingerprint a `ClientRequestToken`'s actions are remembered by: the SHA-256 of
/// their canonical encoding, so what the store retains per token is a constant 32 bytes
/// however large the transaction was, and two calls' actions still compare equal
/// exactly when their canonical encodings do.
pub fn action_fingerprint(canonical: &str) -> String {
    use sha2::Digest;
    hex::encode(sha2::Sha256::digest(canonical.as_bytes()))
}

/// What an omitted `Return*` member stands for: the value the service defaults it to —
/// "If set to NONE (the default), no statistics are returned"
/// (`TransactWriteItems$ReturnItemCollectionMetrics`), and `NONE` is likewise the
/// `ReturnConsumedCapacity` level that reports nothing ("NONE - No ConsumedCapacity
/// details are included in the response"). A request that omits the member and one that
/// spells that value therefore ask for the same response.
const DEFAULT_REPORTING_LEVEL: &str = "NONE";

/// How the REPORTING options of a request are encoded into the fingerprint its
/// `ClientRequestToken` is remembered by, in the order the caller lists them.
///
/// A token stands for the whole request it was first used with — it is refused when
/// "you retried a request with a different payload but with an idempotent token that
/// was already used" (`IdempotentParameterMismatchException`) — so every member of that
/// payload is part of the fingerprint, the ones choosing what the response reports
/// included: a retry under the same token that asks for a different level of
/// `ReturnConsumedCapacity`, or that turns `ReturnItemCollectionMetrics` on, is a
/// different request and earns [`IDEMPOTENT_PARAMETER_MISMATCH_MESSAGE`] rather than a
/// replay. An omitted member encodes as the value it defaults to
/// ([`DEFAULT_REPORTING_LEVEL`]), so leaving it out and spelling that value are one
/// request.
pub fn canonical_reporting_options(stated: &[Option<&str>]) -> String {
    let mut text = String::new();
    for option in stated {
        text.push_str(option.unwrap_or(DEFAULT_REPORTING_LEVEL));
        text.push('|');
    }
    text
}

/// A transaction that has already been applied under a `ClientRequestToken`.
///
/// "Providing a ClientRequestToken makes the call to TransactWriteItems idempotent,
/// meaning that multiple identical calls have the same effect as one single call"
/// (`TransactWriteItems$ClientRequestToken`): a repeat inside
/// [`TRANSACTION_IDEMPOTENCY_WINDOW`] applies NOTHING and answers what the first call
/// answered, and a repeat of the token with different actions is the documented
/// `IdempotentParameterMismatchException` — which is what [`Self::actions`], the
/// fingerprint of the actions the first call carried, is compared against.
#[derive(Debug, Clone)]
pub struct CompletedTransaction {
    /// The fingerprint of the actions the first call under this token carried
    /// ([`action_fingerprint`] over [`canonical_item_text`]), which is a constant size
    /// whatever the transaction carried.
    pub actions: String,
    /// When that call completed: the idempotency window is measured from here.
    pub completed_at: SystemTime,
    /// The capacity and item-collection figures that call reported, so a repeat is
    /// answered from what the transaction DID rather than re-applied.
    pub ledger: WriteLedger,
}

/// The capacity and item-collection figures a multi-item WRITE reports, totalled per
/// table.
///
/// `BatchWriteItem` and `TransactWriteItems` both report "the capacity units consumed
/// by the entire operation" as one `ConsumedCapacity` per table and item-collection
/// metrics as a list per table, so both accumulate into this as each item lands and
/// project it once at the end.
#[derive(Debug, Clone, Default)]
pub struct WriteLedger {
    capacity: BTreeMap<String, WriteCapacity>,
    metrics: BTreeMap<String, Vec<ItemCollectionMetrics>>,
}

impl WriteLedger {
    /// Add what one item write consumed on `table`.
    pub fn record(&mut self, table: &str, capacity: &WriteCapacity) {
        let total = self.capacity.entry(table.to_string()).or_default();
        total.table += capacity.table;
        for (index, units) in &capacity.local_secondary_indexes {
            *total
                .local_secondary_indexes
                .entry(index.clone())
                .or_default() += units;
        }
        for (index, units) in &capacity.global_secondary_indexes {
            *total
                .global_secondary_indexes
                .entry(index.clone())
                .or_default() += units;
        }
    }

    /// Add the item collection one item write landed in, when the table has one.
    pub fn record_metrics(&mut self, table: &str, metrics: Option<ItemCollectionMetrics>) {
        if let Some(metrics) = metrics {
            self.metrics
                .entry(table.to_string())
                .or_default()
                .push(metrics);
        }
    }

    /// Scale the TABLE's own figure by `factor` — what a TRANSACTIONAL write consumes
    /// relative to the same single-item write ([`TRANSACTIONAL_CAPACITY_FACTOR`]).
    ///
    /// The factor is the cost of the ITEM write: "transactional write requests require two
    /// write capacity units to perform one write per second for items up to 1 KB" ("Read/write
    /// capacity mode", Amazon DynamoDB Developer Guide) — a statement about the item, so the
    /// secondary-index entries the same write projects are charged what one write of them
    /// costs and are left as they were recorded. It is the same half of the breakdown the
    /// factor is applied to when a transactional write is REFUSED
    /// ([`crate::state::charge_refused_transactional_write`]).
    pub fn scale_table_units(&mut self, factor: f64) {
        for capacity in self.capacity.values_mut() {
            capacity.table *= factor;
        }
    }

    /// What this ledger holds, table by table: the figures the operation reports as its
    /// `ConsumedCapacity`, itemised the way that member itemises them.
    ///
    /// They are also the figures the operation COUNTS into the table's consumption series
    /// ([`crate::state::charge_reported_ledger`]), so what a client reads out of CloudWatch
    /// and what the response named cannot differ.
    pub fn itemised(&self) -> impl Iterator<Item = (&str, &WriteCapacity)> {
        self.capacity
            .iter()
            .map(|(table, capacity)| (table.as_str(), capacity))
    }

    /// The `ConsumedCapacity` list the operation reports, one entry per table it wrote
    /// to, or `None` when the request asked for no capacity detail.
    ///
    /// `kind` is the capacity the operation BILLS these figures as
    /// ([`crate::state::CapacityKind`]): the write it performed, or the read a
    /// `ClientRequestToken` replay bills instead of re-applying the transaction.
    pub fn consumed_capacity(
        &self,
        requested: Option<&str>,
        kind: CapacityKind,
        report: KindReport,
    ) -> Option<Vec<ConsumedCapacity>> {
        let reported: Vec<ConsumedCapacity> = self
            .capacity
            .iter()
            .filter_map(|(table, capacity)| {
                consumed_write_capacity(requested, table, capacity, kind, report)
            })
            .collect();
        (!reported.is_empty()).then_some(reported)
    }

    /// The `ItemCollectionMetrics` map the operation reports, or `None` when no write
    /// landed in a table that has an item collection to report.
    pub fn item_collection_metrics(&self) -> Option<HashMap<String, Vec<ItemCollectionMetrics>>> {
        (!self.metrics.is_empty()).then(|| {
            self.metrics
                .iter()
                .map(|(table, metrics)| (table.clone(), metrics.clone()))
                .collect()
        })
    }
}

/// The capacity a multi-item READ reports, totalled per table, for the same reason
/// [`WriteLedger`] exists: `BatchGetItem` and `TransactGetItems` both report one
/// `ConsumedCapacity` per table.
#[derive(Debug, Clone, Default)]
pub struct ReadLedger {
    units: BTreeMap<String, f64>,
}

impl ReadLedger {
    /// Add what one read consumed on `table`.
    pub fn record(&mut self, table: &str, units: f64) {
        *self.units.entry(table.to_string()).or_default() += units;
    }

    /// The `ConsumedCapacity` list the operation reports, one entry per table it read.
    pub fn consumed_capacity(
        &self,
        requested: Option<&str>,
        report: KindReport,
    ) -> Option<Vec<ConsumedCapacity>> {
        let reported: Vec<ConsumedCapacity> = self
            .units
            .iter()
            .filter_map(|(table, units)| consumed_capacity(requested, table, *units, report))
            .collect();
        (!reported.is_empty()).then_some(reported)
    }
}

/// `item`, encoded so that two requests carrying the same item encode identically
/// whatever order their JSON maps — or their set members — arrived in: attribute names
/// in order, every value tagged by its type, sets sorted by their encoded members.
///
/// This is what makes a repeated `ClientRequestToken` comparable
/// ([`CompletedTransaction::actions`]); it is never reported to a caller.
///
/// The walk is ITERATIVE over an explicit stack, like [`attribute_value_size`] and
/// [`canonicalize_attribute_value`] and for the same reason: nesting in a request is
/// chosen by the caller, so recursing on it would make stack use a function of the
/// request.
pub fn canonical_item_text(item: &Item) -> String {
    let mut text = String::new();
    let mut work: Vec<CanonicalStep<'_>> = Vec::new();
    push_attributes(item, &mut work);
    while let Some(step) = work.pop() {
        match step {
            // The closing delimiter of a collection whose members are already on the
            // stack above it, so it is emitted after the last of them.
            CanonicalStep::Literal(literal) => text.push_str(literal),
            CanonicalStep::Named(name, value) => {
                text.push_str(&format!("{}={};", name.len(), name));
                work.push(CanonicalStep::Value(value));
            }
            CanonicalStep::Value(value) => match value {
                AttributeValue::S(string) => {
                    text.push_str(&format!("S{}:{string};", string.len()));
                }
                AttributeValue::N(number) => text.push_str(&format!("N:{number};")),
                AttributeValue::B(bytes) => {
                    text.push_str(&format!("B:{};", hex::encode(bytes)));
                }
                AttributeValue::Ss(members) => {
                    append_canonical_set('S', members.clone(), &mut text);
                }
                AttributeValue::Ns(members) => {
                    append_canonical_set('N', members.clone(), &mut text);
                }
                AttributeValue::Bs(members) => {
                    append_canonical_set('B', members.iter().map(hex::encode).collect(), &mut text);
                }
                AttributeValue::Null(is_null) => text.push_str(&format!("NULL:{is_null};")),
                AttributeValue::Bool(flag) => text.push_str(&format!("BOOL:{flag};")),
                AttributeValue::L(members) => {
                    text.push_str("L[");
                    work.push(CanonicalStep::Literal("];"));
                    for member in members.iter().rev() {
                        work.push(CanonicalStep::Value(member));
                    }
                }
                AttributeValue::M(map) => {
                    text.push_str("M[");
                    work.push(CanonicalStep::Literal("];"));
                    push_attributes(map, &mut work);
                }
            },
        }
    }
    text
}

/// One step of the canonical walk above: a value to encode, one attribute's name and
/// value, or a literal already decided (a collection's closing delimiter).
enum CanonicalStep<'v> {
    Literal(&'static str),
    Named(&'v str, &'v AttributeValue),
    Value(&'v AttributeValue),
}

/// Push every attribute of `item` so that they POP in name order: the canonical form
/// is the same whatever order the request's JSON map arrived in.
fn push_attributes<'v>(item: &'v Item, work: &mut Vec<CanonicalStep<'v>>) {
    for (name, value) in item.iter().collect::<BTreeMap<_, _>>().into_iter().rev() {
        work.push(CanonicalStep::Named(name, value));
    }
}

/// One set, appended with its members in order: a set is unordered, so two requests
/// that sent the same members in different orders encode identically.
fn append_canonical_set(tag: char, mut members: Vec<String>, text: &mut String) {
    members.sort();
    text.push_str(&format!("{tag}[{}];", members.join(",")));
}
