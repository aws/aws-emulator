// SPDX-License-Identifier: CC0-1.0

//! The UPDATE one request states, and how it is applied to an item: the four actions of an
//! `UpdateExpression` (and of the legacy `AttributeUpdates` map, which states the same
//! four), the right-hand side of a `SET` with the two functions it may call, and the walk
//! that applies them to the stored image.
//!
//! These are the rules about CHANGING an item, split out of [`super`] — which holds the
//! text an expression is read from and the rules about reading one — so each file carries
//! one concern. [`super`] re-exports them, so every caller still names them
//! `crate::state::…`.

use super::*;

/// One action of a parsed `UpdateExpression` (or of the legacy `AttributeUpdates`
/// map, which states the same four actions a request at a time).
#[derive(Debug, Clone)]
pub(super) enum UpdateAction {
    Set(DocumentPath, SetValue),
    Remove(DocumentPath),
    Add(DocumentPath, AttributeValue),
    Delete(DocumentPath, AttributeValue),
}

impl UpdateAction {
    /// The path this action writes to.
    pub(super) fn path(&self) -> &DocumentPath {
        match self {
            Self::Set(path, _) | Self::Remove(path) | Self::Add(path, _) | Self::Delete(path, _) => {
                path
            }
        }
    }

    /// Every value this action would STORE in the item, as the request stated it.
    ///
    /// An assignment stores what its operands evaluate to and an `ADD` stores the number
    /// or the collection it leaves behind, so a value either of them carries becomes item
    /// bytes. A `DELETE` SUBTRACTS the members its operand names — "those values are
    /// subtracted from the old set" — and a `REMOVE` carries no operand at all, so
    /// neither of them stores one.
    pub(super) fn stored_values(&self) -> Vec<&AttributeValue> {
        let mut values = Vec::new();
        match self {
            Self::Set(_, value) => value.collect_values(&mut values),
            Self::Add(_, value) => values.push(value),
            Self::Remove(_) | Self::Delete(..) => {}
        }
        values
    }
}

/// The right-hand side of one `SET`: one operand, or two joined by `+` or `-`.
#[derive(Debug, Clone)]
pub(super) enum SetValue {
    Single(SetOperand),
    Sum(SetOperand, SetOperand),
    Difference(SetOperand, SetOperand),
}

impl SetValue {
    /// Every value this right-hand side carries, at any depth of the functions it nests.
    pub(super) fn collect_values<'a>(&'a self, into: &mut Vec<&'a AttributeValue>) {
        match self {
            Self::Single(operand) => operand.collect_values(into),
            Self::Sum(left, right) | Self::Difference(left, right) => {
                left.collect_values(into);
                right.collect_values(into);
            }
        }
    }
}

/// One operand of a `SET`: a path, a literal value, or one of the two documented
/// functions.
#[derive(Debug, Clone)]
pub(super) enum SetOperand {
    Path(DocumentPath),
    Value(AttributeValue),
    /// `if_not_exists (path, operand)` — "if the item does not contain an attribute at
    /// the specified path, then if_not_exists evaluates to operand; otherwise, it
    /// evaluates to path".
    IfNotExists(DocumentPath, Box<SetOperand>),
    /// `list_append (operand, operand)` — "evaluates to a list with a new element
    /// added to it".
    ListAppend(Box<SetOperand>, Box<SetOperand>),
}

impl SetOperand {
    /// Every value this operand carries, at any depth: a function's arguments are
    /// operands of their own, and either of them may be a value.
    ///
    /// The nesting is bounded where the expression is parsed, so this walk is too.
    pub(super) fn collect_values<'a>(&'a self, into: &mut Vec<&'a AttributeValue>) {
        match self {
            Self::Path(_) => {}
            Self::Value(value) => into.push(value),
            Self::IfNotExists(_, operand) => operand.collect_values(into),
            Self::ListAppend(left, right) => {
                left.collect_values(into);
                right.collect_values(into);
            }
        }
    }
}

