// SPDX-License-Identifier: CC0-1.0

//! The one half of a constraint refusal the framework's walk cannot state — the
//! COLLECTION a clause about a collection member is about — served as the per-operation
//! wrapper that is the only layer able to state it.
//!
//! # The rule
//!
//! A value outside a constraint the model declares is refused before any handler runs,
//! over one clause per violation, each naming the value that was rejected, the member path
//! it sat at and the requirement it failed. The value is the only place the request's own
//! input is reported back, so a client comparing it against what it sent has to see its
//! own input.
//!
//! A `@length` on a map or a list is checked ON THE MEMBER, before the walk descends into
//! the entries, so the only rendering the framework has for the rejected value is a
//! SUMMARY of its size — `<n> items` for a list, `<n> entries` for a map — which is not a
//! value any caller sent (`vendor/smithy-schema-server/src/constraints.rs`). AWS states
//! the collection itself there: a map as `{<key>=<value>, …}`, a list as `[<element>, …]`.
//!
//! A constraint declared on a map's VALUE SHAPE is the other half: it is checked on each
//! ENTRY, so the framework reports it at the entry (`<map member>.<key>`) with the one
//! constraint that failed, where AWS reports it at the MAP MEMBER — quoting the value that
//! member held and listing the value shape's WHOLE declared constraint set, so one offending
//! map is ONE validation error however many of its entries, or of one value's constraints,
//! were broken. A service whose recordings show that states it
//! ([`StatedClause::grouping_map_values`]); one that does not keeps the framework's
//! per-entry clause.
//!
//! # Why a wrapper
//!
//! [`Service::override_refusal`](smithy_server_runtime::Service::override_refusal) is
//! handed the violations alone and never the request they were found in, so that value
//! cannot be recovered there. [`QuotingSubmittedCollections`] is where it is: a
//! per-operation wrapper, which has the request and can re-read it.
//!
//! Nothing is re-implemented and no check is repeated in other words: the violations and
//! the submitted collections come from this crate's shared collecting walk
//! ([`crate::validation::refusing_violations_with_mapped_echoes`], which runs the
//! protocol's own deserializer and the framework's own `constraints::validate` and owns
//! every bound on what one clause may quote), the restoring rule is
//! [`crate::validation_message::quoting_the_submitted_collection`], each clause is the
//! framework's own `Violation::message`, the envelope around them is the framework's own,
//! and the status and the wire envelope come from the framework's refusal channel. A
//! refusal with NO such clause is left to that channel untouched.
//!
//! # What a service states, and what it does not
//!
//! THE HOME for this wrapper: a service states only the facts that are its own — the error
//! CODE its refusals carry, the ORDER it states a map's entries in, how it answers a
//! rendered response, and which of its operations are served through the pass
//! ([`StatedClauseTable`]). Everything else is here, so two services cannot disagree about
//! how one collection is quoted.
//!
//! A map's entries have an order on the wire, none in the value the codec builds from it (a
//! string map arrives as a `HashMap`), and possibly a third in the clause the service
//! states — so the shared walk, which reads the BUILT input, can only state one map in key
//! order. The order the request wrote is still in the request, which this layer has, so a
//! service that states another order supplies it as a [`MapEntryOrder`] over the entries
//! read back from the request ([`submitted_map_entries_at`], at the member path the clause
//! names, so a map declared on a nested structure member is reached too);
//! [`in_submitted_order`] is the order the request carried, which is what a service whose
//! recordings show the submitted order passes, and [`in_hashed_entry_order`] the order a
//! layer holding that map in a hash table keyed on each entry's own key walks it in.
//!
//! A service that words its constraint refusal in its own PRE-DISPATCH layer, rather than
//! through the wrapper, states the same one fact and reaches the same restating through
//! [`quoting_submitted_maps`]: such a layer holds the request but has no echo to restate
//! from, so the map is quoted from the body under the shared echo ceiling. Both paths share
//! the guards ([`restated_map`]) and the rendering, so neither can word one collection
//! differently from the other.
//!
//! What this layer may NOT do is enlarge the clause: every bound on what one message quotes
//! belongs to the shared walk, so a restated map is taken only when it renders to exactly
//! the length that walk had already decided to carry — the same entries in another order,
//! and therefore the same bytes. A clause that fails any of the guards keeps the shared
//! walk's own rendering: a value that is not a map (a list, whose order the built value does
//! keep), a member the body carries no map of strings at, a body this cannot read, a member
//! whose entry count differs from the one the walk counted, and an order the service's own
//! rule declines to state. A clause stated at a map MEMBER is bounded the same way: it quotes
//! the map up to the ceiling the shared walk quotes one clause to
//! ([`crate::validation::quoted_echo`]).
//!
//! The request is read AT MOST ONCE PER MEMBER PATH per refusal, and for at most a fixed
//! number of paths ([`SubmittedMaps`], [`MAX_PARSED_PATHS`]), the negative answer included —
//! so both the parses one refusal performs and the keys it retains are bounded by constants
//! rather than by the violation count a request chooses. That bound is what keeps a clause
//! stated at `<map member>.<key>`, whose last step is the caller's own text, from being able
//! to drive one parse per entry sent.

use std::collections::{HashMap, HashSet};

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use aws_smithy_schema::Schema;
use aws_smithy_schema_server::constraints::ViolationKind;
use smithy_server_runtime::{
    BoxFuture, ErasedOperation, Operation, OperationContext, OperationRoutingTable, Refused,
    Service, Violation,
};

use crate::validation::{quoted_echo, refusing_violations_with_mapped_echoes, rendered_map};
use crate::validation_message::{
    element_constraint_set, envelope_over, map_entry_count, map_value_set_requirement,
    quoting_the_submitted_collection, requirement,
};

/// The order a service states a map's entries in inside a clause, over the entries as the
/// REQUEST carried them — or `None` where that service's own rule declines to state an
/// order for them, which keeps the shared walk's own rendering.
pub type MapEntryOrder = fn(&[(String, String)]) -> Option<Vec<(&str, &str)>>;

/// The entries in the order the request submitted them: the order the caller's own JSON
/// object wrote them.
///
/// The [`MapEntryOrder`] of a service whose recordings quote a map in the order it was
/// sent, which is the order the validating layer in front of such a service iterates a
/// map that KEEPS its insertion order.
#[must_use]
pub fn in_submitted_order(entries: &[(String, String)]) -> Option<Vec<(&str, &str)>> {
    Some(
        entries
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str()))
            .collect(),
    )
}

/// The entries in the order the HASH TABLE holding them is walked, for the
/// [`MapEntryOrder`] of a service whose layer holds a submitted map in a table keyed on
/// each entry's own key and states it by enumerating that table rather than by sorting it:
///
/// * in ascending bucket, the bucket being the low bits of the key's string hash
///   ([`map_entry_bucket`]) at the table's own width ([`map_entry_table_buckets`]);
/// * two keys of ONE bucket in KEY order, which is the order the bucket's chain was filled
///   in — the table is filled from the map's entries in key order, so a bucket's chain
///   carries them in that order too.
///
/// The order is a function of the KEYS and of the entry count alone — a member, an
/// operation and a depth do not enter it, and the order the request wrote its entries in
/// does not either — so one map is stated the same way wherever it is carried, and a
/// handler that reads the map it already holds reaches the same order
/// ([`hashed_entry_order`]).
#[must_use]
pub fn in_hashed_entry_order(entries: &[(String, String)]) -> Option<Vec<(&str, &str)>> {
    let held: Vec<(&str, &str)> = entries
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect();
    Some(hashed_entry_order(&held))
}

/// `entries` in the order [`in_hashed_entry_order`] states them, for a caller holding the
/// entries as borrowed pairs — a handler wording its own clause about a map it has already
/// read, which reaches the same order as a clause restated from the request because the
/// order reads nothing but the keys.
#[must_use]
pub fn hashed_entry_order<'e>(entries: &[(&'e str, &'e str)]) -> Vec<(&'e str, &'e str)> {
    let buckets = map_entry_table_buckets(entries.len());
    let mut stated: Vec<(&str, &str)> = entries.to_vec();
    stated.sort_by_key(|(key, _)| (map_entry_bucket(key, buckets), *key));
    stated
}

/// How many buckets the table holding a map's entries starts with, before it has grown.
pub const MAP_ENTRY_TABLE_MIN_BUCKETS: usize = 16;

/// How wide the table holding `entries` entries is: it starts at
/// [`MAP_ENTRY_TABLE_MIN_BUCKETS`] and doubles whenever the entries put into it pass three
/// quarters of its width, so the width is a power of two decided by the entry COUNT alone.
#[must_use]
pub fn map_entry_table_buckets(entries: usize) -> usize {
    let mut buckets = MAP_ENTRY_TABLE_MIN_BUCKETS;
    // `buckets` is a power of two, so three quarters of it is exact; the guard keeps the
    // doubling inside `usize` for an entry count no request could carry anyway.
    while buckets / 4 * 3 < entries && buckets <= usize::MAX / 2 {
        buckets *= 2;
    }
    buckets
}