/// Which item an update is applied to: one the addressed key already HOLDS, or the one
/// the update is about to add there ("adds a new item to the table if it does not
/// already exist" — `UpdateItem`'s own doc).
///
/// What it decides is where a DOCUMENT PATH is resolved. A path reaches into the item
/// the update edits, so a step that reaches no container the next step can read
/// addresses no location OF THAT ITEM and the update is refused
/// ([`DocumentPath::addresses_a_location`]) — a statement about a STORED image. A key
/// that holds no item has none: there is no stored value for the path to be invalid
/// for, so the update is applied as the create it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateTarget {
    /// The item the addressed key holds.
    StoredItem,
    /// The item this update would add at a key that holds none.
    NewItem,
}

impl UpdateTarget {
    /// Which of the two a caller holding the stored item (or nothing) is applying to.
    pub fn of(stored: Option<&Item>) -> Self {
        match stored {
            Some(_) => Self::StoredItem,
            None => Self::NewItem,
        }
    }
}

/// The update one request states, from `UpdateExpression` or from the legacy
/// `AttributeUpdates` map, parsed once and then applied to the stored item.
#[derive(Debug, Clone)]
pub struct ParsedUpdate {
    /// The input member the update travelled in, which is what every refusal this
    /// update raises names.
    pub(super) member: &'static str,
    pub(super) actions: Vec<UpdateAction>,
}

impl ParsedUpdate {
    /// Whether this update states NO action at all — an `UpdateItem` carrying neither an
    /// `UpdateExpression` nor one `AttributeUpdates` entry.
    ///
    /// That is the one update that adds the item its key names without writing anything
    /// into it: there is no clause whose effect on an item that is not there could be
    /// nothing, so what the call asks for is the item at that key
    /// ("adds a new item to the table if it does not already exist"). An update that DOES
    /// state clauses adds an item only where one of them gave it a value.
    pub fn states_no_action(&self) -> bool {
        self.actions.is_empty()
    }

    /// What the SHAPE of this update costs the item-size accounting, on top of the bytes
    /// the item it leaves behind stores.
    ///
    /// An update is sized against the documented 400 KB item ceiling over the stored item
    /// PLUS a frame for each clause the request states: the ceiling bounds what one write
    /// may ask for, and a clause asks for work whatever the item ends up holding. So the
    /// largest operand a write can carry shrinks by a fixed amount for every clause it
    /// states, and each clause KIND has its own frame ([`SET_CLAUSE_CHARGE`] and the
    /// constants beside it). Nothing of the request's text, of the attribute names it
    /// writes or of the values it binds is read here: one clause of a kind costs what that
    /// kind costs, so two requests stating the same clauses are sized alike.
    ///
    /// A `REMOVE` is the one action whose frame depends on the request, because what it
    /// asks for is the path itself: nothing is written, so the ask IS the path it names,
    /// charged whether or not the item holds anything there.
    ///
    /// The frames are DIFFERENCES, each read off the boundary of a write that already carries
    /// one assignment ([`super::request_shape`]), so the clause that write carries is the
    /// baseline and costs nothing: a single-clause update is sized against the item ceiling
    /// over what the item it leaves is charged and nothing besides
    /// ([`super::update_charged_size`]). What a frame says is how much SHORTER the operand gets
    /// when a further clause is stated, which is what [`BASELINE_CLAUSE_CHARGE`] takes back out
    /// of the sum.
    pub fn request_shape_charge(&self) -> i64 {
        let mut charge: i64 = 0;
        for action in &self.actions {
            charge = charge.saturating_add(match action {
                UpdateAction::Set(_, value) => set_clause_charge(value),
                UpdateAction::Remove(path) => REMOVE_CLAUSE_CHARGE
                    .saturating_add(i64::try_from(path.render().len()).unwrap_or(i64::MAX)),
                UpdateAction::Add(..) => ADD_CLAUSE_CHARGE,
                UpdateAction::Delete(..) => DELETE_CLAUSE_CHARGE,
            });
        }
        // Never below zero: an update whose only clause carries a frame smaller than the
        // baseline's asks for no more than that baseline does.
        (charge - BASELINE_CLAUSE_CHARGE).max(0)
    }

    /// What an ARITHMETIC assignment is charged in place of the number it computes: the
    /// widest OPERAND it read.
    ///
    /// What such a clause asks for is the two operands it has to read before it can store
    /// anything — the number it computes from them is the service's own result, not something
    /// the caller sent — so the item-size accounting is charged those operands and never the
    /// digit width of the result. Which is why the largest operand one write may carry does
    /// not move when the same arithmetic happens to carry into another digit: one pair of
    /// 2-digit operands summing to a 2-digit result and another summing to a 3-digit one admit
    /// the same write.
    ///
    /// Stated as the DIFFERENCE the gate applies to the bytes the item it is handed stores
    /// (which already carry the computed number), so it is added to the clause frames beside
    /// it ([`Self::request_shape_charge`]) rather than re-sizing the item: positive where the
    /// operands are wider than the result, negative where the result carried into a digit the
    /// operands do not have, zero for every other clause kind.
    ///
    /// A READ of the item as it stood BEFORE the update, which is what the operands were
    /// evaluated against: an operand this reading cannot evaluate — a path the item does not
    /// hold, a value that is no number — states no arithmetic the gate could charge and is
    /// left to the pass that applies the update.
    pub(super) fn arithmetic_operand_charge(&self, before: Option<&Item>) -> i64 {
        let absent = Item::new();
        let before = before.unwrap_or(&absent);
        let mut charge: i64 = 0;
        for action in &self.actions {
            let UpdateAction::Set(_, value) = action else {
                continue;
            };
            let (SetValue::Sum(left, right) | SetValue::Difference(left, right)) = value else {
                continue;
            };
            let Ok(computed) = evaluate_set_value(self.member, value, before) else {
                continue;
            };
            let widest = [left, right]
                .into_iter()
                .filter_map(|operand| evaluate_set_operand(self.member, operand, before).ok())
                .map(|operand| attribute_value_size(&operand.value))
                .max()
                .unwrap_or_default();
            // The operand charged IN and the computed number charged OUT, saturatingly: both
            // are sizes of one attribute value, so neither arithmetic can leave this range.
            charge = charge
                .saturating_add(widest)
                .saturating_sub(attribute_value_size(&computed));
        }
        charge
    }

    /// What the SHAPE of the request that stated this update costs the item-size
    /// accounting ([`UpdateRequestShape`]).
    ///
    /// Which is a fact of the MEMBER the update travelled in: an `UpdateExpression`
    /// states clauses and is charged their frames beside the floor under the part of the
    /// item it does not address, while the legacy per-attribute map states no clause at all
    /// and is charged the item alone.
    ///
    /// `before` is the item the addressed key held when the update was applied (`None` for a
    /// key that held none), which is what an arithmetic clause's operands were read against
    /// ([`Self::arithmetic_operand_charge`]).
    pub fn request_shape(&self, before: Option<&Item>) -> UpdateRequestShape {
        if self.member == UPDATE_EXPRESSION_MEMBER {
            UpdateRequestShape::StatedAsAnExpression {
                clauses: self
                    .request_shape_charge()
                    .saturating_add(self.arithmetic_operand_charge(before)),
                addressed: self.addressed_attributes(),
            }
        } else {
            UpdateRequestShape::StatedWithoutAnExpression
        }
    }

    /// The top-level attributes this update's clauses ADDRESS — the attribute every action
    /// names, whether it writes into it, reaches inside it or takes something away.
    ///
    /// What the item-size accounting charges in full rather than against the floor under the
    /// part of the item the change leaves alone ([`UpdateRequestShape`]): an attribute a
    /// clause names is what the request asked for, while the rest of the item is what the
    /// change found there.
    pub(super) fn addressed_attributes(&self) -> ::std::collections::BTreeSet<String> {
        self.actions
            .iter()
            .map(|action| action.path().attribute().to_string())
            .collect()
    }