/// Which bucket of a table of `buckets` buckets the entry keyed `key` sits in: the key's
/// string hash ([`crate::validation_message::string_hash`]) with its high half folded into
/// its low half, reduced to the table's width.
///
/// The fold is what makes a table narrower than the hash separate keys that differ only
/// above its width. `buckets` is a power of two ([`map_entry_table_buckets`]), so the
/// reduction is the low bits of the folded hash.
#[must_use]
pub fn map_entry_bucket(key: &str, buckets: usize) -> usize {
    let hash = crate::validation_message::string_hash(key);
    let folded = hash ^ (hash >> 16);
    (folded as usize) & (buckets - 1)
}

/// How a service renders a `@pattern` expression inside a requirement sentence it states:
/// the expression as the MODEL writes it (`str::to_string`), or without the whole-value
/// anchors a model adds to express the match AWS's own pattern makes implicitly
/// ([`crate::validation::unanchored`]) for a service whose recordings drop them. The CHECK
/// always ran the model's expression as written; this is the rendering alone.
pub type PatternRendering = fn(&str) -> String;

/// How a service answers a response one of its layers rendered — the seam a service whose
/// wire form differs from the one a renderer wrote states that difference at (the member an
/// error body carries its text in, say).
pub type AnsweredAs = fn(RtResponse) -> RtResponse;

/// A response answered exactly as its renderer wrote it: the [`AnsweredAs`] of a service
/// that restates nothing.
#[must_use]
pub fn as_rendered(resp: RtResponse) -> RtResponse {
    resp
}

/// The facts ONE service states about its own constraint clauses: the error code they
/// carry, the order it states a map's entries in, and how it answers a rendered response.
#[derive(Clone, Copy)]
pub struct StatedClause {
    /// The error code a constraint refusal of this service carries — the one its own
    /// refusal seam picks for the class, because the restated message has to travel with
    /// the code it would have travelled with: a client branches on the code.
    pub refusal_code: &'static str,
    /// The order this service states a map's entries in ([`MapEntryOrder`]).
    pub entry_order: MapEntryOrder,
    /// How this service answers a rendered response ([`AnsweredAs`]).
    pub answered_as: AnsweredAs,
    /// How this service renders a `@pattern` inside the constraint set it states at a MAP
    /// member for a constraint that map's VALUES broke, or `None` where it keeps the
    /// framework's per-entry clause instead ([`StatedClause::grouping_map_values`]).
    pub map_value_patterns: Option<PatternRendering>,
}

impl StatedClause {
    /// The clause of a service that quotes a map in the order the request submitted it and
    /// answers a rendered response unchanged, under `refusal_code`.
    #[must_use]
    pub fn under(refusal_code: &'static str) -> Self {
        Self {
            refusal_code,
            entry_order: in_submitted_order,
            answered_as: as_rendered,
            map_value_patterns: None,
        }
    }

    /// The same clause, stating a constraint broken by one of a MAP's values at the map
    /// MEMBER — the whole map as the value, the value shape's complete declared constraint
    /// set as the requirement — with a `@pattern` in that set rendered as `pattern` renders
    /// one.
    ///
    /// For a service whose recordings show that clause. The grouping, the set (read off the
    /// operation's input schema, so it is the MODEL's constraints and not the one that
    /// happened to fail) and every sentence in it stay here; what a service chooses is
    /// whether it states that clause at all and how the expression is quoted.
    #[must_use]
    pub fn grouping_map_values(self, pattern: PatternRendering) -> Self {
        Self {
            map_value_patterns: Some(pattern),
            ..self
        }
    }

    /// The same clause, with the order this service states a map's entries in.
    #[must_use]
    pub fn ordering_entries(self, entry_order: MapEntryOrder) -> Self {
        Self {
            entry_order,
            ..self
        }
    }

    /// The same clause, with how this service answers a rendered response.
    #[must_use]
    pub fn answered(self, answered_as: AnsweredAs) -> Self {
        Self {
            answered_as,
            ..self
        }
    }
}

/// One modelled operation, served so that a constraint clause about a collection member
/// states the collection the request carried.
///
/// It makes no routing decision and holds no state of its own: the operation it wraps is
/// the one the generated routing table names for the same key, built from the SAME shared
/// engine, and a request this wrapper does not refuse is handed to it untouched — so the
/// framework still owns the whole deserialize -> validate -> handle -> serialize sequence
/// and every status, shape and message it answers with.
///
/// The cost is DELIBERATE: a request's body is read twice, here and in the operation's own
/// sequence, because the framework hands no already-built input to an operation. Which
/// operations pay it is the service's own choice ([`StatedClauseTable::quoting`] against
/// [`StatedClauseTable::answering`]).
pub struct QuotingSubmittedCollections<Op> {
    operation: Op,
    stated: StatedClause,
}

impl<Op> QuotingSubmittedCollections<Op> {
    /// Wrap one operation, already constructed from the shared engine.
    pub fn new(operation: Op, stated: StatedClause) -> Self {
        Self { operation, stated }
    }
}

impl<Op> ErasedOperation for QuotingSubmittedCollections<Op>
where
    Op: Operation + ErasedOperation,
{
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            // The whole refusal is decided — and the borrow of the definition it is
            // rendered through taken and dropped — BEFORE the await below:
            // `&dyn ErasedServiceDefinition` is not `Sync`, so holding one across that
            // await would make this future non-`Send`.
            if let Some(refused) = quoting_refusal::<Op>(svc, req, &self.stated) {
                return (self.stated.answered_as)(refused);
            }
            (self.stated.answered_as)(self.operation.run(svc, req, ctx).await)
        })
    }

    /// The wrapped operation's modelled auth schemes: wrapping an operation must not
    /// change which schemes sign it.
    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        self.operation.auth_schemes()
    }
}

/// One modelled operation, served so that its responses are answered as this service
/// answers them ([`AnsweredAs`]) and NOTHING else changes.
///
/// For an operation whose model declares no bounded collection member, where the pass
/// above could never produce a clause and would only read the body a second time.
pub struct AnsweringAs<Op> {
    operation: Op,
    answered_as: AnsweredAs,
}

impl<Op> AnsweringAs<Op> {
    /// Wrap one operation, already constructed from the shared engine.
    pub fn new(operation: Op, answered_as: AnsweredAs) -> Self {
        Self {
            operation,
            answered_as,
        }
    }
}

impl<Op> ErasedOperation for AnsweringAs<Op>
where
    Op: Operation + ErasedOperation,
{
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move { (self.answered_as)(self.operation.run(svc, req, ctx).await) })
    }

    /// The wrapped operation's modelled auth schemes, for the same reason as above.
    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        self.operation.auth_schemes()
    }
}

/// The routing table a service serves its operations through so that their error bodies
/// are the ones IT answers with: every operation wrapped, keyed by the routing key the
/// protocol resolves (the operation's own modelled name).
///
/// A service builds one in its `EmulatedService::new` and answers
/// `Service::override_operation` from it; [`quoting`](Self::quoting) and
/// [`answering`](Self::answering) each take one already-constructed operation and file it
/// under [`Operation::NAME`], so a wrapped operation cannot land under a key the protocol
/// never resolves.
pub struct StatedClauseTable {
    table: OperationRoutingTable,
    stated: StatedClause,
}

impl StatedClauseTable {
    /// An empty table whose operations state `stated`'s clauses.
    #[must_use]
    pub fn new(stated: StatedClause) -> Self {
        Self {
            table: OperationRoutingTable::new(),
            stated,
        }
    }

    /// Serve `operation` with a constraint clause about a collection member QUOTING the
    /// collection the request submitted — for an operation whose model declares a bounded
    /// collection member, which is the only kind that can produce such a clause.
    pub fn quoting<O: Operation + ErasedOperation>(&mut self, operation: O) -> &mut Self {
        self.table.add_operation(
            O::NAME,
            Box::new(QuotingSubmittedCollections::new(operation, self.stated)),
        );
        self
    }

    /// Serve `operation` with its responses answered as this service answers them and no
    /// collection pass at all.
    pub fn answering<O: Operation + ErasedOperation>(&mut self, operation: O) -> &mut Self {
        self.table.add_operation(
            O::NAME,
            Box::new(AnsweringAs::new(operation, self.stated.answered_as)),
        );
        self
    }

    /// The table, for the service to hold and route from.
    #[must_use]
    pub fn finish(self) -> OperationRoutingTable {
        self.table
    }
}

/// The refusal `req` earns from `Op`'s own constraints when at least one of its clauses is
/// about a COLLECTION whose submitted value could be stated, or about a MAP's VALUE for a
/// service that states those ([`StatedClause::grouping_map_values`]) — or `None`, which is
/// every other request, including one refused only by clauses this changes nothing about, so
/// the framework's own channel answers it exactly as before.
pub fn quoting_refusal<Op: Operation>(
    svc: &dyn Service,
    req: &RtRequest,
    stated: &StatedClause,
) -> Option<RtResponse> {
    // The codec the request is SERVED in, resolved the way the framework's own dispatch
    // resolves it: the author's `override_protocol` when the service replaces the
    // model-derived one, else the one the definition selects FOR THIS REQUEST (the same
    // resolution [`crate::front_door`] makes). A pass that read the request with a
    // different codec from the operation behind it would find a different violation set —
    // or none at all, for a service whose own codec is what reads its bodies. The
    // selection is per request because a service may declare several protocols and serves
    // each request in the one it names, so the service's first-declared codec is only its
    // own for a service that declares one.
    let protocol = svc
        .override_protocol()
        .unwrap_or_else(|| svc.definition().wire_protocol(req).protocol());
    let refusing = refusing_violations_with_mapped_echoes::<Op>(protocol, req).0;
    let mut restated_a_clause = false;
    // The request's own maps, read at most once per member whatever the clause count.
    let mut submitted = SubmittedMaps::of(req);
    // The MAP members already stated. Every value of one map has to satisfy the same set, so
    // the clause is the same sentence however many entries of that map broke it and
    // whichever of the set's constraints each broke: stating it once is what makes one
    // offending map ONE validation error.
    let mut grouped_maps: HashSet<String> = HashSet::new();
    let mut clauses: Vec<String> = Vec::new();
    let mut violations: Vec<Violation> = Vec::new();
    for (violation, echo) in refusing {
        if let Some(pattern) = stated.map_value_patterns {
            // A further violation under a map whose clause is ALREADY stated needs no second
            // clause. Every one of them is one of that map's VALUES: the shared walk reports
            // a map's KEY as a path segment and never as a constrained leaf
            // (`vendor/smithy-schema-server/src/constraints.rs`), so nothing else is
            // reported below a map member.
            if entry_of(&violation.path).is_some_and(|(member, _)| grouped_maps.contains(member))
            {
                violations.push(violation);
                continue;
            }
            if let Some((member, clause)) =
                map_value_clause::<Op>(&violation, &mut submitted, pattern, stated.entry_order)
            {
                restated_a_clause = true;
                violations.push(violation);
                if grouped_maps.insert(member) {
                    clauses.push(clause);
                }
                continue;
            }
        }
        let quoted = quoting_the_submitted_collection(violation.clone(), echo);
        let restated = as_stated(&violation, quoted, &mut submitted, stated.entry_order);
        let violation = if restated.value == violation.value {
            violation
        } else {
            restated_a_clause = true;
            restated
        };
        clauses.push(violation.message());
        violations.push(violation);
    }
    if !restated_a_clause {
        return None;
    }
    // Every other clause is the framework's own sentence over the violation as stated, and
    // the envelope over them is the framework's own too, so the only differences from the
    // refusal that channel would have rendered are the collection a clause about a
    // collection quotes and the member a clause about a map's values is stated at.
    let message = envelope_over(&clauses)?;
    Some(svc.definition().refusal_response(
        ::aws_smithy_schema_server::ProtocolError::Invalid(violations),
        Some(Refused {
            code: stated.refusal_code.to_string(),
            message,
        }),
        req,
    ))
}

/// The MAP member `violation` belongs to and the clause stated at it, or `None` when
/// `violation` is not a map VALUE's.
///
/// A map entry's value is reported by the framework at `<map member>.<key>`, so the member
/// is that path's first segment and the key is everything after it (a key may itself carry a
/// `.`). THREE facts have to hold together before a clause is stated, and each is read
/// rather than assumed:
///
/// * the operation's input declares that member with a constraint set for its
///   collection's elements/values ([`element_constraint_set`], which reads the element
///   constraints codegen attaches to a collection member and nothing else, so no scalar and
///   no structure member can match);
/// * the REQUEST carried that member as an object of strings, in which
/// * `<key>` is one of the keys.
///
/// So a nested structure member whose own name happens to collide with a key keeps the
/// framework's clause, and a LIST — whose element path is `<list>.<position>.member` and
/// whose body is an array — never reaches here.
///
/// The value half is the whole map the request carried, in the order that service states
/// entries in, exactly as a clause about that same member's own `@length` quotes it
/// ([`as_stated`]): a clause quotes the value held at the member it names.
fn map_value_clause<Op: Operation>(
    violation: &Violation,
    submitted: &mut SubmittedMaps<'_>,
    pattern: PatternRendering,
    order: MapEntryOrder,
) -> Option<(String, String)> {
    stated_map_value_clause(
        violation,
        submitted,
        &[Op::INPUT_SCHEMA],
        pattern,
        order,
    )
}