    /// Parse one `UpdateExpression`: the `SET`, `REMOVE`, `ADD` and `DELETE` sections,
    /// each at most once, each a comma-separated list of actions.
    pub fn parse(
        member: &'static str,
        expression: &str,
        substitutions: Substitutions<'_>,
    ) -> Result<Self, RequestError> {
        let read = tokenize_expression_spanned(member, expression)?;
        let mut parser = ExprParser::over(member, &read, substitutions);
        let actions = parser.update_actions()?;
        // This member groups with parentheses too — an arithmetic operand pair may be
        // written inside one — so it is held to the same grouping rule every other grammar
        // of this service is ([`ExprParser::check_no_redundant_parentheses`]): a pair that
        // wraps nothing but another pair states nothing about how the expression is read and
        // is refused, before anything is applied.
        parser.check_no_redundant_parentheses()?;
        parser.check_no_reserved_name()?;
        Self::from_actions(member, actions)
    }

    /// The same update, as the legacy `AttributeUpdates` map states it: one action per
    /// top-level attribute, `PUT` by default (the `AttributeValueUpdate$Action` doc).
    ///
    /// The map's entry count is the request's to choose and this legacy member carries
    /// no modelled bound, so it is clamped BEFORE one action per entry is built.
    pub fn from_attribute_updates(
        updates: &HashMap<String, AttributeValueUpdate>,
    ) -> Result<Self, RequestError> {
        if updates.len() > MAX_UPDATE_ACTIONS {
            return Err(too_many_actions(updates.len()));
        }
        let mut actions = Vec::new();
        for (name, update) in updates {
            let path = DocumentPath::attribute_named(name);
            let action = update.action.as_deref().unwrap_or("PUT");
            match action {
                "PUT" => actions.push(UpdateAction::Set(
                    path,
                    SetValue::Single(SetOperand::Value(required_update_value(name, update)?)),
                )),
                // "DELETE - If no value is specified, the attribute and its value are
                // removed from the item", and with a set value those members are
                // subtracted from the stored set.
                "DELETE" => actions.push(match &update.value {
                    None => UpdateAction::Remove(path),
                    Some(value) => UpdateAction::Delete(path, value.clone()),
                }),
                "ADD" => {
                    actions.push(UpdateAction::Add(path, required_update_value(name, update)?));
                }
                other => {
                    return Err(RequestError::Validation(format!(
                        "One or more parameter values were invalid: Unknown AttributeUpdates \
                         action: {other} for attribute {name}"
                    )))
                }
            }
        }
        Self::from_actions(ATTRIBUTE_UPDATES_MEMBER, actions)
    }

    /// Hold the actions, refusing two that write overlapping document locations:
    /// DynamoDB cannot apply both, so the request is wrong rather than one of them
    /// winning silently.
    ///
    /// The action count comes from the request, so it is CLAMPED here — before the
    /// overlap check, before [`Self::apply`] walks it and before any caller takes the
    /// store's lock — and the overlap check itself indexes the paths rather than
    /// comparing every pair: a path overlaps another exactly when one is a step-prefix
    /// of the other, so each path only has to look its own prefixes up. The index
    /// keeps each path's POSITION in the update, because that is what orders the two
    /// paths in the refusal ([`overlapping_paths`]).
    pub(super) fn from_actions(
        member: &'static str,
        actions: Vec<UpdateAction>,
    ) -> Result<Self, RequestError> {
        if actions.len() > MAX_UPDATE_ACTIONS {
            return Err(too_many_actions(actions.len()));
        }
        let paths: Vec<&DocumentPath> = actions.iter().map(UpdateAction::path).collect();
        check_paths_disjoint(member, &paths)?;
        // A value an action would STORE has to fit the item that would have to hold it,
        // so it is held to the documented item ceiling here — a fact of the request,
        // settled before any table is looked up. The ceiling is worded as the UPDATE path
        // words that quota, because the item that would cross it is the one this change
        // leaves behind. A value the update only COMPARES or SUBTRACTS never becomes item
        // bytes, which is why it is the action's stored values that are weighed and not
        // every entry of the substitution table it was read from
        // ([`UpdateAction::stored_values`]).
        for action in &actions {
            for value in action.stored_values() {
                if attribute_value_size(value) > MAX_ITEM_SIZE_BYTES {
                    return Err(RequestError::Validation(
                        ITEM_UPDATE_SIZE_EXCEEDED_MESSAGE.to_string(),
                    ));
                }
            }
        }
        Ok(Self { member, actions })
    }