/// The clause each of `violations` earns as a MAP VALUE's, keyed by the member path that
/// earned it — for a service that words its constraint refusal in its own PRE-DISPATCH layer
/// rather than through [`QuotingSubmittedCollections`], and whose recordings show a
/// constraint broken by one of a map's values stated at the MAP MEMBER.
///
/// A violation that is not a map value's contributes no entry, which is what such a layer
/// keeps the framework's own per-entry clause for
/// ([`crate::validation_message::aws_validation_refusal_over_collections_quoting`], the
/// renderer this is handed to). The rule, the sentence, the set read off `input_schemas` and
/// every bound on what one clause may quote are [`stated_map_value_clause`]'s, so a
/// pre-dispatch layer and the wrapper cannot word one map two ways; the request is read at
/// most once per member path and for at most [`MAX_PARSED_PATHS`] paths, so the parses one
/// refusal performs stay bounded by constants.
///
/// Two entries of one map earn the SAME clause — it names the member, the whole map and the
/// whole set — so a renderer stating the clauses once per distinct text states one
/// validation error for one offending map. That is also why the clause is BUILT once per
/// map member here and reused for every further violation under it
/// ([`member_clause`]): the walk can report one violation per ENTRY of an offending map, so
/// rendering the whole map inside the per-violation loop would be work one request chooses
/// twice over — in its body's length and in its entry count. With the clause built per
/// member, the renderings one refusal performs are bounded by the map members it names (at
/// most [`MAX_PARSED_PATHS`], the same bound that caps the parses) and the clauses it
/// returns by [`crate::validation::MAX_VIOLATIONS`], the ceiling the framework's own walk
/// collects under.
#[must_use]
pub fn stated_map_value_clauses(
    violations: &[Violation],
    req: &RtRequest,
    input_schemas: &[&'static Schema],
    pattern: PatternRendering,
    order: MapEntryOrder,
) -> HashMap<String, String> {
    let mut submitted = SubmittedMaps::of(req);
    // What each map member earns, built the first time a violation names it — the NEGATIVE
    // answer included, so a member no clause can be stated at is decided once too.
    let mut stated: HashMap<String, Option<MemberClause>> = HashMap::new();
    let mut clauses: HashMap<String, String> = HashMap::new();
    for violation in violations.iter().take(crate::validation::MAX_VIOLATIONS) {
        // A violation the walk marked `@sensitive` earns none: such a member is one the
        // service never discloses, so the map it sat in is quoted nowhere — the clause that
        // states no value at all is the framework's own, which this leaves in place.
        if violation.sensitive {
            continue;
        }
        let Some((member, key)) = entry_of(&violation.path) else {
            continue;
        };
        if !stated.contains_key(member) {
            let built = member_clause(member, &mut submitted, input_schemas, pattern, order);
            stated.insert(member.to_string(), built);
        }
        let Some(Some(member_clause)) = stated.get(member) else {
            continue;
        };
        // The key half stays per violation: a clause is stated only for an entry the
        // REQUEST carried under that member, which is a set lookup rather than a scan of
        // the map once per violation.
        if member_clause.keys.contains(key) {
            clauses.insert(violation.path.clone(), member_clause.clause.clone());
        }
    }
    clauses
}

/// What one MAP member earns: the clause stated at it, and the keys the request carried
/// under it — which is what decides whether a violation reported below that member belongs
/// to an entry the request actually sent.
struct MemberClause {
    /// The clause, identical for every violation under this member.
    clause: String,
    /// The keys the request's own map carried, as a set.
    keys: HashSet<String>,
}

/// The clause stated at the map member `member` and the keys its map carried, or `None`
/// when no clause can be stated there — the ONE place that clause is built, whichever layer
/// states it.
///
/// THREE facts have to hold together, and each is read rather than assumed: the operation's
/// input declares that member with a constraint set for its collection's values
/// ([`element_constraint_set`]), the REQUEST carried that member as a map of strings, and
/// the service states an order for its entries. The whole map renders ONCE here, so a
/// refusal renders at most one map per member it names.
fn member_clause(
    member: &str,
    submitted: &mut SubmittedMaps<'_>,
    input_schemas: &[&'static Schema],
    pattern: PatternRendering,
    order: MapEntryOrder,
) -> Option<MemberClause> {
    let declared = element_constraint_set(input_schemas, member)?;
    let entries = submitted.of_path(member)?;
    let keys: HashSet<String> = entries.iter().map(|(key, _)| key.clone()).collect();
    let pairs = order(entries)?;
    let requirements: Vec<String> = declared
        .into_iter()
        .map(|kind| stated_requirement(kind, pattern))
        .collect();
    Some(MemberClause {
        clause: format!(
            "Value '{}' at '{member}' failed to satisfy constraint: {}",
            quoted_echo(&rendered_map(&pairs)),
            map_value_set_requirement(&requirements),
        ),
        keys,
    })
}

/// The MAP member `violation` belongs to and the clause stated at it, or `None` when
/// `violation` is not a map VALUE's.
///
/// The clause and every guard around it are [`member_clause`]'s; this is the per-violation
/// half — the `@sensitive` withholding the walk applied, and whether the entry the
/// violation names is one the request carried.
fn stated_map_value_clause(
    violation: &Violation,
    submitted: &mut SubmittedMaps<'_>,
    input_schemas: &[&'static Schema],
    pattern: PatternRendering,
    order: MapEntryOrder,
) -> Option<(String, String)> {
    if violation.sensitive {
        return None;
    }
    let (member, key) = entry_of(&violation.path)?;
    let stated = member_clause(member, submitted, input_schemas, pattern, order)?;
    stated
        .keys
        .contains(key)
        .then_some((member.to_string(), stated.clause))
}

/// The member and the entry KEY a path of the form `<member>.<key>` names, or `None` for a
/// path with no step at all. The key is everything after the FIRST step, because a map key
/// may itself carry a `.`.
fn entry_of(path: &str) -> Option<(&str, &str)> {
    path.split_once('.')
}

/// One constraint of a collection's element/value shape as a clause states it: the
/// framework's own words for the class ([`requirement`]), with a `@pattern` rendered as the
/// service renders one — so a requirement nested into a map clause reads exactly like one a
/// reported member gets.
fn stated_requirement(kind: ViolationKind, pattern: PatternRendering) -> String {
    match kind {
        ViolationKind::Pattern(expression) => {
            requirement(ViolationKind::Pattern(pattern(&expression)))
        }
        other => requirement(other),
    }
}

/// `quoted` — the shared rendering of the collection `violation` is about — with a MAP's
/// entries restated in the order the service states them (`order`) rather than in the key
/// order the built input leaves them in.
///
/// Only the ORDER changes: the entries, the braces and the `<key>=<value>` form are the
/// shared rendering's ([`rendered_map`]), and the restated map is taken only when it
/// renders to exactly the length the shared walk had already decided to carry — the same
/// entries in another order, and therefore the same bytes. See the module header for every
/// guard that keeps `quoted` instead.
fn as_stated(
    violation: &Violation,
    quoted: Violation,
    submitted: &mut SubmittedMaps<'_>,
    order: MapEntryOrder,
) -> Violation {
    let Some(rendered) = quoted.value.as_deref() else {
        return quoted;
    };
    let Some(restated) = restated_map(violation, submitted, order) else {
        return quoted;
    };
    if restated.len() != rendered.len() {
        return quoted;
    }
    Violation {
        value: Some(restated),
        ..quoted
    }
}

/// `violations` with every clause about a MAP member quoting the map `req` carried, in the
/// order the service states one (`order`), where the validating walk stored its own summary
/// of that map's SIZE.
///
/// For a service that words its constraint refusal in its own PRE-DISPATCH layer rather
/// than through [`QuotingSubmittedCollections`]: such a layer holds the request, which is
/// where the entry order still is, but it has no echo to restate from — the shared walk
/// renders a map it reaches as the built input's own unordered map, so a clause about one
/// arrives carrying `<n> entries`, which is not a value any caller sent. This quotes it
/// from the body instead, under [`crate::validation::quoted_echo`]'s ceiling, which is the
/// same ceiling the shared walk quotes one clause to.
///
/// Everything but the order is this module's, exactly as for the wrapper: the guards
/// ([`restated_map`]), the rendering ([`rendered_map`]) and the reading
/// ([`submitted_map_entries_at`]) — so a pre-dispatch layer and the wrapper cannot word one
/// collection two ways. The request is read at most once per member path asked for and for
/// at most [`MAX_PARSED_PATHS`] paths ([`SubmittedMaps`]), so the parses one refusal
/// performs are bounded by constants rather than by the violation count a request chooses.
#[must_use]
pub fn quoting_submitted_maps(
    violations: Vec<Violation>,
    req: &RtRequest,
    order: MapEntryOrder,
) -> Vec<Violation> {
    let mut submitted = SubmittedMaps::of(req);
    violations
        .into_iter()
        .map(|violation| {
            match restated_map(&violation, &mut submitted, order) {
                Some(restated) => Violation {
                    value: Some(crate::validation::quoted_echo(&restated).into_owned()),
                    ..violation
                },
                None => violation,
            }
        })
        .collect()
}

/// The MAP the request carried at the member `violation` names, rendered in `order` — or
/// `None` for a clause this states nothing about.
///
/// The one place the guards live, so the two restating paths above cannot disagree about
/// which clause is a map's and which map is the one it is about: the clause has to be a
/// `@length` one whose stored value IS the walk's rendering of a map's size
/// ([`map_entry_count`]), the request has to carry a map of strings at that member's path,
/// its entry count has to be the one the walk counted, and the service's own rule has to
/// state an order for those entries.
fn restated_map(
    violation: &Violation,
    submitted: &mut SubmittedMaps<'_>,
    order: MapEntryOrder,
) -> Option<String> {
    if !matches!(
        violation.kind,
        ViolationKind::MinLength(_) | ViolationKind::MaxLength(_)
    ) {
        return None;
    }
    let entries = map_entry_count(violation.value.as_deref()?)?;
    let held = submitted.of_path(&violation.path)?;
    if held.len() != entries {
        return None;
    }
    let ordered = order(held)?;
    Some(rendered_map(&ordered))
}

/// The most distinct member paths ONE refusal reads the body for.
///
/// The parse count has to be bounded by the MODEL and not by the request, and a clause's
/// member path is not always the model's: a constraint broken by one of a map's VALUES is
/// reported at `<map member>.<key>`, where the key is the caller's own text, so a request
/// can present as many distinct paths as it sent entries. This is that bound — well above
/// the map members any emulated input declares, and a constant — so a refusal reads the
/// body a fixed number of times whatever a request chooses. A path past it is answered as
/// a member no map was found at, which keeps the validating walk's own rendering.
const MAX_PARSED_PATHS: usize = 16;

/// How many STEPS of one member path this reader descends through the body, at most.
///
/// The depth has to be bounded by the MODEL and not by the request, for the same reason
/// [`MAX_PARSED_PATHS`] bounds the count: a clause's path is not always the model's — a
/// constraint broken by one of a map's VALUES is reported at `<map member>.<key>`, the key
/// is the caller's own text and may itself carry a `.`, and [`SubmittedMaps::of_path`]
/// splits the whole path on `.` — so the number of steps a path presents is request-shaped,
/// while each step is one level of descent into the body. This is that bound — well above
/// the deepest member path an emulated input declares (`probes.1.member.packetSize` is
/// four), and a constant — so one read descends a fixed depth whatever a request chooses. A
/// path past it is answered as a member no map was found at, which keeps the validating
/// walk's own rendering, exactly as a path past the count is.
const MAX_PATH_STEPS: usize = 8;

/// The maps the REQUEST carried, each read at most ONCE however many clauses ask for it,
/// and at most [`MAX_PARSED_PATHS`] of them read at all.
///
/// Why it exists: a clause is restated per VIOLATION, and the walk can report one violation
/// per element of a collection, so reading the body inside that loop would re-parse the whole
/// body once per clause — work one request chooses twice over, in its body's length and in
/// its violation count. Here the answer for a member path is remembered the first time it is
/// asked for, the NEGATIVE answer included (a path the body does not carry a map of strings
/// at is never re-read), and only [`MAX_PARSED_PATHS`] paths are read at all — so both the
/// parses one refusal performs and the keys it retains are bounded by constants rather than
/// by anything the request chooses.
struct SubmittedMaps<'r> {
    /// The buffered request body, already read and deserialized once by the shared walk
    /// before this layer looks at it.
    body: &'r [u8],
    /// What the body holds at each member path asked for so far, `None` for a path this
    /// layer found no map of strings at. At most [`MAX_PARSED_PATHS`] entries.
    read: HashMap<String, Option<Vec<(String, String)>>>,
    /// How many more distinct paths this refusal may read the body for.
    budget: usize,
}

impl<'r> SubmittedMaps<'r> {
    /// Read `req`'s body lazily: nothing is parsed until a clause asks for a member.
    #[must_use]
    pub fn of(req: &'r RtRequest) -> Self {
        Self {
            body: req.body().bytes().unwrap_or_default(),
            read: HashMap::new(),
            budget: MAX_PARSED_PATHS,
        }
    }

    /// The entries the member at `path` carried, in the order the request wrote them, or
    /// `None` when the body carries no map of strings there — and `None` for every path
    /// past this refusal's [`MAX_PARSED_PATHS`] reads, without reading the body again.
    ///
    /// `path` is the member path a clause is stated at, so a map declared on a nested
    /// structure member is reached at the dotted path it is reported under
    /// ([`submitted_map_entries_at`]) and a top-level one at its own name.
    fn of_path(&mut self, path: &str) -> Option<&[(String, String)]> {
        if !self.read.contains_key(path) {
            if self.budget == 0 {
                return None;
            }
            self.budget -= 1;
            let steps: Vec<&str> = path.split('.').collect();
            let read = submitted_map_entries_at(self.body, &steps);
            self.read.insert(path.to_string(), read);
        }
        self.read.get(path)?.as_deref()
    }
}

/// The entries the request's `member` carried, in the order its body wrote them, or `None`
/// when the body is not a structure, carries no such member, or carries one that is not a
/// map of strings.
///
/// A JSON body is read by `serde_json`'s own parser, through `serde`'s deserializer seam, and
/// a CBOR one by the CBOR implementation the framework's own codec for that protocol is built
/// on ([`read_member_entries`]): nothing here reads the body's bytes itself. The seam is what
/// keeps the ORDER, which a parse into a `Value` would not — that type holds its members in a
/// map of its own.
///
/// It reads the body the server has already buffered and the framework has already
/// deserialized into the operation's input, over the one member a clause is about and
/// skipping every other without building it, so it adds no growth a request can drive
/// beyond that one parse; and what it produces is only used while it renders to the length
/// the shared walk's own bounds had already admitted (see [`as_stated`]).
///
/// The member is matched without regard to case because a violation names it in the
/// lowerCamel path the framework states a clause at, while the body carries the wire name
/// the model declares. A model that declared two members of one shape differing only in
/// case would make the first one the body carries the one read.
#[must_use]
pub fn submitted_map_entries(body: &[u8], member: &str) -> Option<Vec<(String, String)>> {
    submitted_map_entries_at(body, &[member])
}

/// The entries the request carried at `path` — the member names from the body's root down
/// to the map, as the dotted member path a clause is stated at spells them — in the order
/// its body wrote them, or `None` when the body does not carry a map of strings there.
///
/// [`submitted_map_entries`] is this over a path of one step, which is the member a clause
/// at the TOP level of the body names; a map declared on a nested structure member is
/// reported at `<member>.<member>.…`, and that is the whole difference. Every guard, the
/// parser seam that keeps the order and the case-insensitive matching of a member are the
/// same, and so is the one parse per question: the members not on the path are skipped
/// without being built.
#[must_use]
pub fn submitted_map_entries_at(body: &[u8], path: &[&str]) -> Option<Vec<(String, String)>> {
    read_member_entries(body, path, AdmittedValues::StringsOnly)
}

/// [`submitted_map_entries_at`] over a map whose values are SCALARS of any kind: a string as
/// sent, and a number or a boolean as the document wrote it.
///
/// For a clause about a map whose VALUE SHAPE is not a string — a map of doubles under a
/// `@range`, say — which the reader above answers `None` for, because a clause quoting a map
/// may never quote one it read only part of. Everything else is that reader's: the same
/// parser seam keeping the document's order, the same case-insensitive matching of a member,
/// the same single parse per question, and the same `None` for a member the body carries no
/// map of scalars at. How a value is RENDERED into a clause belongs to the service that
/// states it, because the form a value is written in is that value's own shape's.
#[must_use]
pub fn submitted_scalar_map_entries_at(
    body: &[u8],
    path: &[&str],
) -> Option<Vec<(String, String)>> {
    read_member_entries(body, path, AdmittedValues::Scalars)
}

/// The one reading both entry points above go through: `path`'s member, read as the map of
/// `admitted` values it holds.
///
/// Both wire forms a request's body can arrive in are read, because a service's model may
/// declare several protocols and serves each request in the one it names: the JSON document
/// an `awsJson` or `restJson1` request carries, and otherwise the CBOR map an `rpcv2Cbor`
/// request carries ([`cbor_member_entries`]). A body neither reads is answered `None`, which
/// keeps the validating walk's own rendering of the collection.
fn read_member_entries(
    body: &[u8],
    path: &[&str],
    admitted: AdmittedValues,
) -> Option<Vec<(String, String)>> {
    // Every descent below is one level per step of `path`, and the step count a path
    // presents is request-shaped ([`MAX_PATH_STEPS`]): the depth is bounded HERE, at the one
    // funnel both readers go through, so neither can be driven past it.
    if path.is_empty() || path.len() > MAX_PATH_STEPS {
        return None;
    }
    let mut deserializer = ::serde_json::Deserializer::from_slice(body);
    ::serde::de::DeserializeSeed::deserialize(MemberEntries { path, admitted }, &mut deserializer)
        .ok()
        .flatten()
        .or_else(|| cbor_member_entries(body, path, admitted))
}

/// [`read_member_entries`] over the CBOR body an `rpcv2Cbor` request carries: the entries the
/// member at `path` held, in the order the map wrote them.
///
/// The same answer the JSON reader gives for the same request, so a clause about a collection
/// quotes the same thing whichever of a service's declared protocols carried the call. Read
/// with the CBOR implementation the framework's own codec for that protocol is built on, over
/// the one member a clause is about: every other member is SKIPPED without being built, so
/// one question costs one pass of the body exactly as the JSON reading does, and the descent
/// is one level per step of a path whose length [`read_member_entries`] has already bounded
/// by [`MAX_PATH_STEPS`].
///
/// `None` when the body is not a CBOR map, carries no such member, or carries one that is
/// not a map of `admitted` values — the same conditions the JSON reader answers `None` for.
fn cbor_member_entries(
    body: &[u8],
    path: &[&str],
    admitted: AdmittedValues,
) -> Option<Vec<(String, String)>> {
    cbor_entries_at(&mut ::aws_smithy_cbor::Decoder::new(body), path, admitted)
}

/// The map `path` names inside the CBOR map the decoder is positioned at, descending through
/// the structure members ahead of it by reading each with this same walk.
fn cbor_entries_at(
    decoder: &mut ::aws_smithy_cbor::Decoder<'_>,
    path: &[&str],
    admitted: AdmittedValues,
) -> Option<Vec<(String, String)>> {
    let (step, rest) = path.split_first()?;
    // A map of a declared length, or an indefinite one closed by a break item: an encoder of
    // this protocol is free to write either, and the framework's own codec writes the second.
    let declared = decoder.map().ok()?;
    let mut read = 0_u64;
    loop {
        if cbor_map_ends(decoder, declared, read)? {
            return None;
        }
        // The member is matched without regard to case for the reason the JSON reader
        // matches it that way: a violation names it in the lowerCamel path a clause is
        // stated at, while the body carries the wire name the model declares.
        let name = decoder.string().ok()?;
        if name.eq_ignore_ascii_case(step) {
            return if rest.is_empty() {
                cbor_map_of(decoder, admitted)
            } else {
                cbor_entries_at(decoder, rest, admitted)
            };
        }
        decoder.skip().ok()?;
        read = read.saturating_add(1);
    }
}

/// Whether the CBOR map the decoder is reading ends here, `read` entries in — consuming the
/// break item of an indefinite-length one. `None` when the bytes end mid-map.
fn cbor_map_ends(
    decoder: &mut ::aws_smithy_cbor::Decoder<'_>,
    declared: Option<u64>,
    read: u64,
) -> Option<bool> {
    match declared {
        Some(count) => Some(read == count),
        None => {
            if matches!(decoder.datatype().ok()?, ::aws_smithy_cbor::data::Type::Break) {
                decoder.skip().ok()?;
                return Some(true);
            }
            Some(false)
        }
    }
}

/// The CBOR map the decoder is positioned at as its entries, in the order it wrote them, or
/// `None` when it is no map of `admitted` values.
fn cbor_map_of(
    decoder: &mut ::aws_smithy_cbor::Decoder<'_>,
    admitted: AdmittedValues,
) -> Option<Vec<(String, String)>> {
    let declared = decoder.map().ok()?;
    let mut entries: Vec<(String, String)> = Vec::new();
    loop {
        if cbor_map_ends(decoder, declared, entries.len() as u64)? {
            return Some(entries);
        }
        let key = decoder.string().ok()?;
        entries.push((key, cbor_scalar(decoder, admitted)?));
    }
}

/// The value the decoder is positioned at, as the text it was written with — a string as
/// sent, and, where the caller admits any scalar, a number or a boolean as the document
/// wrote it. `None` for a value of any other kind, which is what makes a map carrying one
/// "no map of `admitted` values".
fn cbor_scalar(
    decoder: &mut ::aws_smithy_cbor::Decoder<'_>,
    admitted: AdmittedValues,
) -> Option<String> {
    use ::aws_smithy_cbor::data::Type;
    let datatype = decoder.datatype().ok()?;
    match (datatype, admitted) {
        (Type::String | Type::StringIndef, _) => decoder.string().ok(),
        (_, AdmittedValues::StringsOnly) => None,
        (Type::Bool, _) => decoder.boolean().ok().map(|held| held.to_string()),
        (
            Type::U8 | Type::U16 | Type::U32 | Type::U64 | Type::I8 | Type::I16 | Type::I32
            | Type::I64 | Type::Int,
            _,
        ) => decoder.long().ok().map(|whole| whole.to_string()),
        (Type::F16 | Type::F32 | Type::F64, _) => {
            decoder.double().ok().map(|held| held.to_string())
        }
        _ => None,
    }
}

/// Which values a map read back from a request body may hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AdmittedValues {
    /// Strings alone: a map holding anything else is read as no map at all.
    StringsOnly,
    /// Any scalar — a string, a number or a boolean — each as the document wrote it.
    Scalars,
}

/// Reads the member a PATH names as the map of [`AdmittedValues`] it holds, keeping the order
/// the document wrote the entries in. Every member off the path is skipped without being
/// built, and a path of more than one step descends through the structure members ahead of
/// the map by reading each of them with this same seed.
struct MemberEntries<'a> {
    path: &'a [&'a str],
    admitted: AdmittedValues,
}