    /// The update a request that states none makes: nothing at all. `UpdateItem`
    /// still "adds a new item to the table if it does not already exist", so such a
    /// request creates the item from its key alone.
    pub fn nothing() -> Self {
        Self {
            member: "UpdateExpression",
            actions: Vec::new(),
        }
    }

    /// Apply every action to `item`, reporting the top-level attributes that changed.
    ///
    /// Every operand reads the item as it stood BEFORE the update, and so does every
    /// document path: all the clauses of one update resolve against ONE pre-image, so
    /// a clause that shortens a list cannot move the location a later clause named.
    /// That is what orders the two phases below — the writes first (writing an
    /// existing location, and appending past the end of a list, leave every earlier
    /// index where the pre-image had it), then the removals greatest path first (so
    /// removing one location never shifts a smaller one that is still to be removed).
    ///
    /// Within each phase the order is the PATH's, not the order the expression spells
    /// its clauses in: the writes smallest path first, the removals greatest path
    /// first. Which only shows where two clauses write past the end of ONE list — each
    /// such write appends ("if the index is greater than the list's size, the new
    /// element is appended"), so the element of the SMALLER index lands first and the
    /// appended run reads in index order, whichever order the clauses were written in.
    /// Everywhere else the two orders agree, because one update's paths are disjoint
    /// ([`check_paths_disjoint`]) and a write to a location the pre-image already holds
    /// moves no other location.
    ///
    /// A value an action COMPUTES goes through [`check_attribute_value`] before it is
    /// written, exactly as one a request sent does, and the item is clamped AS IT
    /// GROWS — the room check and the running member count both precede each write —
    /// so a many-action update cannot multiply a per-value bound into an unbounded item
    /// while it is being built, however many actions the expression states. Those checks
    /// run in the write phase's own order, so an update whose clauses break more than one
    /// of them is refused for the SMALLEST path that breaks one rather than for the
    /// first clause written. A value past the documented item ceiling is worded as the
    /// UPDATE path words that quota ([`stated_for_an_update`]): the item that crossed it
    /// is the one this change would leave behind.
    ///
    /// `target` says whether the item is one the table HOLDS: the document paths of an
    /// update are resolved against the item it edits, and a key that holds no item has
    /// no stored image for a path to be invalid for
    /// ([`UpdateTarget`], [`DocumentPath::addresses_a_location`]).
    pub fn apply(
        &self,
        item: &mut Item,
        target: UpdateTarget,
    ) -> Result<UpdateOutcome, RequestError> {
        let before = item.clone();
        let mut changed: Vec<DocumentPath> = Vec::new();
        let mut members_written = 0usize;
        let mut removals: Vec<&DocumentPath> = Vec::new();
        let mut writes: Vec<&UpdateAction> = Vec::new();
        for action in &self.actions {
            // Every action addresses a location of the STORED item it is applied to, and a
            // path whose steps before the last reach no container the last step can read
            // addresses none ([`DocumentPath::addresses_a_location`]) — whichever action
            // states it, and whether or not applying it would have changed anything. So
            // the update is refused rather than served as the no-op it would otherwise be.
            //
            // Asked of a STORED item alone: the rule is about the item the update edits,
            // and an update of a key that holds none is the create it is — the item it
            // leaves behind is built here, so a clause that WRITES still needs the
            // location it writes into ([`DocumentPath::set`] refuses a parent that is not
            // there) while a clause that only takes something away has nothing to do.
            if target == UpdateTarget::StoredItem && !action.path().addresses_a_location(&before)
            {
                return Err(action.path().invalid());
            }
            match action {
                UpdateAction::Remove(path) => removals.push(path),
                _ => writes.push(action),
            }
        }
        writes.sort_by(|one, two| one.path().0.cmp(&two.path().0));
        for action in writes {
            let path = action.path();
            match action {
                UpdateAction::Set(path, value) => {
                    let value = evaluate_set_value(self.member, value, &before)?;
                    check_attribute_value(path.attribute(), &value)
                        .map_err(stated_for_an_update)?;
                    members_written =
                        check_members_written(members_written, path.attribute(), &value)?;
                    check_room_for(item, path, &value)?;
                    path.set(item, value)?;
                    changed.push(path.clone());
                }
                // Collected above: a removal is applied in the second phase.
                UpdateAction::Remove(_) => {}
                UpdateAction::Add(_, value) => {
                    let added = apply_add(self.member, path, value, &before)?;
                    check_attribute_value(path.attribute(), &added)
                        .map_err(stated_for_an_update)?;
                    members_written =
                        check_members_written(members_written, path.attribute(), &added)?;
                    check_room_for(item, path, &added)?;
                    path.set(item, added)?;
                    changed.push(path.clone());
                }
                UpdateAction::Delete(_, value) => {
                    if apply_delete(self.member, path, value, &before, item)? {
                        changed.push(path.clone());
                    }
                }
            }
        }
        removals.sort_by(|one, two| two.0.cmp(&one.0));
        for path in removals {
            if path.remove(item).is_some() {
                changed.push(path.clone());
            }
        }
        Ok(UpdateOutcome { changed })
    }
}