impl<'de> ::serde::de::DeserializeSeed<'de> for MemberEntries<'_> {
    type Value = Option<Vec<(String, String)>>;

    fn deserialize<D: ::serde::Deserializer<'de>>(
        self,
        deserializer: D,
    ) -> Result<Self::Value, D::Error> {
        deserializer.deserialize_map(self)
    }
}

impl<'de> ::serde::de::Visitor<'de> for MemberEntries<'_> {
    type Value = Option<Vec<(String, String)>>;

    fn expecting(&self, formatter: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        write!(
            formatter,
            "a JSON object carrying the member {}",
            self.path.first().copied().unwrap_or_default()
        )
    }

    fn visit_map<A: ::serde::de::MapAccess<'de>>(
        self,
        mut members: A,
    ) -> Result<Self::Value, A::Error> {
        let Some((step, rest)) = self.path.split_first() else {
            return Ok(None);
        };
        let mut found: Option<Vec<(String, String)>> = None;
        while let Some(name) = members.next_key::<String>()? {
            if found.is_none() && name.eq_ignore_ascii_case(step) {
                found = if rest.is_empty() {
                    match self.admitted {
                        AdmittedValues::StringsOnly => {
                            Some(members.next_value::<OrderedStringMap>()?.0)
                        }
                        AdmittedValues::Scalars => {
                            Some(members.next_value::<OrderedScalarMap>()?.0)
                        }
                    }
                } else {
                    // The map is deeper: the member on the path is itself read with this
                    // same seed, over the rest of the path.
                    members.next_value_seed(MemberEntries {
                        path: rest,
                        admitted: self.admitted,
                    })?
                };
            } else {
                members.next_value::<::serde::de::IgnoredAny>()?;
            }
        }
        Ok(found)
    }
}