/// What one update changed: every document path it wrote or removed.
///
/// Kept as PATHS rather than as the attribute names they reach into, because the two
/// things a caller asks about them are different questions: which of the item's
/// attributes the update touched (a key attribute among them refuses the whole update),
/// and which of the locations the expression NAMED hold a value afterwards (what the
/// `UPDATED_*` forms report).
#[derive(Debug, Clone, Default)]
pub struct UpdateOutcome {
    pub(super) changed: Vec<DocumentPath>,
}

impl UpdateOutcome {
    /// The top-level attributes the update changed.
    pub fn attributes(&self) -> BTreeSet<String> {
        self.changed
            .iter()
            .map(|path| path.attribute().to_string())
            .collect()
    }

    /// The `Attributes` an `UPDATED_*` projection of `item` reports.
    ///
    /// "Returns only the updated attributes" is read PER DOCUMENT PATH, not per
    /// attribute: what is reported for each path the update changed is the value AT
    /// that path, wrapped in the containers the path steps through — `REMOVE l[1]`
    /// reports `l` as a one-element list holding whatever index 1 addresses now, not
    /// the whole stored list. A path that addresses nothing in `item` contributes
    /// nothing, which is what a `SET` past the end of a list leaves behind: the element
    /// is appended, so the index the expression named holds nothing afterwards.
    ///
    /// Reported by the one reporter this service projects document paths with
    /// ([`crate::state::project_paths`]), so two changed paths that step through the
    /// same container — the same map entry or the same list index — are reported under
    /// one entry for it here exactly as they are for a `ProjectionExpression`.
    pub(super) fn projected_paths(&self, item: &Item) -> Item {
        crate::state::project_paths(item, &self.changed)
    }
}

/// The value of an `AttributeValueUpdate` that needs one.
pub(super) fn required_update_value(
    name: &str,
    update: &AttributeValueUpdate,
) -> Result<AttributeValue, RequestError> {
    update.value.clone().ok_or_else(|| {
        RequestError::Validation(format!(
            "One or more parameter values were invalid: Only DELETE action is allowed when no \
             attribute value is specified: {name}"
        ))
    })
}

/// What one `ADD` leaves at its path: "If the existing attribute is a number, and if
/// Value is also a number, then Value is mathematically added to the existing
/// attribute"; a set is unioned; and an attribute that is not there yet starts from
/// `0` for a number, or from the empty set, so the value itself is the result.
///
/// The LEGACY per-attribute map reaches one operand the expression's `ADD` does not: a
/// LIST, which is CONCATENATED onto the list the attribute holds — "If the attribute
/// does not already exist, then the attribute and its values are added to the item. If
/// the attribute does exist, then the behavior of ADD depends on the data type of the
/// attribute" (`AttributeValueUpdate$Action`), and recorded behaviour shows that
/// data type including a list. The expression member states its own operand domain —
/// "The ADD action only supports Number and set data types"
/// (`UpdateItem$UpdateExpression`) — so a list operand written there is outside the
/// operator's domain and refused as such ([`ADD_OPERAND`]).
pub(super) fn apply_add(
    member: &str,
    path: &DocumentPath,
    value: &AttributeValue,
    before: &Item,
) -> Result<AttributeValue, RequestError> {
    if set_len(value) == Some(0) {
        return Err(RequestError::Validation(format!(
            "One or more parameter values were invalid: An ADD action may not specify an empty \
             set: {}",
            path.render()
        )));
    }
    if let (AttributeValue::L(appended), true) = (value, appends_lists(member)) {
        return match path.get(before) {
            None => Ok(value.clone()),
            Some(AttributeValue::L(held)) => {
                // Both lists are already bounded by [`MAX_COLLECTION_MEMBERS`] — the
                // stored one at admission, the operand where this request's values were
                // read — and the concatenation is held to that ceiling again where the
                // result is checked ([`Self::apply`]), so what is built here is bounded.
                let mut members = held.clone();
                members.extend(appended.iter().cloned());
                Ok(AttributeValue::L(members))
            }
            Some(_held) => Err(incorrect_update_operand_type()),
        };
    }
    // The operand's own type is a fact of the REQUEST, so an operand outside what `ADD`
    // takes is refused before the item is read at all.
    ADD_OPERAND.check(member, value)?;
    match path.get(before) {
        None => match value {
            AttributeValue::N(_) | AttributeValue::Ns(_) | AttributeValue::Ss(_)
            | AttributeValue::Bs(_) => Ok(value.clone()),
            _other => Err(incorrect_update_operand_type()),
        },
        Some(AttributeValue::N(held)) => match value {
            AttributeValue::N(increment) => Ok(AttributeValue::N(number_arithmetic(
                held, increment, false,
            )?)),
            _other => Err(incorrect_update_operand_type()),
        },
        Some(held) if set_members(held).is_some() => set_union(held, value),
        Some(_held) => Err(incorrect_update_operand_type()),
    }
}

/// Apply one `DELETE`, reporting whether it changed the item. Deleting from an
/// attribute that is not there is nothing to do, the way the legacy action's doc says:
/// "DELETE - Nothing happens; there is no attribute to delete".
pub(super) fn apply_delete(
    member: &str,
    path: &DocumentPath,
    value: &AttributeValue,
    before: &Item,
    item: &mut Item,
) -> Result<bool, RequestError> {
    if set_len(value) == Some(0) {
        return Err(RequestError::Validation(format!(
            "One or more parameter values were invalid: A DELETE action may not specify an empty \
             set: {}",
            path.render()
        )));
    }
    // As for `ADD`: an operand `DELETE` cannot subtract is refused whatever the item
    // holds at that path, including nothing.
    DELETE_OPERAND.check(member, value)?;
    let Some(held) = path.get(before) else {
        return Ok(false);
    };
    match set_difference(held, value)? {
        Some(remaining) => path.set(item, remaining)?,
        None => {
            path.remove(item);
        }
    }
    Ok(true)
}

/// One evaluated `SET` operand: the value it stands for, and whether the REQUEST spelled
/// that value — a `:placeholder` it bound — rather than the item holding it.
///
/// Which of the two it is decides how a type the operator cannot take is refused
/// ([`mismatched_update_operand`]), so it travels with the value rather than being guessed
/// from the operand's shape afterwards.
#[derive(Debug, Clone)]
pub(super) struct EvaluatedOperand {
    pub(super) value: AttributeValue,
    pub(super) stated_by_the_request: bool,
}

/// The refusal an update operand whose type its operator or function cannot take earns,
/// decided by WHERE that type came from.
///
/// A type the REQUEST spelled is a fact of the expression's own text, so AWS names the
/// operator or function and that type. A type read from the stored ITEM is known only once
/// the item is read, and AWS states its fixed class sentence for it
/// ([`INCORRECT_UPDATE_OPERAND_TYPE_MESSAGE`]) — the same sentence `ADD` and `DELETE` state
/// for a stored attribute their operand cannot be combined with, naming neither operand.
pub(super) fn mismatched_update_operand(
    member: &str,
    operator_or_function: &str,
    operand: &EvaluatedOperand,
) -> RequestError {
    if operand.stated_by_the_request {
        return incorrect_operand_type(
            member,
            operator_or_function,
            attribute_value_type(&operand.value),
        );
    }
    incorrect_update_operand_type()
}