/// A JSON object of strings as the entries it holds, in document order.
struct OrderedStringMap(Vec<(String, String)>);

impl<'de> ::serde::Deserialize<'de> for OrderedStringMap {
    fn deserialize<D: ::serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Entries;

        impl<'de> ::serde::de::Visitor<'de> for Entries {
            type Value = OrderedStringMap;

            fn expecting(&self, formatter: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                formatter.write_str("a JSON object of strings")
            }

            fn visit_map<A: ::serde::de::MapAccess<'de>>(
                self,
                mut entries: A,
            ) -> Result<Self::Value, A::Error> {
                let mut held: Vec<(String, String)> = Vec::new();
                while let Some(entry) = entries.next_entry::<String, String>()? {
                    held.push(entry);
                }
                Ok(OrderedStringMap(held))
            }
        }

        deserializer.deserialize_map(Entries)
    }
}

/// A JSON object of SCALARS as the entries it holds, in document order, each value as the
/// document wrote it: a string as sent, and a number or a boolean in the text `serde_json`
/// read it as.
struct OrderedScalarMap(Vec<(String, String)>);

impl<'de> ::serde::Deserialize<'de> for OrderedScalarMap {
    fn deserialize<D: ::serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Entries;

        impl<'de> ::serde::de::Visitor<'de> for Entries {
            type Value = OrderedScalarMap;

            fn expecting(&self, formatter: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                formatter.write_str("a JSON object of scalars")
            }

            fn visit_map<A: ::serde::de::MapAccess<'de>>(
                self,
                mut entries: A,
            ) -> Result<Self::Value, A::Error> {
                let mut held: Vec<(String, String)> = Vec::new();
                while let Some((key, value)) = entries.next_entry::<String, Scalar>()? {
                    held.push((key, value.0));
                }
                Ok(OrderedScalarMap(held))
            }
        }

        deserializer.deserialize_map(Entries)
    }
}

/// One scalar of a JSON document as the text it was written with. A value of any other kind
/// fails the read, which is what makes a map carrying one "no map of scalars".
struct Scalar(String);