/// The value one `SET` writes, evaluated against the item as it stood before the
/// update.
pub(super) fn evaluate_set_value(
    member: &str,
    value: &SetValue,
    before: &Item,
) -> Result<AttributeValue, RequestError> {
    match value {
        SetValue::Single(operand) => Ok(evaluate_set_operand(member, operand, before)?.value),
        SetValue::Sum(left, right) | SetValue::Difference(left, right) => {
            let subtract = matches!(value, SetValue::Difference(_, _));
            let left = evaluate_set_operand(member, left, before)?;
            let right = evaluate_set_operand(member, right, before)?;
            match (&left.value, &right.value) {
                (AttributeValue::N(one), AttributeValue::N(two)) => Ok(AttributeValue::N(
                    number_arithmetic(one, two, subtract)?,
                )),
                // Arithmetic takes two numbers, so the operand that is not one is what
                // the refusal names — the left one when neither is.
                _ => {
                    let offending = if matches!(left.value, AttributeValue::N(_)) {
                        &right
                    } else {
                        &left
                    };
                    Err(mismatched_update_operand(
                        member,
                        if subtract { "-" } else { "+" },
                        offending,
                    ))
                }
            }
        }
    }
}

/// One `SET` operand's value. A path that addresses nothing is the caller's mistake —
/// there is no value to write — rather than a silent no-op.
pub(super) fn evaluate_set_operand(
    member: &str,
    operand: &SetOperand,
    before: &Item,
) -> Result<EvaluatedOperand, RequestError> {
    match operand {
        SetOperand::Value(value) => Ok(EvaluatedOperand {
            value: value.clone(),
            stated_by_the_request: true,
        }),
        SetOperand::Path(path) => path
            .get(before)
            .cloned()
            .map(|value| EvaluatedOperand {
                value,
                stated_by_the_request: false,
            })
            .ok_or_else(|| RequestError::Validation(MISSING_OPERAND_ATTRIBUTE_MESSAGE.to_string())),
        SetOperand::IfNotExists(path, fallback) => match path.get(before) {
            Some(value) => Ok(EvaluatedOperand {
                value: value.clone(),
                stated_by_the_request: false,
            }),
            None => evaluate_set_operand(member, fallback, before),
        },
        SetOperand::ListAppend(first, second) => {
            let first = evaluate_set_operand(member, first, before)?;
            let second = evaluate_set_operand(member, second, before)?;
            // The concatenation states what the request stated only where BOTH its
            // operands did: a list the item held is what makes the result the item's.
            let stated_by_the_request =
                first.stated_by_the_request && second.stated_by_the_request;
            match (&first.value, &second.value) {
                (AttributeValue::L(head), AttributeValue::L(tail)) => {
                    // The concatenation is bounded BEFORE it is built, and the result
                    // is held to the documented item size: a `list_append` nested
                    // inside another would otherwise double its operand per level, and
                    // only the outermost result ever reaches the item-size check.
                    let total = head.len().saturating_add(tail.len());
                    if total > MAX_COLLECTION_MEMBERS {
                        return Err(too_many_members("the list being appended to", total));
                    }
                    let mut appended = head.clone();
                    appended.extend(tail.iter().cloned());
                    let appended = AttributeValue::L(appended);
                    if attribute_value_size(&appended) > MAX_ITEM_SIZE_BYTES {
                        return Err(RequestError::Validation(
                            ITEM_UPDATE_SIZE_EXCEEDED_MESSAGE.to_string(),
                        ));
                    }
                    Ok(EvaluatedOperand {
                        value: appended,
                        stated_by_the_request,
                    })
                }
                // `list_append` concatenates two lists, so the operand that is not a
                // list is what the refusal names, whichever position it arrived in.
                _ => {
                    let offending = if matches!(first.value, AttributeValue::L(_)) {
                        &second
                    } else {
                        &first
                    };
                    Err(mismatched_update_operand(member, "list_append", offending))
                }
            }
        }
    }
}