impl<'de> ::serde::Deserialize<'de> for Scalar {
    fn deserialize<D: ::serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let held = ::serde_json::Value::deserialize(deserializer)?;
        match held {
            ::serde_json::Value::String(text) => Ok(Self(text)),
            ::serde_json::Value::Number(number) => Ok(Self(number.to_string())),
            ::serde_json::Value::Bool(held) => Ok(Self(held.to_string())),
            other => Err(::serde::de::Error::custom(format!(
                "a map entry carrying no scalar value: {other}"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RULE: one refusal reads the body for at most [`MAX_PARSED_PATHS`] distinct member
    /// paths, however many clauses ask — a clause stated at `<map member>.<key>` carries
    /// the CALLER's own text in its last step, so the path count is not the model's.
    #[test]
    fn one_refusal_reads_the_body_for_a_bounded_number_of_paths() {
        let body = ::aws_smithy_runtime_api::http::Request::new(
            ::aws_smithy_types::body::SdkBody::from(r#"{"Attributes":{"a":"1"}}"#),
        );
        let mut submitted = SubmittedMaps::of(&body);

        // The member the body does carry a map at answers it, and answers it again
        // without spending a second read.
        for read in 1..=3 {
            assert!(
                submitted.of_path("attributes").is_some(),
                "read {read}: the member carrying a map must be read back"
            );
        }
        assert_eq!(
            submitted.budget,
            MAX_PARSED_PATHS - 1,
            "one member path must spend one read however often it is asked for"
        );

        // Every path a caller could shape spends at most the rest of the budget: past it
        // nothing is read and nothing is kept.
        for index in 0..100 {
            assert!(
                submitted.of_path(&format!("attributes.key-{index}")).is_none(),
                "path {index}: a caller-shaped path carries no map of strings"
            );
        }
        assert_eq!(submitted.budget, 0, "the reads must be spent, not exceeded");
        assert_eq!(
            submitted.read.len(),
            MAX_PARSED_PATHS,
            "at most one remembered answer per read, got {}",
            submitted.read.len()
        );
    }

    /// RULE: one read descends at most [`MAX_PATH_STEPS`] steps into the body, on either
    /// wire form — a path whose last step is the CALLER's own map key may carry a `.` of its
    /// own, so the step count is not the model's either. A path past the bound is answered
    /// as a member no map was found at, which keeps the validating walk's own rendering.
    #[test]
    fn one_read_descends_a_bounded_number_of_path_steps() {
        /// A body nesting `steps` structure members, the innermost carrying a map of
        /// strings, written as JSON and as CBOR — the shape a path of that length matches.
        fn nested(steps: usize) -> (Vec<u8>, Vec<u8>) {
            let mut json = String::from(r#"{"m":{"k":"v"}}"#);
            for _ in 0..steps {
                json = format!(r#"{{"step":{json}}}"#);
            }
            let value: ::serde_json::Value =
                ::serde_json::from_str(&json).expect("the body is JSON");
            (json.clone().into_bytes(), cbor_of(&value))
        }

        /// `value` as CBOR, written with declared container lengths.
        fn cbor_of(value: &::serde_json::Value) -> Vec<u8> {
            fn write(encoder: &mut ::aws_smithy_cbor::Encoder, value: &::serde_json::Value) {
                match value {
                    ::serde_json::Value::String(text) => {
                        encoder.str(text);
                    }
                    ::serde_json::Value::Object(members) => {
                        encoder.map(members.len());
                        for (name, held) in members {
                            encoder.str(name);
                            write(encoder, held);
                        }
                    }
                    other => panic!("this case writes no {other}"),
                }
            }
            let mut encoder = ::aws_smithy_cbor::Encoder::new(Vec::new());
            write(&mut encoder, value);
            encoder.into_writer()
        }

        // The deepest path the bound admits is read; one step past it reads nothing, on
        // either wire form, however deeply the body nests to match it.
        for (case, steps, expected) in [
            ("at the bound", MAX_PATH_STEPS - 1, true),
            ("one step past the bound", MAX_PATH_STEPS, false),
            ("far past the bound", MAX_PATH_STEPS * 4, false),
        ] {
            let mut path: Vec<&str> = vec!["step"; steps];
            path.push("m");
            let (json, cbor) = nested(steps);
            for (wire, body) in [("json", &json), ("cbor", &cbor)] {
                let read = submitted_map_entries_at(body, &path).is_some();
                assert_eq!(
                    read, expected,
                    "case {case:?} over {wire}: a path of {} steps must {}be read",
                    path.len(),
                    if expected { "" } else { "not " }
                );
            }
        }
    }

    /// RULE: the member a clause is about is read back in the order the document wrote it,
    /// whatever else the body carries — and nothing is read where there is nothing to read.
    #[test]
    fn the_named_member_is_read_back_in_document_order() {
        let body = br#"{"Before":"x","Attributes":{"z":"1","a":"2","m":"3"},"After":[1,2]}"#;
        let entries = submitted_map_entries(body, "attributes").expect("the member is a map");
        assert_eq!(
            entries,
            vec![
                ("z".to_string(), "1".to_string()),
                ("a".to_string(), "2".to_string()),
                ("m".to_string(), "3".to_string()),
            ],
            "expected the entries in document order, got {entries:?}"
        );

        // (case, body, member)
        for (case, body, member) in [
            ("no such member", &br#"{"Other":{"a":"1"}}"#[..], "attributes"),
            ("not an object", &br#"[1,2,3]"#[..], "attributes"),
            ("not JSON at all", &b"<Error/>"[..], "attributes"),
            (
                "a member that is not a map of strings",
                &br#"{"Attributes":{"a":1}}"#[..],
                "attributes",
            ),
        ] {
            assert!(
                submitted_map_entries(body, member).is_none(),
                "case {case:?}: expected nothing to be read"
            );
        }
    }

    /// RULE: a map a NESTED member carries is read back at the dotted path a clause names
    /// it by, in document order — and a path that names nothing reads nothing.
    #[test]
    fn a_nested_members_map_is_read_back_at_its_path() {
        let body = br#"{"SamplingRule":{"RuleName":"r","Attributes":{"z":"1","a":"2"}},"Other":3}"#;
        let entries = submitted_map_entries_at(body, &["samplingRule", "attributes"])
            .expect("the nested member is a map");
        assert_eq!(
            entries,
            vec![
                ("z".to_string(), "1".to_string()),
                ("a".to_string(), "2".to_string()),
            ],
            "expected the entries in document order, got {entries:?}"
        );

        // (case, path)
        for (case, path) in [
            ("a step that is not there", &["rule", "attributes"][..]),
            ("a leaf that is not there", &["samplingRule", "tags"][..]),
            ("a leaf that is not a map", &["samplingRule", "ruleName"][..]),
            ("no path at all", &[][..]),
            (
                "a path deeper than the body nests",
                &["samplingRule", "attributes", "z"][..],
            ),
        ] {
            assert!(
                submitted_map_entries_at(body, path).is_none(),
                "case {case:?}: expected nothing to be read"
            );
        }
    }

    /// RULE: a map of SCALARS is read back with each value as the document wrote it — a
    /// string as sent, a number and a boolean in their own text — while the reader of a map
    /// of STRINGS answers nothing for the same body; and a map carrying a value that is no
    /// scalar is read by neither.
    ///
    /// The rows span the value kinds a JSON document writes: a string, an integral and a
    /// fractional number, a negative one, an exponent form, a boolean, and the two composites
    /// that are no scalar at all.
    #[test]
    fn a_map_of_scalars_is_read_back_with_each_value_as_written() {
        let body = br#"{"RoutingConfig":{"AdditionalVersionWeights":{"2":1.5,"3":2,"4":-0.25,"5":1e3,"6":"x","7":true}}}"#;
        let path = &["routingConfig", "additionalVersionWeights"][..];
        let entries = submitted_scalar_map_entries_at(body, path)
            .expect("the nested member is a map of scalars");
        let read: Vec<(&str, &str)> = entries
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str()))
            .collect();
        assert_eq!(
            read,
            vec![
                ("2", "1.5"),
                ("3", "2"),
                ("4", "-0.25"),
                ("5", "1000.0"),
                ("6", "x"),
                ("7", "true"),
            ],
            "expected every scalar in document order as the document wrote it, got {read:?}"
        );
        assert!(
            submitted_map_entries_at(body, path).is_none(),
            "a map of numbers is no map of STRINGS, so the string reader must read nothing"
        );

        // (case, body) — a value that is no scalar is read by neither.
        for (case, body) in [
            (
                "an entry carrying an object",
                &br#"{"Tags":{"a":{"b":"c"}}}"#[..],
            ),
            ("an entry carrying a list", &br#"{"Tags":{"a":[1]}}"#[..]),
            ("an entry carrying null", &br#"{"Tags":{"a":null}}"#[..]),
        ] {
            assert!(
                submitted_scalar_map_entries_at(body, &["tags"]).is_none(),
                "case {case:?}: expected nothing to be read"
            );
        }
    }

    /// A CBOR body, written as `lengths` says, carrying `members` at its root.
    fn cbor_body(members: &[(&str, CborValue<'_>)], lengths: CborLengths) -> Vec<u8> {
        let mut encoder = ::aws_smithy_cbor::Encoder::new(Vec::new());
        write_cbor_map(&mut encoder, members, lengths);
        encoder.into_writer()
    }

    /// One value a CBOR case writes, over the item kinds a request body carries.
    enum CborValue<'a> {
        Text(&'a str),
        Whole(i64),
        Fraction(f64),
        Flag(bool),
        Map(&'a [(&'a str, CborValue<'a>)]),
        List(&'a [i64]),
    }

    /// How a CBOR case writes its container headers: both forms are CBOR, and the framework's
    /// own codec writes the second.
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum CborLengths {
        Declared,
        Indefinite,
    }

    fn write_cbor_map(
        encoder: &mut ::aws_smithy_cbor::Encoder,
        members: &[(&str, CborValue<'_>)],
        lengths: CborLengths,
    ) {
        match lengths {
            CborLengths::Declared => encoder.map(members.len()),
            CborLengths::Indefinite => encoder.begin_map(),
        };
        for (name, held) in members {
            encoder.str(name);
            match held {
                CborValue::Text(text) => {
                    encoder.str(text);
                }
                CborValue::Whole(whole) => {
                    encoder.long(*whole);
                }
                CborValue::Fraction(held) => {
                    encoder.double(*held);
                }
                CborValue::Flag(flag) => {
                    encoder.boolean(*flag);
                }
                CborValue::Map(nested) => write_cbor_map(encoder, nested, lengths),
                CborValue::List(elements) => {
                    match lengths {
                        CborLengths::Declared => encoder.array(elements.len()),
                        CborLengths::Indefinite => encoder.begin_array(),
                    };
                    for element in *elements {
                        encoder.long(*element);
                    }
                    if lengths == CborLengths::Indefinite {
                        encoder.end();
                    }
                }
            }
        }
        if lengths == CborLengths::Indefinite {
            encoder.end();
        }
    }

    /// RULE: the member a clause is about is read back from the CBOR body an `rpcv2Cbor`
    /// request carries exactly as it is read back from the JSON body of the same request —
    /// in the order the map wrote its entries, at a top-level member and at a nested path,
    /// with both container framings, and nothing read where there is nothing to read.
    #[test]
    fn a_cbor_request_body_is_read_back_the_same_way_a_json_one_is() {
        for lengths in [CborLengths::Declared, CborLengths::Indefinite] {
            let case = match lengths {
                CborLengths::Declared => "declared lengths",
                CborLengths::Indefinite => "indefinite lengths",
            };
            let body = cbor_body(
                &[
                    ("Before", CborValue::Text("x")),
                    (
                        "Attributes",
                        CborValue::Map(&[
                            ("z", CborValue::Text("1")),
                            ("a", CborValue::Text("2")),
                            ("m", CborValue::Text("3")),
                        ]),
                    ),
                    ("After", CborValue::List(&[1, 2])),
                ],
                lengths,
            );
            let entries =
                submitted_map_entries(&body, "attributes").expect("the member is a map");
            assert_eq!(
                entries,
                vec![
                    ("z".to_string(), "1".to_string()),
                    ("a".to_string(), "2".to_string()),
                    ("m".to_string(), "3".to_string()),
                ],
                "case {case}: expected the entries in the order the map wrote them, got \
                 {entries:?}"
            );

            // A map a nested member carries, at the dotted path a clause names it by.
            let nested = cbor_body(
                &[
                    (
                        "SamplingRule",
                        CborValue::Map(&[
                            ("RuleName", CborValue::Text("r")),
                            (
                                "Attributes",
                                CborValue::Map(&[
                                    ("z", CborValue::Text("1")),
                                    ("a", CborValue::Text("2")),
                                ]),
                            ),
                        ]),
                    ),
                    ("Other", CborValue::Whole(3)),
                ],
                lengths,
            );
            let read = submitted_map_entries_at(&nested, &["samplingRule", "attributes"])
                .expect("the nested member is a map");
            assert_eq!(
                read,
                vec![
                    ("z".to_string(), "1".to_string()),
                    ("a".to_string(), "2".to_string()),
                ],
                "case {case}: expected the nested entries in order, got {read:?}"
            );

            // A map of SCALARS, each value as the map wrote it, where the reader of a map of
            // STRINGS answers nothing for the same body.
            let scalars = cbor_body(
                &[(
                    "Weights",
                    CborValue::Map(&[
                        ("2", CborValue::Fraction(1.5)),
                        ("3", CborValue::Whole(2)),
                        ("4", CborValue::Text("x")),
                        ("5", CborValue::Flag(true)),
                    ]),
                )],
                lengths,
            );
            let scalar_entries = submitted_scalar_map_entries_at(&scalars, &["weights"])
                .expect("the member is a map of scalars");
            let stated: Vec<(&str, &str)> = scalar_entries
                .iter()
                .map(|(key, value)| (key.as_str(), value.as_str()))
                .collect();
            assert_eq!(
                stated,
                vec![("2", "1.5"), ("3", "2"), ("4", "x"), ("5", "true")],
                "case {case}: expected every scalar as the map wrote it, got {stated:?}"
            );
            assert!(
                submitted_map_entries_at(&scalars, &["weights"]).is_none(),
                "case {case}: a map of numbers is no map of STRINGS"
            );

            // (what is asked for, the path asked for, the body) — nothing is read where there
            // is nothing to read.
            let reads_nothing: Vec<(&str, Vec<&str>, Vec<u8>)> = vec![
                ("no such member", vec!["nothing"], body.clone()),
                (
                    "a step that is not there",
                    vec!["rule", "attributes"],
                    nested.clone(),
                ),
                (
                    "a leaf that is not a map",
                    vec!["samplingRule", "ruleName"],
                    nested.clone(),
                ),
                (
                    "a member that is not a map of strings",
                    vec!["weights"],
                    scalars.clone(),
                ),
                (
                    "a body that is no map at all",
                    vec!["attributes"],
                    vec![0x01],
                ),
            ];
            for (asked, path, held) in reads_nothing {
                assert!(
                    submitted_map_entries_at(&held, &path).is_none(),
                    "case {case}, {asked}: expected nothing to be read"
                );
            }
        }
    }

    /// RULE: the submitted order is the entries as they were read, unchanged — the order a
    /// service whose layer keeps a map's insertion order states.
    #[test]
    fn the_submitted_order_states_the_entries_as_they_were_sent() {
        let entries: Vec<(String, String)> = [("z", "1"), ("a", "2")]
            .into_iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect();
        assert_eq!(
            in_submitted_order(&entries),
            Some(vec![("z", "1"), ("a", "2")]),
            "expected the submitted order"
        );
    }

    /// RULE: the table holding a map's entries starts at
    /// [`MAP_ENTRY_TABLE_MIN_BUCKETS`] buckets and doubles whenever the entries put into it
    /// pass three quarters of its width, so its width is decided by the entry count alone.
    #[test]
    fn the_table_doubles_when_the_entries_pass_three_quarters_of_its_width() {
        // (entries, the width that many entries are held in) — each bound and the count
        // just past it.
        let rows: [(usize, usize); 10] = [
            (0, 16),
            (1, 16),
            (12, 16),
            (13, 32),
            (24, 32),
            (25, 64),
            (48, 64),
            (49, 128),
            (96, 128),
            (97, 256),
        ];
        for (entries, expected) in rows {
            let actual = map_entry_table_buckets(entries);
            assert_eq!(
                actual, expected,
                "case {entries} entries: expected {expected} buckets, got {actual}"
            );
        }
    }

    /// RULE: a map stated from its hash table is stated in ascending bucket of each key's
    /// string hash at the table's own width, two keys of one bucket in key order — so the
    /// stated order follows the KEYS and the table's width, and neither the order the request
    /// wrote the entries in nor their sort order alone.
    #[test]
    fn a_hashed_map_is_stated_in_the_order_its_table_is_walked() {
        // A key of ONE ASCII character hashes to that character's own code point, and one
        // of two to `31 * <first> + <second>`; neither reaches the high half of the hash,
        // so each bucket below is that number at the table's width: 'a' (97) and 'q' (113)
        // share bucket 1 of a 16-bucket table, 'p' (112) sits in bucket 0, and 'b'..'l'
        // (98..108) in buckets 2..12.
        //
        // (case, the entries as submitted, the order the table is walked in)
        type Row<'r> = (&'r str, &'r [(&'r str, &'r str)], &'r [&'r str]);
        let rows: &[Row<'_>] = &[
            (
                "two keys in two buckets",
                &[("a", "1"), ("p", "2")],
                &["p", "a"],
            ),
            (
                "the same two, submitted the other way round",
                &[("p", "2"), ("a", "1")],
                &["p", "a"],
            ),
            (
                "two keys sharing a bucket, stated in key order",
                &[("a", "1"), ("q", "2")],
                &["a", "q"],
            ),
            (
                "the same two, submitted the other way round",
                &[("q", "2"), ("a", "1")],
                &["a", "q"],
            ),
            // Twelve entries fit a 16-bucket table, where 'p' sits in bucket 0 ahead of
            // every other key …
            (
                "twelve entries, in a table of sixteen buckets",
                &[
                    ("a", "1"),
                    ("b", "2"),
                    ("c", "3"),
                    ("d", "4"),
                    ("e", "5"),
                    ("f", "6"),
                    ("g", "7"),
                    ("h", "8"),
                    ("i", "9"),
                    ("j", "10"),
                    ("k", "11"),
                    ("p", "12"),
                ],
                &["p", "a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "k"],
            ),
            // … and one more entry doubles the table to 32 buckets, where 'p' sits in
            // bucket 16 and is stated last.
            (
                "thirteen entries, in a table of thirty-two buckets",
                &[
                    ("a", "1"),
                    ("b", "2"),
                    ("c", "3"),
                    ("d", "4"),
                    ("e", "5"),
                    ("f", "6"),
                    ("g", "7"),
                    ("h", "8"),
                    ("i", "9"),
                    ("j", "10"),
                    ("k", "11"),
                    ("l", "12"),
                    ("p", "13"),
                ],
                &["a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "k", "l", "p"],
            ),
            ("one entry", &[("z", "1")], &["z"]),
            ("no entries at all", &[], &[]),
        ];

        for (case, submitted, expected) in rows {
            let entries: Vec<(String, String)> = submitted
                .iter()
                .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
                .collect();
            let stated = in_hashed_entry_order(&entries).expect("an order is always stated");
            let keys: Vec<&str> = stated.iter().map(|(key, _)| *key).collect();
            assert_eq!(
                keys,
                expected.to_vec(),
                "case {case:?}: expected {expected:?}, got {keys:?}"
            );
            // The map is RE-ORDERED, never changed: every entry is stated once, with the
            // value it was submitted with.
            let mut by_key = stated.clone();
            by_key.sort_unstable();
            let mut sorted_submitted = submitted.to_vec();
            sorted_submitted.sort_unstable();
            assert_eq!(
                by_key, sorted_submitted,
                "case {case:?}: the stated map is the submitted one re-ordered, got {stated:?}"
            );
        }
    }
}
